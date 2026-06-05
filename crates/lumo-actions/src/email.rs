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
use lumo_core::{Action, ActionRegistry, ActionResult, ResourceFactory, RunTeardown, StepCtx};
use lumo_dsl::ResourceDecl;
use once_cell::sync::Lazy;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::Value;
use std::path::PathBuf;
use std::sync::Arc;

use crate::resource_store::ResourceStore;

pub fn register(r: &mut ActionRegistry) {
    r.register(SendAction);
    r.register(FetchAction);
    // T3: a `spec.resources.<name>` of kind `smtp` is one transport reused by
    // every `email.send` that binds to it (so its connection pool persists across
    // messages), reclaimed at run end. Unbound sends build a transport per call.
    r.register_teardown(Arc::new(SmtpTeardown));
    r.register_resource_factory(Arc::new(SmtpFactory));
}

// ─── T3: `smtp` resource — one transport per run, reused across sends ──────────
//
// A `spec.resources.<name>: {kind: smtp}` is one `AsyncSmtpTransport` built once
// on the first `email.send` that binds to it (`Step.resource`), kept in
// `SMTP_TRANSPORTS` keyed by `(run_id, name)`, reused by every later bound send —
// so lettre's connection pool stays warm across messages — and dropped at run end
// by [`SmtpTeardown`]. An unbound send builds a transport per call, as before.
//
// Unlike db/http, the connection identity (host/port/**credentials**) is NOT read
// from the decl: credentials must stay vault-resolved, and the decl's config is
// static YAML (never template-resolved). So the transport is built from the FIRST
// bound send's inputs (vault-resolved like always); later bound sends reuse that
// transport and their own host/port/credentials are ignored. The per-step network
// gate on `host` still runs on every send (defense in depth).

const SMTP_KIND: &str = "smtp";

/// Transports opened for `smtp` resources, keyed `(run_id, resource name)`.
/// `AsyncSmtpTransport` is `Send + Sync + Clone` (it shares a pool internally),
/// so the cached `Arc` is sent straight to `send` without cloning.
static SMTP_TRANSPORTS: ResourceStore<AsyncSmtpTransport<Tokio1Executor>> = ResourceStore::new();

/// The `smtp` resource the send binds to, or `None` when it binds nothing (or
/// binds a non-`smtp` / undeclared resource) — in which case it builds a
/// transport per call, exactly as before.
fn smtp_slot(ctx: &StepCtx) -> Option<String> {
    let name = ctx.current_resource()?;
    match ctx.resource_decl(&name) {
        Ok(decl) if decl.kind == SMTP_KIND => Some(name),
        _ => None,
    }
}

/// Build an implicit-TLS (rustls) SMTP transport for `host:port` with `creds`.
/// Shared by the bound (reused) and unbound (per-call) send paths.
fn build_transport(
    host: &str,
    port: u16,
    username: String,
    password: String,
) -> Result<AsyncSmtpTransport<Tokio1Executor>, StepError> {
    let creds = Credentials::new(username, password);
    Ok(AsyncSmtpTransport::<Tokio1Executor>::relay(host)
        .map_err(|e| StepError::msg(format!("email.send tls setup: {e}")))?
        .port(port)
        .credentials(creds)
        .build())
}

/// Build (once) and return the shared transport for an `smtp` resource at
/// `(run_id, slot)`, reusing it on later sends. The first bound send's
/// connection params establish it; later sends reuse it. Idempotent: on a
/// concurrent open the first transport wins and the loser is dropped.
async fn ensure_transport(
    run_id: &str,
    slot: &str,
    host: &str,
    port: u16,
    username: String,
    password: String,
) -> Result<Arc<AsyncSmtpTransport<Tokio1Executor>>, StepError> {
    if let Some(t) = SMTP_TRANSPORTS.get(run_id, slot) {
        return Ok(t);
    }
    let t = build_transport(host, port, username, password)?;
    Ok(SMTP_TRANSPORTS.get_or_put(run_id, slot, Arc::new(t)))
}

/// Drop every `smtp` transport opened for `run_id` (releasing its connection
/// pool). Idempotent — a no-op when the run opened none. The end-of-run teardown
/// body; also exposed for tests.
#[doc(hidden)]
pub fn close_run_transports(run_id: &str) {
    let _ = SMTP_TRANSPORTS.take_run(run_id);
}

/// Whether any `smtp` resource transport is currently open for `run_id`. For tests.
#[doc(hidden)]
pub fn smtp_transport_open(run_id: &str) -> bool {
    SMTP_TRANSPORTS.has_run(run_id)
}

/// End-of-run hook: drops every `smtp` transport for the run so cached SMTP
/// connections don't linger past the flow.
struct SmtpTeardown;

#[async_trait]
impl RunTeardown for SmtpTeardown {
    async fn teardown(&self, run_id: &str) {
        close_run_transports(run_id);
    }
}

/// T3 resource factory for `smtp`. Like `http`, the transport is built **lazily**
/// by the first bound send, not here — its credentials are vault-resolved step
/// inputs, not part of the decl that `open` receives. So `open` only validates
/// the declaration; the action opens-and-reuses via [`ensure_transport`].
struct SmtpFactory;

#[async_trait]
impl ResourceFactory for SmtpFactory {
    fn kind(&self) -> &str {
        SMTP_KIND
    }

    async fn open(&self, _decl: &ResourceDecl, _run_id: &str, _name: &str) -> Result<(), StepError> {
        Ok(())
    }
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
            return Err(StepError::msg(
                "email.send: `to` must have at least one recipient",
            ));
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
        // T3:绑定了 `smtp` 资源时,复用本 run 的共享 transport(连接池跨多封信
        // 保活)——首封建立、后续复用;未绑定则按原行为每封新建 transport。
        let resp = match smtp_slot(ctx) {
            Some(name) => {
                let mailer =
                    ensure_transport(ctx.run_id(), &name, &host, port, username, password).await?;
                mailer
                    .send(message)
                    .await
                    // 错误里只带 SMTP 层信息,不回显凭据。
                    .map_err(|e| StepError::msg(format!("email.send smtp: {e}")))?
            }
            None => {
                let mailer = build_transport(&host, port, username, password)?;
                mailer
                    .send(message)
                    .await
                    .map_err(|e| StepError::msg(format!("email.send smtp: {e}")))?
            }
        };

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

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn decl(yaml: &str) -> ResourceDecl {
        serde_yaml::from_str(yaml).expect("valid ResourceDecl yaml")
    }

    fn ctx_with(resources: &[(&str, &str)], current: Option<&str>) -> StepCtx {
        let map: BTreeMap<String, ResourceDecl> = resources
            .iter()
            .map(|(name, yaml)| (name.to_string(), decl(yaml)))
            .collect();
        let ctx = StepCtx::new(
            "run-smtp".into(),
            "flow-smtp".into(),
            ActionRegistry::new(),
            None,
            Value::Null,
            lumo_dsl::Capabilities::default(),
            Vec::new(),
        )
        .with_resources(map);
        ctx.set_current_resource(current);
        ctx
    }

    #[test]
    fn smtp_slot_selects_only_smtp_kind_bindings() {
        let resources = &[("mail", "kind: smtp\n"), ("db", "kind: sqlite\npath: /tmp/x\n")];
        assert_eq!(
            smtp_slot(&ctx_with(resources, Some("mail"))).as_deref(),
            Some("mail")
        );
        // Unbound / non-smtp / undeclared ⇒ None ⇒ per-call transport (back-compat).
        assert_eq!(smtp_slot(&ctx_with(resources, None)), None);
        assert_eq!(smtp_slot(&ctx_with(resources, Some("db"))), None);
        assert_eq!(smtp_slot(&ctx_with(resources, Some("ghost"))), None);
    }

    #[test]
    fn smtp_factory_kind_matches() {
        assert_eq!(SmtpFactory.kind(), SMTP_KIND);
        assert_eq!(SMTP_KIND, "smtp");
    }

    // `relay(host).build()` is lazy (no socket until `send`), so the reuse +
    // teardown contract is testable without an SMTP server.
    #[tokio::test]
    async fn smtp_resource_opens_one_transport_then_reuses_it() {
        let run = "smtp-reuse-unit";
        assert!(!smtp_transport_open(run));
        let t1 = ensure_transport(run, "mail", "smtp.example.com", 465, "u".into(), "p".into())
            .await
            .unwrap();
        // The first bound send establishes the transport; a later send reuses it
        // (its own host/credentials are ignored — same handle returned).
        let t2 = ensure_transport(run, "mail", "other.example.com", 25, "x".into(), "y".into())
            .await
            .unwrap();
        assert!(
            Arc::ptr_eq(&t1, &t2),
            "second ensure reuses the cached transport"
        );
        assert!(smtp_transport_open(run));
        close_run_transports(run);
        assert!(!smtp_transport_open(run));
        close_run_transports(run); // no-op, must not panic
    }
}
