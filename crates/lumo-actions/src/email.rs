//! Email 动作:SMTP 发信(`email.send`)+ IMAP 收信/标记/搬移
//! (`email.fetch` / `email.mark` / `email.move`)。
//!
//! 两条链路统一走 **rustls + ring**,刻意避开 openssl / native-tls 的 C 依赖,
//! 以满足信创 / 跨平台交叉编译的硬性要求(见 `Cargo.toml` 注释)。
//! MIME 解析用 mail-parser(纯 Rust,同一红线)。
//!
//! 能力门控顺序与 `http.rs` 一致——**先校验入参、再 gate 文件系统(发信附件
//! fs.read / 收信附件落盘 fs.write)、最后 gate 网络主机**,所有门控都在建立任何
//! socket 之前完成,因此未授权的能力会以 `capability denied` 快速失败,而不会
//! 泄漏成连接错误。
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
use mail_parser::{MessageParser, MimeHeaders};
use once_cell::sync::Lazy;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::Value;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::resource_store::ResourceStore;

pub fn register(r: &mut ActionRegistry) {
    r.register(SendAction);
    r.register(FetchAction);
    r.register(MarkAction);
    r.register(MoveAction);
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

pub(crate) const SMTP_KIND: &str = "smtp";

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

/// 单封邮件全文大小上限默认 10 MiB——RPA 流程取全文是为了解析,不是为了搬运
/// 超大邮件;超限**显式报错**(而非静默跳过),让 flow 作者自己决定调大或过滤。
const DEFAULT_MAX_BYTES: u64 = 10 * 1024 * 1024;

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
    /// 是否取回并解析正文(`text` / `html` 字段)。默认 false——保持旧版
    /// 仅头部的行为与流量开销。
    #[serde(default)]
    include_body: bool,
    /// 附件落盘目录。给出时每封邮件输出 `attachments: [{filename, path, size}]`;
    /// 目录走 fs.write 门控,文件名经消毒(剥路径段/`..`),重名自动加序号。
    #[serde(default)]
    save_attachments_to: Option<String>,
    /// 单封邮件原始字节(RFC822 全文)上限;仅在取全文(include_body 或
    /// save_attachments_to)时生效,超限该封显式报错。默认 10 MiB。
    #[serde(default = "default_max_bytes")]
    max_bytes_per_message: u64,
}
fn default_mailbox() -> String {
    "INBOX".into()
}
fn default_limit() -> u32 {
    10
}
fn default_max_bytes() -> u64 {
    DEFAULT_MAX_BYTES
}

#[async_trait]
impl Action for FetchAction {
    fn id(&self) -> &'static str {
        "email.fetch"
    }
    fn summary(&self) -> &'static str {
        "Fetch the latest N messages from an IMAP mailbox (headers; optional body text/html and attachment download)"
    }
    fn schema(&self) -> &'static serde_json::Value {
        static SCHEMA: Lazy<Value> = Lazy::new(crate::schema::derive::<FetchIn>);
        &SCHEMA
    }
    async fn execute(&self, ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let input: FetchIn = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("email.fetch input invalid: {e}")))?;

        // 1) 校验:limit 必须为正,否则没有可取的窗口。
        if input.limit == 0 {
            return Err(StepError::msg("email.fetch: `limit` must be >= 1"));
        }
        if input.max_bytes_per_message == 0 {
            return Err(StepError::msg(
                "email.fetch: `max_bytes_per_message` must be >= 1",
            ));
        }

        // 2) fs.write 门控:附件落盘目录在触网之前授权(目录形授权如
        //    `./attachments/**` 同时覆盖目录本身;消毒后的文件名出不了该目录)。
        let save_dir: Option<PathBuf> = match &input.save_attachments_to {
            Some(dir) => {
                let p = PathBuf::from(dir);
                ctx.ensure_fs_write(&p)?;
                Some(p)
            }
            None => None,
        };

        // 3) 网络门控:在建立任何 IMAP 连接之前 gate 远端主机。
        ctx.ensure_network_url(&input.host)?;

        let messages = fetch_messages(&input, save_dir.as_deref()).await?;
        Ok(ActionResult::from(Value::Array(messages)))
    }
}

/// rustls 隐式 TLS 的 IMAP 会话类型(fetch/mark/move 共用)。
type ImapSession = async_imap::Session<tokio_rustls::client::TlsStream<tokio::net::TcpStream>>;

/// 经 rustls 建立隐式 TLS 的 IMAP 连接并登录。`op` 仅用于错误信息前缀
/// (email.fetch / email.mark / email.move),凭据绝不回显。
async fn connect_login(
    op: &str,
    host: &str,
    port: u16,
    username: &str,
    password: &str,
) -> Result<ImapSession, StepError> {
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
        .map_err(|e| StepError::msg(format!("{op} invalid server name `{host}`: {e}")))?;

    let tcp = TcpStream::connect((host, port))
        .await
        .map_err(|e| StepError::msg(format!("{op} connect {host}:{port}: {e}")))?;
    let tls = connector
        .connect(server_name, tcp)
        .await
        .map_err(|e| StepError::msg(format!("{op} tls handshake: {e}")))?;

    // Client::new 不消费问候语——按 async-imap 文档先读一次服务器 greeting。
    let mut client = async_imap::Client::new(tls);
    let _greeting = client
        .read_response()
        .await
        .map_err(|e| StepError::msg(format!("{op} greeting: {e}")))?
        .ok_or_else(|| StepError::msg(format!("{op}: no IMAP greeting")))?;

    client
        .login(username, password)
        .await
        // login 失败会把 Client 一起还回来;只回显错误,不回显凭据。
        .map_err(|(e, _client)| StepError::msg(format!("{op} login: {e}")))
}

/// 登录并取 `mailbox` 中最新 `limit` 封信。仅头部时与旧版完全一致;需要全文
/// (正文或附件落盘)时用 `BODY.PEEK[]` 拉 RFC822 原始字节——PEEK 不动 `\Seen`
/// 标记,"标已读"是 `email.mark` 的职责,fetch 保持只读语义。
async fn fetch_messages(input: &FetchIn, save_dir: Option<&Path>) -> Result<Vec<Value>, StepError> {
    let mut session = connect_login(
        "email.fetch",
        &input.host,
        input.port,
        &input.username,
        &input.password,
    )
    .await?;

    let mailbox = session
        .select(&input.mailbox)
        .await
        .map_err(|e| StepError::msg(format!("email.fetch select `{}`: {e}", input.mailbox)))?;

    let total = mailbox.exists;
    if total == 0 {
        let _ = session.logout().await;
        return Ok(Vec::new());
    }

    // 取最新 N 封:序号窗口 [start..=total],降序输出(最新在前)。
    let start = total.saturating_sub(input.limit).saturating_add(1);
    let seq_set = format!("{start}:{total}");
    let need_raw = input.include_body || save_dir.is_some();
    let query = if need_raw {
        "(UID ENVELOPE INTERNALDATE BODY.PEEK[])"
    } else {
        "(UID ENVELOPE INTERNALDATE)"
    };

    let fetch_result: Result<Vec<Value>, StepError> = async {
        let stream = session
            .fetch(&seq_set, query)
            .await
            .map_err(|e| StepError::msg(format!("email.fetch FETCH: {e}")))?;
        let fetches: Vec<_> = stream
            .try_collect()
            .await
            .map_err(|e| StepError::msg(format!("email.fetch collect: {e}")))?;

        // 同一目录内的重名去重要跨整批邮件统计,否则两封信的同名附件会互相覆盖。
        let mut used_names: HashSet<String> = HashSet::new();
        let mut rows: Vec<Value> = Vec::with_capacity(fetches.len());
        for f in &fetches {
            let mut row = fetch_to_json(f);
            if need_raw {
                enrich_with_mime(&mut row, f, input, save_dir, &mut used_names).await?;
            }
            rows.push(row);
        }
        // 序号升序 → 反转成最新在前。
        rows.reverse();
        Ok(rows)
    }
    .await;

    let _ = session.logout().await;
    fetch_result
}

/// 解析一封信的 RFC822 原始字节,把正文(`text`/`html`)与附件清单叠加到 `row`。
/// 超过 `max_bytes_per_message` 显式报错(带 uid 与两侧字节数,便于定位调参)。
async fn enrich_with_mime(
    row: &mut Value,
    f: &async_imap::types::Fetch,
    input: &FetchIn,
    save_dir: Option<&Path>,
    used_names: &mut HashSet<String>,
) -> Result<(), StepError> {
    let uid_label = f
        .uid
        .map(|u| u.to_string())
        .unwrap_or_else(|| format!("seq {}", f.message));
    let raw = f.body().ok_or_else(|| {
        StepError::msg(format!("email.fetch: server returned no body for uid {uid_label}"))
    })?;
    if raw.len() as u64 > input.max_bytes_per_message {
        return Err(StepError::msg(format!(
            "email.fetch: message uid {uid_label} is {} bytes, over `max_bytes_per_message` ({})",
            raw.len(),
            input.max_bytes_per_message
        )));
    }
    let msg = parse_mime(raw)?;
    let obj = row.as_object_mut().expect("fetch_to_json yields an object");
    if input.include_body {
        // multipart/alternative 的首个 text/html 段;字符集(含 GBK/GB18030)
        // 由 mail-parser 解码成 UTF-8。缺失侧输出 null,便于 flow 判空。
        obj.insert(
            "text".into(),
            msg.body_text(0).map(|s| s.into_owned().into()).unwrap_or(Value::Null),
        );
        obj.insert(
            "html".into(),
            msg.body_html(0).map(|s| s.into_owned().into()).unwrap_or(Value::Null),
        );
    }
    if let Some(dir) = save_dir {
        let saved = save_attachments(&msg, dir, used_names).await?;
        obj.insert("attachments".into(), Value::Array(saved));
    }
    Ok(())
}

/// 解析 MIME。解析失败(非法/截断的消息)显式报错而非静默空值。
fn parse_mime(raw: &[u8]) -> Result<mail_parser::Message<'_>, StepError> {
    MessageParser::default()
        .parse(raw)
        .ok_or_else(|| StepError::msg("email.fetch: failed to parse message MIME"))
}

/// 把一封已解析邮件的全部附件写入 `dir`(已过 fs.write 门控),返回
/// `[{filename, path, size}]`。文件名先消毒再去重——`used` 跨多封邮件共享,
/// 叠加磁盘已存在检查,保证本批内与历史产物都不被覆盖。
async fn save_attachments(
    msg: &mail_parser::Message<'_>,
    dir: &Path,
    used: &mut HashSet<String>,
) -> Result<Vec<Value>, StepError> {
    let mut out = Vec::new();
    if msg.attachment_count() == 0 {
        return Ok(out);
    }
    tokio::fs::create_dir_all(dir)
        .await
        .map_err(|e| StepError::msg(format!("email.fetch mkdir {}: {e}", dir.display())))?;
    for part in msg.attachments() {
        let name = sanitize_filename(part.attachment_name().unwrap_or("attachment"));
        let final_name = dedup_filename(dir, &name, used);
        let path = dir.join(&final_name);
        let bytes = part.contents();
        tokio::fs::write(&path, bytes)
            .await
            .map_err(|e| StepError::msg(format!("email.fetch write {}: {e}", path.display())))?;
        used.insert(final_name.clone());
        out.push(serde_json::json!({
            "filename": final_name,
            "path": path.display().to_string(),
            "size": bytes.len(),
        }));
    }
    Ok(out)
}

/// 附件名消毒:只取最后一个路径段(同时处理 `/` 与 `\`),剥控制字符,把 `..`
/// 换成 `_`——附件名来自不可信的发件方,绝不能携带路径穿越出落盘目录。
/// 清洗后为空(或只剩 `.`)时兜底为 `attachment`。
fn sanitize_filename(raw: &str) -> String {
    let last = raw.rsplit(['/', '\\']).next().unwrap_or("");
    let cleaned: String = last.chars().filter(|c| !c.is_control()).collect();
    let cleaned = cleaned.replace("..", "_");
    let trimmed = cleaned.trim();
    if trimmed.is_empty() || trimmed == "." {
        "attachment".into()
    } else {
        trimmed.to_string()
    }
}

/// 重名加序号:`report.txt` → `report_1.txt` → `report_2.txt`……同时避开
/// 本批已用名(`used`)与磁盘已有文件。
fn dedup_filename(dir: &Path, name: &str, used: &HashSet<String>) -> String {
    let taken = |n: &str| used.contains(n) || dir.join(n).exists();
    if !taken(name) {
        return name.to_string();
    }
    let (stem, ext) = match name.rsplit_once('.') {
        // `.hidden` 这种点开头的名字整体当 stem,不拆出空 stem。
        Some((s, e)) if !s.is_empty() => (s, Some(e)),
        _ => (name, None),
    };
    for n in 1u32.. {
        let candidate = match ext {
            Some(e) => format!("{stem}_{n}.{e}"),
            None => format!("{stem}_{n}"),
        };
        if !taken(&candidate) {
            return candidate;
        }
    }
    unreachable!("u32 sequence exhausted before finding a free filename")
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

// ─── email.mark / email.move 共用入参形状 ───────────────────────────────────────

/// `uid` 字段:单个 UID 或 UID 列表(untagged,两种写法都合法)。
#[derive(Deserialize, JsonSchema)]
#[serde(untagged)]
enum UidSpec {
    One(u32),
    Many(Vec<u32>),
}

impl UidSpec {
    fn to_vec(&self) -> Vec<u32> {
        match self {
            UidSpec::One(u) => vec![*u],
            UidSpec::Many(v) => v.clone(),
        }
    }
}

/// 校验非空并渲染成 IMAP uid-set(逗号分隔)。空列表无事可做,显式报错。
fn uid_set(op: &str, uids: &[u32]) -> Result<String, StepError> {
    if uids.is_empty() {
        return Err(StepError::msg(format!(
            "{op}: `uid` must name at least one message"
        )));
    }
    Ok(uids
        .iter()
        .map(|u| u.to_string())
        .collect::<Vec<_>>()
        .join(","))
}

// ─── email.mark (IMAP UID STORE) ────────────────────────────────────────────────

pub struct MarkAction;

/// 要置/清的标记。seen/unseen 与 flagged/unflagged 是同一面旗的正反向,
/// deleted 只置 `\Deleted`(物理删除由 `expunge` 决定)。
#[derive(Deserialize, JsonSchema, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
enum MarkSet {
    Seen,
    Unseen,
    Flagged,
    Unflagged,
    Deleted,
}

impl MarkSet {
    /// UID STORE 的 ±FLAGS 查询串。`.SILENT` 免去服务器逐封回显新 FLAGS。
    fn store_query(self) -> &'static str {
        match self {
            MarkSet::Seen => "+FLAGS.SILENT (\\Seen)",
            MarkSet::Unseen => "-FLAGS.SILENT (\\Seen)",
            MarkSet::Flagged => "+FLAGS.SILENT (\\Flagged)",
            MarkSet::Unflagged => "-FLAGS.SILENT (\\Flagged)",
            MarkSet::Deleted => "+FLAGS.SILENT (\\Deleted)",
        }
    }
    fn as_str(self) -> &'static str {
        match self {
            MarkSet::Seen => "seen",
            MarkSet::Unseen => "unseen",
            MarkSet::Flagged => "flagged",
            MarkSet::Unflagged => "unflagged",
            MarkSet::Deleted => "deleted",
        }
    }
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct MarkIn {
    host: String,
    port: u16,
    username: String,
    /// VM 已解析的明文口令。仅用于鉴权,绝不落日志。
    password: String,
    #[serde(default = "default_mailbox")]
    mailbox: String,
    /// 单个 UID 或 UID 列表。
    uid: UidSpec,
    /// 标记动作:seen / unseen / flagged / unflagged / deleted。
    set: MarkSet,
    /// 置 `deleted` 后是否立即 EXPUNGE 物理删除。默认 false(只打标,可撤销)。
    /// 仅 `set: deleted` 时允许为 true。
    #[serde(default)]
    expunge: bool,
}

#[async_trait]
impl Action for MarkAction {
    fn id(&self) -> &'static str {
        "email.mark"
    }
    fn summary(&self) -> &'static str {
        "Set or clear IMAP flags (seen/flagged/deleted) on messages by UID, optionally expunging"
    }
    fn schema(&self) -> &'static serde_json::Value {
        static SCHEMA: Lazy<Value> = Lazy::new(crate::schema::derive::<MarkIn>);
        &SCHEMA
    }
    async fn execute(&self, ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let input: MarkIn = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("email.mark input invalid: {e}")))?;

        // 1) 校验:uid 非空;expunge 只配 deleted(其他组合是笔误,显式拒绝)。
        let uids = input.uid.to_vec();
        let set_str = uid_set("email.mark", &uids)?;
        if input.expunge && input.set != MarkSet::Deleted {
            return Err(StepError::msg(
                "email.mark: `expunge: true` is only valid with `set: deleted`",
            ));
        }

        // 2) 网络门控:在建立任何 IMAP 连接之前 gate 远端主机。
        ctx.ensure_network_url(&input.host)?;

        let mut session = connect_login(
            "email.mark",
            &input.host,
            input.port,
            &input.username,
            &input.password,
        )
        .await?;

        let work: Result<u64, StepError> = async {
            session
                .select(&input.mailbox)
                .await
                .map_err(|e| StepError::msg(format!("email.mark select `{}`: {e}", input.mailbox)))?;
            {
                // SILENT 模式仍可能有零星响应,流要喝干才能复用会话。
                let stream = session
                    .uid_store(&set_str, input.set.store_query())
                    .await
                    .map_err(|e| StepError::msg(format!("email.mark UID STORE: {e}")))?;
                let _: Vec<_> = stream
                    .try_collect()
                    .await
                    .map_err(|e| StepError::msg(format!("email.mark store collect: {e}")))?;
            }
            let mut expunged: u64 = 0;
            if input.expunge {
                // 注意:RFC3501 EXPUNGE 清的是邮箱里**所有** `\Deleted` 的信,
                // 不止本次 uid 集(UID EXPUNGE 需 UIDPLUS 扩展,这里不依赖)。
                let stream = session
                    .expunge()
                    .await
                    .map_err(|e| StepError::msg(format!("email.mark EXPUNGE: {e}")))?;
                let seqs: Vec<_> = stream
                    .try_collect()
                    .await
                    .map_err(|e| StepError::msg(format!("email.mark expunge collect: {e}")))?;
                expunged = seqs.len() as u64;
            }
            Ok(expunged)
        }
        .await;

        let _ = session.logout().await;
        let expunged = work?;

        Ok(ActionResult::from(serde_json::json!({
            "ok": true,
            "uids": uids,
            "set": input.set.as_str(),
            "expunged": expunged,
        })))
    }
}

// ─── email.move (IMAP UID MOVE / COPY+EXPUNGE 回退) ─────────────────────────────

pub struct MoveAction;

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct MoveIn {
    host: String,
    port: u16,
    username: String,
    /// VM 已解析的明文口令。仅用于鉴权,绝不落日志。
    password: String,
    /// 源邮箱。
    #[serde(default = "default_mailbox")]
    mailbox: String,
    /// 单个 UID 或 UID 列表。
    uid: UidSpec,
    /// 目标邮箱(须已存在;IMAP 对不存在的目标会报 TRYCREATE)。
    to_mailbox: String,
}

#[async_trait]
impl Action for MoveAction {
    fn id(&self) -> &'static str {
        "email.move"
    }
    fn summary(&self) -> &'static str {
        "Move messages by UID to another IMAP mailbox (UID MOVE, falling back to COPY+EXPUNGE)"
    }
    fn schema(&self) -> &'static serde_json::Value {
        static SCHEMA: Lazy<Value> = Lazy::new(crate::schema::derive::<MoveIn>);
        &SCHEMA
    }
    async fn execute(&self, ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let input: MoveIn = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("email.move input invalid: {e}")))?;

        // 1) 校验:uid 非空;目标邮箱非空且不同于源(同箱搬移是笔误)。
        let uids = input.uid.to_vec();
        let set_str = uid_set("email.move", &uids)?;
        if input.to_mailbox.trim().is_empty() {
            return Err(StepError::msg("email.move: `to_mailbox` must not be empty"));
        }
        if input.to_mailbox == input.mailbox {
            return Err(StepError::msg(
                "email.move: `to_mailbox` must differ from `mailbox`",
            ));
        }

        // 2) 网络门控:在建立任何 IMAP 连接之前 gate 远端主机。
        ctx.ensure_network_url(&input.host)?;

        let mut session = connect_login(
            "email.move",
            &input.host,
            input.port,
            &input.username,
            &input.password,
        )
        .await?;

        let work: Result<&'static str, StepError> = async {
            session
                .select(&input.mailbox)
                .await
                .map_err(|e| StepError::msg(format!("email.move select `{}`: {e}", input.mailbox)))?;
            // 服务器支持 RFC6851 MOVE 时走原子 UID MOVE;否则回退为
            // COPY + STORE \Deleted + EXPUNGE 三步(语义同 MOVE,见 RFC6851 §3.1;
            // 注意回退的 EXPUNGE 同样会清掉邮箱里其他已置 \Deleted 的信)。
            let caps = session
                .capabilities()
                .await
                .map_err(|e| StepError::msg(format!("email.move CAPABILITY: {e}")))?;
            if caps.has_str("MOVE") {
                session
                    .uid_mv(&set_str, &input.to_mailbox)
                    .await
                    .map_err(|e| StepError::msg(format!("email.move UID MOVE: {e}")))?;
                Ok("uid_move")
            } else {
                session
                    .uid_copy(&set_str, &input.to_mailbox)
                    .await
                    .map_err(|e| StepError::msg(format!("email.move UID COPY: {e}")))?;
                {
                    let stream = session
                        .uid_store(&set_str, "+FLAGS.SILENT (\\Deleted)")
                        .await
                        .map_err(|e| StepError::msg(format!("email.move UID STORE: {e}")))?;
                    let _: Vec<_> = stream
                        .try_collect()
                        .await
                        .map_err(|e| StepError::msg(format!("email.move store collect: {e}")))?;
                }
                {
                    let stream = session
                        .expunge()
                        .await
                        .map_err(|e| StepError::msg(format!("email.move EXPUNGE: {e}")))?;
                    let _: Vec<_> = stream
                        .try_collect()
                        .await
                        .map_err(|e| StepError::msg(format!("email.move expunge collect: {e}")))?;
                }
                Ok("copy_expunge")
            }
        }
        .await;

        let _ = session.logout().await;
        let method = work?;

        Ok(ActionResult::from(serde_json::json!({
            "ok": true,
            "uids": uids,
            "from": input.mailbox,
            "to_mailbox": input.to_mailbox,
            "method": method,
        })))
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

    // ─── email.fetch MIME 解析 / 附件落盘(本地 .eml fixture,不触网) ──────────

    /// multipart/mixed:text+html(alternative)+ 三个附件——两个重名、一个
    /// 带路径穿越名。覆盖正文双格式、重名加序号、文件名消毒三条规则。
    fn fixture_eml() -> Vec<u8> {
        let s = concat!(
            "From: Alice <a@example.com>\r\n",
            "To: b@example.com\r\n",
            "Subject: =?utf-8?B?5L2g5aW9?=\r\n",
            "MIME-Version: 1.0\r\n",
            "Content-Type: multipart/mixed; boundary=\"XYZ\"\r\n",
            "\r\n",
            "--XYZ\r\n",
            "Content-Type: multipart/alternative; boundary=\"ABC\"\r\n",
            "\r\n",
            "--ABC\r\n",
            "Content-Type: text/plain; charset=utf-8\r\n",
            "\r\n",
            "你好 plain\r\n",
            "--ABC\r\n",
            "Content-Type: text/html; charset=utf-8\r\n",
            "\r\n",
            "<b>你好 html</b>\r\n",
            "--ABC--\r\n",
            "--XYZ\r\n",
            "Content-Type: application/octet-stream; name=\"report.txt\"\r\n",
            "Content-Disposition: attachment; filename=\"report.txt\"\r\n",
            "\r\n",
            "DATA1\r\n",
            "--XYZ\r\n",
            "Content-Type: application/octet-stream\r\n",
            "Content-Disposition: attachment; filename=\"report.txt\"\r\n",
            "\r\n",
            "DATA-2\r\n",
            "--XYZ\r\n",
            "Content-Type: application/octet-stream\r\n",
            "Content-Disposition: attachment; filename=\"../../evil.sh\"\r\n",
            "\r\n",
            "EVIL\r\n",
            "--XYZ--\r\n",
        );
        s.as_bytes().to_vec()
    }

    #[test]
    fn mime_body_text_and_html_extracted() {
        let raw = fixture_eml();
        let msg = parse_mime(&raw).expect("fixture parses");
        let text = msg.body_text(0).expect("text body");
        let html = msg.body_html(0).expect("html body");
        assert!(text.contains("你好 plain"), "text = {text:?}");
        assert!(html.contains("你好 html"), "html = {html:?}");
    }

    #[tokio::test]
    async fn attachments_saved_with_dedup_and_sanitized_names() {
        let raw = fixture_eml();
        let msg = parse_mime(&raw).expect("fixture parses");
        let dir = tempfile::tempdir().expect("tempdir");
        let mut used = HashSet::new();
        let saved = save_attachments(&msg, dir.path(), &mut used)
            .await
            .expect("save attachments");
        assert_eq!(saved.len(), 3, "all three attachments land: {saved:?}");

        // 重名加序号:第二个 report.txt 变 report_1.txt。
        assert_eq!(saved[0]["filename"], "report.txt");
        assert_eq!(saved[1]["filename"], "report_1.txt");
        // 路径穿越名被消毒成最后一段,文件绝不会写出目录之外。
        assert_eq!(saved[2]["filename"], "evil.sh");

        for (entry, want) in saved.iter().zip(["DATA1", "DATA-2", "EVIL"]) {
            let path = PathBuf::from(entry["path"].as_str().unwrap());
            assert!(path.starts_with(dir.path()), "stays in dir: {path:?}");
            let bytes = std::fs::read(&path).expect("saved file readable");
            assert_eq!(bytes, want.as_bytes());
            assert_eq!(entry["size"], serde_json::json!(want.len()));
        }
    }

    #[tokio::test]
    async fn attachment_dedup_also_avoids_preexisting_files_on_disk() {
        let raw = fixture_eml();
        let msg = parse_mime(&raw).expect("fixture parses");
        let dir = tempfile::tempdir().expect("tempdir");
        // 第二批落盘(模拟上一个 run 的产物已在目录里)不得覆盖既有文件。
        std::fs::write(dir.path().join("evil.sh"), b"OLD").unwrap();
        let mut used = HashSet::new();
        let saved = save_attachments(&msg, dir.path(), &mut used)
            .await
            .expect("save attachments");
        assert_eq!(saved[2]["filename"], "evil_1.sh");
        assert_eq!(
            std::fs::read(dir.path().join("evil.sh")).unwrap(),
            b"OLD",
            "pre-existing file untouched"
        );
    }

    #[test]
    fn sanitize_filename_strips_traversal_and_separators() {
        assert_eq!(sanitize_filename("report.txt"), "report.txt");
        assert_eq!(sanitize_filename("../../etc/passwd"), "passwd");
        assert_eq!(sanitize_filename("a/b\\c.txt"), "c.txt");
        assert_eq!(sanitize_filename("..\\..\\boot.ini"), "boot.ini");
        // 残留的 `..` 一律替换;空名/纯点兜底为 attachment。
        assert_eq!(sanitize_filename("a..b.txt"), "a_b.txt");
        assert_eq!(sanitize_filename(""), "attachment");
        assert_eq!(sanitize_filename("."), "attachment");
        assert_eq!(sanitize_filename("dir/"), "attachment");
        // 控制字符剥掉,正常中文名保留。
        assert_eq!(sanitize_filename("报\u{0}表.xlsx"), "报表.xlsx");
    }

    #[test]
    fn dedup_filename_handles_extensionless_and_hidden_names() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut used: HashSet<String> = HashSet::new();
        used.insert("README".into());
        assert_eq!(dedup_filename(dir.path(), "README", &used), "README_1");
        used.insert(".hidden".into());
        // 点开头的名字整体当 stem,不会拆出空 stem。
        assert_eq!(dedup_filename(dir.path(), ".hidden", &used), ".hidden_1");
    }
}
