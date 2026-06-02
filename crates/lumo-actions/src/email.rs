//! Email 动作:SMTP 发信(`email.send`)+ IMAP 收信(`email.fetch`)。
//!
//! 两条链路统一走 **rustls + ring**,刻意避开 openssl / native-tls 的 C 依赖,
//! 以满足信创 / 跨平台交叉编译的硬性要求(见 `Cargo.toml` 注释)。
//!
//! 能力门控顺序与 `http.rs` 一致——**先校验入参、再 gate fs.read(附件)、最后
//! gate 网络主机**,所有门控都在建立任何 socket 之前完成,因此未授权的能力会以
//! `capability denied` 快速失败,而不会泄漏成连接错误。
//!
//! 口令(`password`)由 VM 在动作执行前解析(flow 作者写 `${{ vault.* }}`),
//! 这里只当作普通入参接收,**绝不写入日志 / 步骤快照**。

use async_trait::async_trait;
use futures_util::TryStreamExt;
use lettre::message::{header::ContentType, Attachment, MultiPart, SinglePart};
use lettre::transport::smtp::authentication::Credentials;
use lettre::{AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor};
use lumo_core::error::StepError;
use lumo_core::{Action, ActionRegistry, ActionResult, StepCtx};
use once_cell::sync::Lazy;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::Value;
use std::path::PathBuf;
use std::sync::Arc;

pub fn register(r: &mut ActionRegistry) {
    r.register(SendAction);
    r.register(FetchAction);
}

// ─── email.send (SMTP) ─────────────────────────────────────────────────────────

pub struct SendAction;

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SendIn {
    host: String,
    port: u16,
    username: String,
    /// VM 已解析的明文口令。仅用于鉴权,绝不落日志。
    password: String,
    from: String,
    /// 收件人,至少一个。
    to: Vec<String>,
    #[serde(default)]
    cc: Vec<String>,
    #[serde(default)]
    subject: String,
    #[serde(default)]
    body: String,
    /// 可选 HTML 正文;给出时与纯文本 `body` 组成 `multipart/alternative`。
    #[serde(default)]
    html: Option<String>,
    /// 可选附件,逐个本地文件路径——每个都先过 fs.read 门控再读取。
    #[serde(default)]
    attachments: Vec<String>,
}

#[async_trait]
impl Action for SendAction {
    fn id(&self) -> &'static str {
        "email.send"
    }
    fn summary(&self) -> &'static str {
        "Send an email over SMTP (implicit TLS via rustls)"
    }
    fn schema(&self) -> &'static serde_json::Value {
        static SCHEMA: Lazy<Value> = Lazy::new(crate::schema::derive::<SendIn>);
        &SCHEMA
    }
    async fn execute(&self, ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let SendIn {
            host,
            port,
            username,
            password,
            from,
            to,
            cc,
            subject,
            body,
            html,
            attachments,
        } = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("email.send input invalid: {e}")))?;

        // 1) 校验:至少一个收件人。空数组直接挡掉,不浪费连接。
        if to.is_empty() {
            return Err(StepError::msg("email.send: `to` must have at least one recipient"));
        }

        // 2) 解析信封地址(纯字符串解析,不触网)。地址非法立即报错。
        let from_mbox = parse_mailbox("from", &from)?;
        let mut builder = Message::builder().from(from_mbox);
        for addr in &to {
            builder = builder.to(parse_mailbox("to", addr)?);
        }
        for addr in &cc {
            builder = builder.cc(parse_mailbox("cc", addr)?);
        }
        builder = builder.subject(subject);

        // 3) fs.read 门控:每个附件路径都要先授权,再读取。门控在触网之前。
        let mut parts: Vec<SinglePart> = Vec::new();
        for path in &attachments {
            let p = PathBuf::from(path);
            ctx.ensure_fs_read(&p)?;
            let bytes = tokio::fs::read(&p)
                .await
                .map_err(|e| StepError::msg(format!("email.send read {}: {e}", p.display())))?;
            let filename = p
                .file_name()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_else(|| "attachment".into());
            // application/octet-stream 是通用、永不出错的兜底类型;附件名让收件端
            // 决定如何打开。
            let ct = ContentType::parse("application/octet-stream")
                .map_err(|e| StepError::msg(format!("email.send content-type: {e}")))?;
            parts.push(Attachment::new(filename).body(bytes, ct));
        }

        // 组装正文 / 多部分结构。
        let message = build_message(builder, &body, html.as_deref(), parts)?;

        // 4) 网络门控:在建立任何 SMTP 连接之前 gate 远端主机。
        ctx.ensure_network_url(&host)?;

        // 5) 发送:relay() 走隐式 TLS(rustls wrapper),端口可由调用方覆盖。
        let creds = Credentials::new(username, password);
        let mailer = AsyncSmtpTransport::<Tokio1Executor>::relay(&host)
            .map_err(|e| StepError::msg(format!("email.send tls setup: {e}")))?
            .port(port)
            .credentials(creds)
            .build();

        let resp = mailer
            .send(message)
            .await
            // 错误里只带 SMTP 层信息,不回显凭据。
            .map_err(|e| StepError::msg(format!("email.send smtp: {e}")))?;

        let code = format!("{}", resp.code());
        let server_message: Vec<String> = resp.message().map(|s| s.to_string()).collect();
        Ok(ActionResult::from(serde_json::json!({
            "ok": true,
            "code": code,
            "message": server_message,
        })))
    }
}

/// 把 `"Name <addr@host>"` 或裸 `addr@host` 解析为 lettre `Mailbox`。
fn parse_mailbox(field: &str, raw: &str) -> Result<lettre::message::Mailbox, StepError> {
    raw.parse::<lettre::message::Mailbox>()
        .map_err(|e| StepError::msg(format!("email.send invalid `{field}` address `{raw}`: {e}")))
}

/// 依据是否有 HTML / 附件,组装最终 `Message`:
/// - 仅纯文本           → singlepart text/plain
/// - 文本 + HTML        → multipart/alternative
/// - 有附件             → multipart/mixed,首段为(文本或 alternative),其后挂附件
fn build_message(
    builder: lettre::message::MessageBuilder,
    body: &str,
    html: Option<&str>,
    attachments: Vec<SinglePart>,
) -> Result<Message, StepError> {
    let map = |e: lettre::error::Error| StepError::msg(format!("email.send build: {e}"));

    // 正文段:纯文本或 文本/HTML 二选一(alternative)。
    let body_part: MultiPart = match html {
        Some(h) => MultiPart::alternative_plain_html(body.to_string(), h.to_string()),
        None => MultiPart::mixed().singlepart(SinglePart::plain(body.to_string())),
    };

    if attachments.is_empty() {
        // 无附件:HTML 时直接发 alternative;纯文本时发单段 text/plain。
        return match html {
            Some(_) => builder.multipart(body_part).map_err(map),
            None => builder
                .singlepart(SinglePart::plain(body.to_string()))
                .map_err(map),
        };
    }

    // 有附件:mixed 容器,首段是正文(text 或 alternative),其后逐个挂附件。
    let mut mixed = MultiPart::mixed().multipart(body_part);
    for part in attachments {
        mixed = mixed.singlepart(part);
    }
    builder.multipart(mixed).map_err(map)
}

// ─── email.fetch (IMAP) ─────────────────────────────────────────────────────────

pub struct FetchAction;

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct FetchIn {
    host: String,
    port: u16,
    username: String,
    /// VM 已解析的明文口令。仅用于鉴权,绝不落日志。
    password: String,
    #[serde(default = "default_mailbox")]
    mailbox: String,
    #[serde(default = "default_limit")]
    limit: u32,
}
fn default_mailbox() -> String {
    "INBOX".into()
}
fn default_limit() -> u32 {
    10
}

#[async_trait]
impl Action for FetchAction {
    fn id(&self) -> &'static str {
        "email.fetch"
    }
    fn summary(&self) -> &'static str {
        "Fetch the latest N message headers from an IMAP mailbox (TLS via rustls)"
    }
    fn schema(&self) -> &'static serde_json::Value {
        static SCHEMA: Lazy<Value> = Lazy::new(crate::schema::derive::<FetchIn>);
        &SCHEMA
    }
    async fn execute(&self, ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let FetchIn {
            host,
            port,
            username,
            password,
            mailbox,
            limit,
        } = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("email.fetch input invalid: {e}")))?;

        // 1) 校验:limit 必须为正,否则没有可取的窗口。
        if limit == 0 {
            return Err(StepError::msg("email.fetch: `limit` must be >= 1"));
        }

        // 2) 网络门控:在建立任何 IMAP 连接之前 gate 远端主机。
        ctx.ensure_network_url(&host)?;

        let messages = fetch_headers(&host, port, &username, &password, &mailbox, limit).await?;
        Ok(ActionResult::from(Value::Array(messages)))
    }
}

/// 经 rustls 建立隐式 TLS 的 IMAP 连接,登录后取 `mailbox` 中最新 `limit` 封信的
/// 头部(uid / from / subject / date)。正文不取(见模块说明的取舍)。
async fn fetch_headers(
    host: &str,
    port: u16,
    username: &str,
    password: &str,
    mailbox: &str,
    limit: u32,
) -> Result<Vec<Value>, StepError> {
    use tokio::net::TcpStream;
    use tokio_rustls::rustls::{pki_types::ServerName, ClientConfig, RootCertStore};
    use tokio_rustls::TlsConnector;

    // rustls 根证书取自系统信任库(rustls-native-certs)。个别无法解析的证书
    // 跳过,不让一颗坏证书拖垮整条链路。
    let mut roots = RootCertStore::empty();
    let native = rustls_native_certs::load_native_certs();
    for cert in native.certs {
        let _ = roots.add(cert);
    }
    let config = ClientConfig::builder()
        .with_root_certificates(roots)
        .with_no_client_auth();
    let connector = TlsConnector::from(Arc::new(config));
    let server_name = ServerName::try_from(host.to_string())
        .map_err(|e| StepError::msg(format!("email.fetch invalid server name `{host}`: {e}")))?;

    let tcp = TcpStream::connect((host, port))
        .await
        .map_err(|e| StepError::msg(format!("email.fetch connect {host}:{port}: {e}")))?;
    let tls = connector
        .connect(server_name, tcp)
        .await
        .map_err(|e| StepError::msg(format!("email.fetch tls handshake: {e}")))?;

    // Client::new 不消费问候语——按 async-imap 文档先读一次服务器 greeting。
    let mut client = async_imap::Client::new(tls);
    let _greeting = client
        .read_response()
        .await
        .map_err(|e| StepError::msg(format!("email.fetch greeting: {e}")))?
        .ok_or_else(|| StepError::msg("email.fetch: no IMAP greeting"))?;

    let mut session = client
        .login(username, password)
        .await
        // login 失败会把 Client 一起还回来;只回显错误,不回显凭据。
        .map_err(|(e, _client)| StepError::msg(format!("email.fetch login: {e}")))?;

    let mailbox = session
        .select(mailbox)
        .await
        .map_err(|e| StepError::msg(format!("email.fetch select `{mailbox}`: {e}")))?;

    let total = mailbox.exists;
    let mut out = Vec::new();
    if total == 0 {
        let _ = session.logout().await;
        return Ok(out);
    }

    // 取最新 N 封:序号窗口 [start..=total],降序输出(最新在前)。
    let start = total.saturating_sub(limit).saturating_add(1);
    let seq_set = format!("{start}:{total}");
    let query = "(UID ENVELOPE INTERNALDATE)";

    let fetch_result: Result<Vec<Value>, StepError> = async {
        let stream = session
            .fetch(&seq_set, query)
            .await
            .map_err(|e| StepError::msg(format!("email.fetch FETCH: {e}")))?;
        let fetches: Vec<_> = stream
            .try_collect()
            .await
            .map_err(|e| StepError::msg(format!("email.fetch collect: {e}")))?;
        let mut rows: Vec<Value> = fetches.iter().map(fetch_to_json).collect();
        // 序号升序 → 反转成最新在前。
        rows.reverse();
        Ok(rows)
    }
    .await;

    let _ = session.logout().await;
    out.extend(fetch_result?);
    Ok(out)
}

/// 把一封 IMAP `Fetch` 的头部信息投影成 JSON 对象。
fn fetch_to_json(f: &async_imap::types::Fetch) -> Value {
    let envelope = f.envelope();
    let subject = envelope
        .and_then(|e| e.subject.as_ref())
        .map(|b| String::from_utf8_lossy(b).to_string());
    let from = envelope
        .and_then(|e| e.from.as_ref())
        .map(|addrs| addrs.iter().map(address_to_string).collect::<Vec<_>>())
        .unwrap_or_default();
    let date = f.internal_date().map(|d| d.to_rfc3339());

    serde_json::json!({
        "seq": f.message,
        "uid": f.uid,
        "from": from,
        "subject": subject,
        "date": date,
    })
}

/// 把 IMAP `Address`(mailbox@host,可选显示名)渲染成 `"Name <addr>"` 字符串。
fn address_to_string(addr: &async_imap::imap_proto::types::Address<'_>) -> String {
    let bytes = |c: &Option<std::borrow::Cow<'_, [u8]>>| {
        c.as_ref().map(|b| String::from_utf8_lossy(b).to_string())
    };
    let name = bytes(&addr.name);
    let mailbox = bytes(&addr.mailbox);
    let host = bytes(&addr.host);
    let email = match (mailbox, host) {
        (Some(m), Some(h)) => format!("{m}@{h}"),
        (Some(m), None) => m,
        _ => String::new(),
    };
    match name {
        Some(n) if !n.is_empty() && !email.is_empty() => format!("{n} <{email}>"),
        _ => email,
    }
}
