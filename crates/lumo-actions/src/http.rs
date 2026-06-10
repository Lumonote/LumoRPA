//! HTTP request action.

use async_trait::async_trait;
use lumo_core::error::StepError;
use lumo_core::{Action, ActionRegistry, ActionResult, ResourceFactory, RunTeardown, StepCtx};
use lumo_dsl::ResourceDecl;
use once_cell::sync::Lazy;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::Value;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use crate::resource_store::ResourceStore;

pub fn register(r: &mut ActionRegistry) {
    r.register(RequestAction);
    r.register(DownloadAction);
    r.register(UploadAction);
    r.register(OAuth2TokenAction);
    r.register(PaginateAction);
    // T3: a `spec.resources.<name>` of kind `http` is one gated client reused by
    // every step that binds to it (so the connection pool / keep-alive persists),
    // reclaimed at run end. Unbound steps build a fresh client per call (back-compat).
    r.register_teardown(Arc::new(HttpTeardown));
    r.register_resource_factory(Arc::new(HttpFactory));
}

/// Build a `reqwest::Client` whose redirect policy re-authorizes EVERY hop
/// against the caller's network capability grants. `reqwest` follows 3xx by
/// default, but the per-action `ctx.ensure_network_url` gate only sees the
/// initial URL — so a granted host could 302 to an ungranted internal target
/// (e.g. `169.254.169.254` cloud metadata), bypassing the network sandbox.
/// This closure closes that hole by re-checking each redirect target's host
/// with the SAME matcher the capability system uses (`host_matches_grants`).
/// Shared by `http.request` / `http.download` (and reusable by future HTTP
/// actions like `http.upload` / `notify.send`).
pub(crate) fn build_gated_client(
    grants: &[String],
    timeout_ms: u64,
) -> Result<reqwest::Client, StepError> {
    build_gated_client_with(grants, timeout_ms, None, None)
}

/// [`build_gated_client`] 的全参版:可选 `proxy`(http/https/socks5 URL,经
/// `Proxy::all` 对所有 scheme 生效)与可选 mTLS 客户端身份(rustls 路径,
/// `Identity::from_pem`)。SSRF 门禁不因走代理而放松:重定向逐跳重审的策略
/// 原样保留,且调用方必须先对目标 URL 与代理 URL 各过一次 `ensure_network_url`。
fn build_gated_client_with(
    grants: &[String],
    timeout_ms: u64,
    proxy: Option<&str>,
    identity: Option<reqwest::Identity>,
) -> Result<reqwest::Client, StepError> {
    let grants = grants.to_vec(); // owned, moved into the closure
    let mut builder = reqwest::Client::builder()
        .timeout(std::time::Duration::from_millis(timeout_ms))
        .redirect(reqwest::redirect::Policy::custom(move |attempt| {
            if attempt.previous().len() >= 10 {
                return attempt.error("too many redirects");
            }
            let host = attempt.url().host_str().map(|h| h.to_string());
            match host {
                Some(ref h) if lumo_core::host_matches_grants(h, &grants) => attempt.follow(),
                other => attempt.error(format!(
                    "redirect to ungranted host blocked (network capability): {other:?}"
                )),
            }
        }));
    if let Some(proxy) = proxy {
        // reqwest 0.12 的 `Proxy::all` 对未知 scheme 并不报错(内部 Matcher 容忍
        // 任意 scheme,无 scheme 还会自动补 http://,错配时可能退化成直连)——
        // 这里自行白名单校验:仅 http/https/socks5(h) 是受支持的代理协议,其余
        // 一律显式报错,绝不静默直连绕开操作者预期的出口路径。
        let lower = proxy.to_ascii_lowercase();
        let scheme_ok = ["http://", "https://", "socks5://", "socks5h://"]
            .iter()
            .any(|s| lower.starts_with(s));
        if !scheme_ok {
            return Err(StepError::msg(format!(
                "http proxy `{proxy}` invalid: scheme must be http/https/socks5"
            )));
        }
        builder = builder.proxy(
            reqwest::Proxy::all(proxy)
                .map_err(|e| StepError::msg(format!("http proxy `{proxy}` invalid: {e}")))?,
        );
    }
    if let Some(identity) = identity {
        builder = builder.identity(identity);
    }
    builder
        .build()
        .map_err(|e| StepError::msg(format!("http client: {e}")))
}

// ─── T3: `http` resource — one gated client per run, reused across steps ───────
//
// A `spec.resources.<name>: {kind: http, timeout_ms?: ...}` is one
// `reqwest::Client` built once on the first step that binds to it
// (`Step.resource`), kept in `HTTP_CLIENTS` keyed by `(run_id, name)`, reused by
// every later bound step — so the connection pool (TCP/TLS keep-alive) and
// redirect policy persist — and dropped at run end by [`HttpTeardown`]. An
// unbound step builds a fresh client per call, exactly as before.
//
// Mirrors the browser rule "the declaration wins when bound": a bound step uses
// the resource's declared `timeout_ms` (the per-step `timeout_ms` is for unbound
// steps). The redirect policy bakes in the run's network grants, which are
// constant for the whole run, so one cached client stays correctly SSRF-gated
// for every step.

pub(crate) const HTTP_KIND: &str = "http";

/// Gated clients opened for `http` resources, keyed `(run_id, resource name)`.
/// `reqwest::Client` is `Send + Sync` and cheaply clonable (it shares its pool
/// behind an `Arc`), so cloning the cached client per request reuses the pool.
static HTTP_CLIENTS: ResourceStore<reqwest::Client> = ResourceStore::new();

/// The `http` resource the step binds to, or `None` when it binds nothing (or
/// binds a non-`http` / undeclared resource) — in which case the step builds a
/// client per call, exactly as before.
fn http_slot(ctx: &StepCtx) -> Option<String> {
    let name = ctx.current_resource()?;
    match ctx.resource_decl(&name) {
        Ok(decl) if decl.kind == HTTP_KIND => Some(name),
        _ => None,
    }
}

/// The request timeout declared by an `http` resource (`timeout_ms:`, flattened
/// into `config`), defaulting to the same 30s a per-step request uses.
fn http_timeout_from_decl(decl: &ResourceDecl) -> u64 {
    decl.config
        .get("timeout_ms")
        .and_then(|v| v.as_u64())
        .unwrap_or_else(default_timeout_ms)
}

/// `http` resource decl 里的可选 `proxy`(http/https/socks5 URL,flattened 进
/// `config`)。对齐 `chromium.cdp` decl 的 proxy 读取模式(9c68607):decl 路径
/// 与 per-step 路径能力对齐,绑定资源的步骤由共享客户端统一走代理。
fn http_proxy_from_decl(decl: &ResourceDecl) -> Option<String> {
    decl.config
        .get("proxy")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
}

/// Build (once) and return the shared gated client for an `http` resource at
/// `(run_id, slot)`, reusing it on later calls. Idempotent: on a concurrent open
/// the first client wins and the loser is dropped by the store.
async fn ensure_client(
    run_id: &str,
    slot: &str,
    grants: &[String],
    timeout_ms: u64,
    proxy: Option<&str>,
) -> Result<Arc<reqwest::Client>, StepError> {
    if let Some(client) = HTTP_CLIENTS.get(run_id, slot) {
        return Ok(client);
    }
    let client = build_gated_client_with(grants, timeout_ms, proxy, None)?;
    Ok(HTTP_CLIENTS.get_or_put(run_id, slot, Arc::new(client)))
}

/// The client an HTTP step should use: a step bound to an `http` resource reuses
/// the run's shared client (built with the resource's declared timeout/proxy); an
/// unbound step builds a fresh client with its own `timeout_ms`. Returns an owned
/// `reqwest::Client` either way (a clone of the cached one shares its pool), so
/// callers are unchanged.
async fn client_for(ctx: &StepCtx, step_timeout_ms: u64) -> Result<reqwest::Client, StepError> {
    match http_slot(ctx) {
        Some(name) => {
            let decl = ctx.resource_decl(&name)?;
            let timeout_ms = http_timeout_from_decl(&decl);
            let proxy = http_proxy_from_decl(&decl);
            // decl 声明的代理也是一个网络目的地 —— 与目标 URL 同样过 SSRF 门禁,
            // 防止经未授权的中转主机绕过网络能力沙箱。
            if let Some(p) = &proxy {
                ctx.ensure_network_url(p)?;
            }
            let client = ensure_client(
                ctx.run_id(),
                &name,
                ctx.network_grants(),
                timeout_ms,
                proxy.as_deref(),
            )
            .await?;
            Ok((*client).clone())
        }
        None => build_gated_client(ctx.network_grants(), step_timeout_ms),
    }
}

/// per-step mTLS 客户端证书输入:`{ cert_path, key_path }`,均为 PEM 文件路径。
/// 拆成两个文件是业界发证的常见形状(cert.pem + key.pem);读取前各过一次
/// `fs.read` 能力门禁,拼成一份 PEM 喂 `Identity::from_pem`(rustls 路径,
/// 纯 Rust,严禁 native-tls)。
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct Mtls {
    /// 客户端证书 PEM(可含中间链)。
    cert_path: String,
    /// 私钥 PEM(PKCS#8 / RSA / EC)。
    key_path: String,
}

/// 读取并装配 mTLS 客户端身份。证书与私钥都是本地敏感文件 —— 与 `http.upload`
/// 读 `src` 同款,读取前必须过 `fs.read` 能力门禁(两个路径各审一次)。非法或
/// 不匹配的 PEM 在 `Identity::from_pem` 处显式报错,绝不静默降级成无证书请求。
async fn load_identity(ctx: &StepCtx, mtls: &Mtls) -> Result<reqwest::Identity, StepError> {
    let cert_path = PathBuf::from(&mtls.cert_path);
    let key_path = PathBuf::from(&mtls.key_path);
    ctx.ensure_fs_read(&cert_path)?;
    ctx.ensure_fs_read(&key_path)?;
    let mut pem = tokio::fs::read(&cert_path).await.map_err(|e| {
        StepError::msg(format!("http mtls: read cert {}: {e}", cert_path.display()))
    })?;
    let key = tokio::fs::read(&key_path)
        .await
        .map_err(|e| StepError::msg(format!("http mtls: read key {}: {e}", key_path.display())))?;
    // `Identity::from_pem`(rustls)吃「证书(链)+ 私钥」合一的 PEM 流;两段
    // 之间补一个换行,防 cert.pem 末尾无换行时两块 PEM 粘连解析失败。
    pem.push(b'\n');
    pem.extend_from_slice(&key);
    reqwest::Identity::from_pem(&pem)
        .map_err(|e| StepError::msg(format!("http mtls: invalid PEM identity: {e}")))
}

/// [`client_for`] 的步骤级覆写版:per-step `proxy` / `mtls` 是该步骤的私有网络
/// 配置,不能折进按 run 缓存的共享资源客户端(否则同 run 其它绑定步骤会被动
/// 走此代理 / 持此证书)—— 带任一者的步骤一律现建专属客户端,timeout 也取
/// 步骤值。两者皆缺省时回落到既有规则(绑定资源共享 / 未绑定现建)。
async fn client_for_with(
    ctx: &StepCtx,
    step_timeout_ms: u64,
    proxy: Option<&str>,
    mtls: Option<&Mtls>,
) -> Result<reqwest::Client, StepError> {
    if proxy.is_none() && mtls.is_none() {
        return client_for(ctx, step_timeout_ms).await;
    }
    // 代理本身也是一个网络目的地:proxy URL 与目标 URL 一样过 SSRF 门禁,防止
    // 经未授权中转主机绕过网络能力沙箱 —— 门禁不因走代理而放松,目标 URL 与
    // 重定向逐跳重审照旧。
    if let Some(p) = proxy {
        ctx.ensure_network_url(p)?;
    }
    let identity = match mtls {
        Some(m) => Some(load_identity(ctx, m).await?),
        None => None,
    };
    build_gated_client_with(ctx.network_grants(), step_timeout_ms, proxy, identity)
}

/// Drop every `http` client opened for `run_id` (releasing its connection pool).
/// Idempotent — a no-op when the run opened none. The end-of-run teardown body;
/// also exposed for tests.
#[doc(hidden)]
pub fn close_run_clients(run_id: &str) {
    let _ = HTTP_CLIENTS.take_run(run_id);
}

/// Whether any `http` resource client is currently open for `run_id`. For tests.
#[doc(hidden)]
pub fn http_client_open(run_id: &str) -> bool {
    HTTP_CLIENTS.has_run(run_id)
}

/// End-of-run hook: drops every `http` resource client for the run so cached
/// connection pools don't linger past the flow.
struct HttpTeardown;

#[async_trait]
impl RunTeardown for HttpTeardown {
    async fn teardown(&self, run_id: &str) {
        close_run_clients(run_id);
    }
}

/// T3 resource factory for `http`. Note: the gated client is built **lazily** by
/// the first bound step, not here — its redirect policy must bake in the run's
/// network grants, which live on the `StepCtx` (capabilities), not in the decl
/// that `open` receives. So `open` validates the declaration and returns; the
/// action opens-and-reuses via [`ensure_client`]. (If the VM ever drives `open`
/// directly it will need to pass grants — the trait's documented escape hatch.)
struct HttpFactory;

#[async_trait]
impl ResourceFactory for HttpFactory {
    fn kind(&self) -> &str {
        HTTP_KIND
    }

    async fn open(&self, _decl: &ResourceDecl, _run_id: &str, _name: &str) -> Result<(), StepError> {
        // No-op: the gated client is built lazily by the first bound step (its
        // redirect policy must bake in the run's network grants, which aren't in
        // the decl). Registered so the kind is discoverable for a future VM driver.
        Ok(())
    }
}

/// 把响应头收成 JSON 对象:单值头 → 字符串,同名多值头(典型如 `Set-Cookie`)
/// → 字符串数组,按到达顺序聚合(P2-3)。此前用 `HashMap<String, String>`
/// 收集,重复头只剩最后一个值 —— 多枚 `Set-Cookie` 被静默吞掉。
///
/// 向后兼容考量:绝大多数响应头都是单值,它们的输出形状(字符串)保持不变;
/// 只有真出现重复头时该键才升级成数组,所以既有 flow 取单值头的表达式不受
/// 影响。`http.request` / `http.upload` 共用本函数,装头逻辑只此一处。
fn headers_to_json(headers: &reqwest::header::HeaderMap) -> Value {
    let mut out = serde_json::Map::new();
    // `HeaderMap` 迭代时同名多值头会按值逐条产出(键重复出现),逐条聚合即可。
    for (name, value) in headers {
        let v = Value::String(value.to_str().unwrap_or("").to_string());
        match out.entry(name.as_str().to_string()) {
            serde_json::map::Entry::Vacant(slot) => {
                slot.insert(v);
            }
            serde_json::map::Entry::Occupied(mut slot) => match slot.get_mut() {
                // 第三个及之后的值:追加进已有数组。
                Value::Array(arr) => arr.push(v),
                // 第二个值:把单值升级成双元素数组。
                single => {
                    let first = single.take();
                    *slot.get_mut() = Value::Array(vec![first, v]);
                }
            },
        }
    }
    Value::Object(out)
}

pub struct RequestAction;

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ReqIn {
    #[serde(default = "default_method")]
    method: String,
    url: String,
    #[serde(default)]
    headers: HashMap<String, String>,
    #[serde(default)]
    query: HashMap<String, String>,
    #[serde(default)]
    body: Option<Value>,
    /// Optional HTTP authentication, applied as an `Authorization` header
    /// (back-compat: new + optional). Bearer token or HTTP Basic.
    #[serde(default)]
    auth: Option<Auth>,
    #[serde(default = "default_timeout_ms")]
    timeout_ms: u64,
    #[serde(default = "default_max_bytes")]
    max_bytes: u64,
    /// 可选代理(http/https/socks5 URL)。代理 URL 与目标 URL 一样受网络能力
    /// 门禁约束(SSRF 不因代理放松);带 proxy/mtls 的步骤现建专属客户端,
    /// 不与 `http` 资源的共享客户端混用。
    #[serde(default)]
    proxy: Option<String>,
    /// 可选 mTLS 客户端证书(PEM 文件路径,读取前过 `fs.read` 门禁)。
    #[serde(default)]
    mtls: Option<Mtls>,
}

/// HTTP auth scheme for `http.request`. A tagged enum so the derived schema
/// carries the `kind: ["bearer","basic"]` constraint and the dispatch is
/// exhaustive. Secrets arrive pre-resolved as plain strings (same as the
/// existing email/transfer actions).
#[derive(Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "lowercase", deny_unknown_fields)]
enum Auth {
    /// `Authorization: Bearer <token>`.
    Bearer { token: String },
    /// `Authorization: Basic base64(user:pass)`.
    Basic { user: String, pass: String },
}

impl Auth {
    /// Apply this scheme to a request builder as an `Authorization` header.
    /// Shared by `http.request` and `http.paginate` so both treat `auth`
    /// identically (and stays exhaustive when new variants land).
    fn apply(&self, req: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        match self {
            Auth::Bearer { token } => req.bearer_auth(token),
            Auth::Basic { user, pass } => req.basic_auth(user, Some(pass)),
        }
    }
}
fn default_method() -> String {
    "GET".into()
}
fn default_timeout_ms() -> u64 {
    30_000
}
fn default_max_bytes() -> u64 {
    100 * 1024 * 1024 // 100 MiB
}

#[async_trait]
impl Action for RequestAction {
    fn id(&self) -> &'static str {
        "http.request"
    }
    fn summary(&self) -> &'static str {
        "Make an HTTP request and return status/body/headers"
    }
    fn schema(&self) -> &'static serde_json::Value {
        static SCHEMA: Lazy<Value> = Lazy::new(crate::schema::derive::<ReqIn>);
        &SCHEMA
    }
    async fn execute(&self, ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let ReqIn {
            method,
            url,
            headers,
            query,
            body,
            auth,
            timeout_ms,
            max_bytes,
            proxy,
            mtls,
        } = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("http.request input invalid: {e}")))?;
        ctx.ensure_network_url(&url)?;

        let client = client_for_with(ctx, timeout_ms, proxy.as_deref(), mtls.as_ref()).await?;

        let mut req = client
            .request(
                method
                    .parse()
                    .map_err(|e| StepError::msg(format!("bad method: {e}")))?,
                &url,
            )
            .query(&query);

        // Auth first, so an explicit `headers.Authorization` (if any) still wins.
        if let Some(auth) = &auth {
            req = auth.apply(req);
        }

        for (k, v) in &headers {
            req = req.header(k, v);
        }

        if let Some(body) = body {
            req = match body {
                Value::String(s) => req.body(s),
                other => req.json(&other),
            };
        }

        let resp = req.send().await.map_err(|e| {
            if e.is_redirect() {
                StepError::msg(
                    "http.request: blocked redirect to ungranted host (network capability)",
                )
            } else {
                // reqwest 的 Error::Display 会带上完整 URL(query 可能含凭据,如
                // ?api_key=...);without_url() 剥掉 URL,防凭据落 step 快照/日志。
                StepError::msg(format!("http send: {}", e.without_url()))
            }
        })?;
        let status = resp.status().as_u16();
        let resp_headers = headers_to_json(resp.headers());
        // 响应大小上限(F-11):Content-Length 预检挡掉声明超限的响应;读后再按
        // 实际字节兜底(chunked / 无 Content-Length 时预检看不到长度)。
        if let Some(len) = resp.content_length() {
            if len > max_bytes {
                return Err(StepError::msg(format!(
                    "http.request: response Content-Length {len} exceeds max_bytes {max_bytes}"
                )));
            }
        }
        let text = resp
            .text()
            .await
            .map_err(|e| StepError::msg(format!("http body: {}", e.without_url())))?;
        if text.len() as u64 > max_bytes {
            return Err(StepError::msg(format!(
                "http.request: response body {} bytes exceeds max_bytes {max_bytes}",
                text.len()
            )));
        }
        let body_json: Option<Value> = serde_json::from_str(&text).ok();

        Ok(ActionResult::from(serde_json::json!({
            "status": status,
            "headers": resp_headers,
            "text": text,
            "json": body_json,
        })))
    }
}

// ─── http.download ────────────────────────────────────────────────────────────

pub struct DownloadAction;

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct DownloadIn {
    url: String,
    dest: String,
    #[serde(default = "default_max_bytes")]
    max_bytes: u64,
    #[serde(default)]
    headers: HashMap<String, String>,
    #[serde(default = "default_timeout_ms")]
    timeout_ms: u64,
    /// 可选代理 / mTLS,语义同 `http.request`(代理 URL 过 SSRF 门禁,证书
    /// 路径过 `fs.read` 门禁,带任一者现建专属客户端)。
    #[serde(default)]
    proxy: Option<String>,
    #[serde(default)]
    mtls: Option<Mtls>,
}

#[async_trait]
impl Action for DownloadAction {
    fn id(&self) -> &'static str {
        "http.download"
    }
    fn summary(&self) -> &'static str {
        "Stream an HTTP GET response to a file, capped at max_bytes"
    }
    fn schema(&self) -> &'static serde_json::Value {
        static SCHEMA: Lazy<Value> = Lazy::new(crate::schema::derive::<DownloadIn>);
        &SCHEMA
    }
    async fn execute(&self, ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let DownloadIn {
            url,
            dest,
            max_bytes,
            headers,
            timeout_ms,
            proxy,
            mtls,
        } = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("http.download input invalid: {e}")))?;
        ctx.ensure_network_url(&url)?;
        let dest_path = PathBuf::from(&dest);
        ctx.ensure_fs_write(&dest_path)?;

        let client = client_for_with(ctx, timeout_ms, proxy.as_deref(), mtls.as_ref()).await?;
        let mut req = client.get(&url);
        for (k, v) in &headers {
            req = req.header(k, v);
        }
        let resp = req.send().await.map_err(|e| {
            if e.is_redirect() {
                StepError::msg(
                    "http.download: blocked redirect to ungranted host (network capability)",
                )
            } else {
                StepError::msg(format!("http.download send: {}", e.without_url()))
            }
        })?;
        let status = resp.status().as_u16();
        let content_type = resp
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string());

        // Cheap pre-check: refuse before opening the file if the server declares
        // a length over the cap — a rejected oversize download leaves no file.
        if let Some(len) = resp.content_length() {
            if len > max_bytes {
                return Err(StepError::msg(format!(
                    "http.download: Content-Length {len} exceeds max_bytes {max_bytes}"
                )));
            }
        }

        if let Some(parent) = dest_path.parent() {
            let _ = tokio::fs::create_dir_all(parent).await;
        }
        let mut file = tokio::fs::File::create(&dest_path).await.map_err(|e| {
            StepError::msg(format!("http.download create {}: {e}", dest_path.display()))
        })?;

        use futures::StreamExt;
        use tokio::io::AsyncWriteExt;
        let mut downloaded: u64 = 0;
        let mut stream = resp.bytes_stream();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|e| {
                StepError::msg(format!("http.download stream: {}", e.without_url()))
            })?;
            downloaded += chunk.len() as u64;
            // Streaming guard: also catches chunked / unknown-length responses
            // that the Content-Length pre-check can't see — delete the partial.
            if downloaded > max_bytes {
                drop(file);
                let _ = tokio::fs::remove_file(&dest_path).await;
                return Err(StepError::msg(format!(
                    "http.download: response exceeds max_bytes {max_bytes}"
                )));
            }
            file.write_all(&chunk)
                .await
                .map_err(|e| StepError::msg(format!("http.download write: {e}")))?;
        }
        file.flush()
            .await
            .map_err(|e| StepError::msg(format!("http.download flush: {e}")))?;

        Ok(ActionResult::from(serde_json::json!({
            "dest": dest,
            "bytes": downloaded,
            "status": status,
            "content_type": content_type,
        })))
    }
}

// ─── http.upload ──────────────────────────────────────────────────────────────

pub struct UploadAction;

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct UploadIn {
    url: String,
    src: String,
    mode: UploadMode,
    #[serde(default)]
    method: Option<String>,
    #[serde(default)]
    field: Option<String>,
    #[serde(default)]
    filename: Option<String>,
    #[serde(default)]
    headers: HashMap<String, String>,
    #[serde(default = "default_max_bytes")]
    max_bytes: u64,
    #[serde(default = "default_timeout_ms")]
    timeout_ms: u64,
    /// 可选代理 / mTLS,语义同 `http.request`。
    #[serde(default)]
    proxy: Option<String>,
    #[serde(default)]
    mtls: Option<Mtls>,
}

/// `http.upload` transport mode. Modeling it as an enum (not a free `String`)
/// keeps the `["multipart","body"]` constraint in the derived schema and makes
/// the dispatch `match` exhaustive.
#[derive(Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
enum UploadMode {
    Multipart,
    Body,
}

#[async_trait]
impl Action for UploadAction {
    fn id(&self) -> &'static str {
        "http.upload"
    }
    fn summary(&self) -> &'static str {
        "Upload a local file via multipart form or raw request body"
    }
    fn schema(&self) -> &'static serde_json::Value {
        static SCHEMA: Lazy<Value> = Lazy::new(crate::schema::derive::<UploadIn>);
        &SCHEMA
    }
    async fn execute(&self, ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let UploadIn {
            url,
            src,
            mode,
            method,
            field,
            filename,
            headers,
            max_bytes,
            timeout_ms,
            proxy,
            mtls,
        } = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("http.upload input invalid: {e}")))?;
        ctx.ensure_network_url(&url)?;
        let src_path = PathBuf::from(&src);
        ctx.ensure_fs_read(&src_path)?;

        // Cap the in-memory read by stat-ing first: prevents OOM from an
        // operator-/attacker-sized file before we ever allocate the buffer.
        let meta = tokio::fs::metadata(&src_path)
            .await
            .map_err(|e| StepError::msg(format!("http.upload stat {}: {e}", src_path.display())))?;
        if meta.len() > max_bytes {
            return Err(StepError::msg(format!(
                "http.upload: file size {} exceeds max_bytes {max_bytes}",
                meta.len()
            )));
        }
        let bytes = tokio::fs::read(&src_path)
            .await
            .map_err(|e| StepError::msg(format!("http.upload read {}: {e}", src_path.display())))?;

        // Reuse the SSRF-gated client: every redirect hop is re-authorized
        // against the network grants, so an upload can't be bounced to an
        // ungranted host (data exfiltration).
        let client = client_for_with(ctx, timeout_ms, proxy.as_deref(), mtls.as_ref()).await?;

        let resp = match mode {
            UploadMode::Multipart => {
                let field = field.unwrap_or_else(|| "file".into());
                let filename = filename.unwrap_or_else(|| {
                    src_path
                        .file_name()
                        .map(|s| s.to_string_lossy().to_string())
                        .unwrap_or_else(|| "file".into())
                });
                let part = reqwest::multipart::Part::bytes(bytes).file_name(filename);
                let form = reqwest::multipart::Form::new().part(field, part);
                let m = method.unwrap_or_else(|| "POST".into());
                let mut req = client
                    .request(
                        m.parse()
                            .map_err(|e| StepError::msg(format!("bad method: {e}")))?,
                        &url,
                    )
                    .multipart(form);
                for (k, v) in &headers {
                    req = req.header(k, v);
                }
                req.send().await.map_err(|e| {
                    if e.is_redirect() {
                        StepError::msg(
                            "http.upload: blocked redirect to ungranted host (network capability)",
                        )
                    } else {
                        StepError::msg(format!("http.upload send: {}", e.without_url()))
                    }
                })?
            }
            UploadMode::Body => {
                let m = method.unwrap_or_else(|| "PUT".into());
                let mut req = client
                    .request(
                        m.parse()
                            .map_err(|e| StepError::msg(format!("bad method: {e}")))?,
                        &url,
                    )
                    .body(bytes);
                for (k, v) in &headers {
                    req = req.header(k, v);
                }
                req.send().await.map_err(|e| {
                    if e.is_redirect() {
                        StepError::msg(
                            "http.upload: blocked redirect to ungranted host (network capability)",
                        )
                    } else {
                        StepError::msg(format!("http.upload send: {}", e.without_url()))
                    }
                })?
            }
        };

        let status = resp.status().as_u16();
        let resp_headers = headers_to_json(resp.headers());
        let text = resp
            .text()
            .await
            .map_err(|e| StepError::msg(format!("http.upload body: {}", e.without_url())))?;
        let body_json: Option<Value> = serde_json::from_str(&text).ok();

        Ok(ActionResult::from(serde_json::json!({
            "status": status,
            "headers": resp_headers,
            "text": text,
            "json": body_json,
        })))
    }
}

// ─── http.oauth2_token ─────────────────────────────────────────────────────────

pub struct OAuth2TokenAction;

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct OAuth2TokenIn {
    /// Token endpoint that issues the access token (the OAuth2 token URL).
    token_url: String,
    /// OAuth2 client id.
    client_id: String,
    /// OAuth2 client secret (arrives pre-resolved as a plain string).
    client_secret: String,
    /// Optional space-delimited scope string.
    #[serde(default)]
    scope: Option<String>,
    /// Optional `audience` parameter (some providers, e.g. Auth0, require it).
    #[serde(default)]
    audience: Option<String>,
    /// Where to put the client credentials: `basic` (default) sends them in an
    /// `Authorization: Basic` header; `body` puts them in the form body.
    #[serde(default)]
    client_auth: ClientAuth,
    #[serde(default = "default_timeout_ms")]
    timeout_ms: u64,
    #[serde(default = "default_max_bytes")]
    max_bytes: u64,
    /// 可选代理 / mTLS,语义同 `http.request`(mTLS 绑定的 token 端点见
    /// RFC 8705,client credentials 场景常见)。
    #[serde(default)]
    proxy: Option<String>,
    #[serde(default)]
    mtls: Option<Mtls>,
}

/// How `http.oauth2_token` presents the client credentials. An enum (not a free
/// `String`) so the derived schema keeps the `["basic","body"]` constraint and
/// the dispatch stays exhaustive.
#[derive(Deserialize, JsonSchema, Default)]
#[serde(rename_all = "lowercase")]
enum ClientAuth {
    /// Credentials in the `Authorization: Basic` header (the spec default).
    #[default]
    Basic,
    /// Credentials in the `application/x-www-form-urlencoded` body.
    Body,
}

#[async_trait]
impl Action for OAuth2TokenAction {
    fn id(&self) -> &'static str {
        "http.oauth2_token"
    }
    fn summary(&self) -> &'static str {
        "Fetch an OAuth2 access token via the client-credentials grant"
    }
    fn schema(&self) -> &'static serde_json::Value {
        static SCHEMA: Lazy<Value> = Lazy::new(crate::schema::derive::<OAuth2TokenIn>);
        &SCHEMA
    }
    async fn execute(&self, ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let OAuth2TokenIn {
            token_url,
            client_id,
            client_secret,
            scope,
            audience,
            client_auth,
            timeout_ms,
            max_bytes,
            proxy,
            mtls,
        } = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("http.oauth2_token input invalid: {e}")))?;
        ctx.ensure_network_url(&token_url)?;

        let client = client_for_with(ctx, timeout_ms, proxy.as_deref(), mtls.as_ref()).await?;

        // Build the form body: grant_type is fixed; scope/audience are optional.
        // For `client_auth: body` the credentials join the form; for `basic`
        // (default) they ride an Authorization: Basic header instead.
        let mut form: Vec<(&str, &str)> = vec![("grant_type", "client_credentials")];
        if let Some(scope) = &scope {
            form.push(("scope", scope));
        }
        if let Some(audience) = &audience {
            form.push(("audience", audience));
        }
        let mut req = client.post(&token_url);
        match client_auth {
            ClientAuth::Basic => {
                req = req.basic_auth(&client_id, Some(&client_secret));
            }
            ClientAuth::Body => {
                form.push(("client_id", &client_id));
                form.push(("client_secret", &client_secret));
            }
        }
        // `.form(..)` sets `Content-Type: application/x-www-form-urlencoded`.
        let req = req.form(&form);

        let resp = req.send().await.map_err(|e| {
            if e.is_redirect() {
                StepError::msg(
                    "http.oauth2_token: blocked redirect to ungranted host (network capability)",
                )
            } else {
                StepError::msg(format!("http.oauth2_token send: {}", e.without_url()))
            }
        })?;
        let status = resp.status();
        if let Some(len) = resp.content_length() {
            if len > max_bytes {
                return Err(StepError::msg(format!(
                    "http.oauth2_token: response Content-Length {len} exceeds max_bytes {max_bytes}"
                )));
            }
        }
        let text = resp
            .text()
            .await
            .map_err(|e| StepError::msg(format!("http.oauth2_token body: {}", e.without_url())))?;
        if text.len() as u64 > max_bytes {
            return Err(StepError::msg(format!(
                "http.oauth2_token: response body {} bytes exceeds max_bytes {max_bytes}",
                text.len()
            )));
        }
        if !status.is_success() {
            return Err(StepError::msg(format!(
                "http.oauth2_token: token endpoint returned {} ({})",
                status.as_u16(),
                text.chars().take(200).collect::<String>()
            )));
        }
        let raw: Value = serde_json::from_str(&text).map_err(|e| {
            StepError::msg(format!("http.oauth2_token: token response is not JSON: {e}"))
        })?;
        let access_token = raw.get("access_token").and_then(|v| v.as_str());
        let access_token = access_token.ok_or_else(|| {
            StepError::msg("http.oauth2_token: token response missing `access_token`")
        })?;

        // 输出只留白名单字段,不带 `raw`:token 响应可能附带 refresh_token /
        // id_token 等更长寿命的凭据,而步骤输出会经 record_step_output 持久化
        // 进运行历史库。expires_in 兼容数字与字符串两种 provider 习惯。
        Ok(ActionResult::from(serde_json::json!({
            "access_token": access_token,
            "token_type": raw.get("token_type").and_then(|v| v.as_str()),
            "expires_in": raw.get("expires_in").and_then(|v| {
                v.as_u64().or_else(|| v.as_str().and_then(|s| s.parse().ok()))
            }),
            "scope": raw.get("scope").and_then(|v| v.as_str()),
        })))
    }
}

// ─── http.paginate ─────────────────────────────────────────────────────────────

pub struct PaginateAction;

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct PaginateIn {
    /// First page URL.
    url: String,
    #[serde(default = "default_method")]
    method: String,
    #[serde(default)]
    headers: HashMap<String, String>,
    #[serde(default)]
    query: HashMap<String, String>,
    /// Optional HTTP authentication, applied to every page request (same shape
    /// as `http.request`).
    #[serde(default)]
    auth: Option<Auth>,
    /// JSON Pointer (RFC 6901, e.g. `/data/items`) to the array of items in each
    /// page body. Omit to treat the whole body as the items array.
    #[serde(default)]
    items_path: Option<String>,
    /// How to advance to the next page.
    pagination: Pagination,
    /// Hard cap on pages fetched. Hitting it sets `truncated: true` (no silent
    /// truncation).
    #[serde(default = "default_max_pages")]
    max_pages: u32,
    /// 聚合 items 的总条数上限(P2-2,默认 10_000)。`max_bytes` 只是**每页**
    /// 上限,乘上 `max_pages`(默认 50)后聚合体仍可能膨胀到 GB 级,这里再加
    /// 一道总量闸。命中后**停止翻页**并置 `truncated: true` —— 与 `max_pages`
    /// 命中同语义:截断是显式标记而非报错,因为调用方拿到的是完整页边界上的
    /// 部分数据,本身可用;这与单页超 `max_bytes` 直接报错不同(那是单个响应
    /// 异常,数据不可信)。不切割单页:最后一页整页计入,items 可略超此值。
    #[serde(default = "default_max_items")]
    max_items: u64,
    /// Per-page response body size cap.
    #[serde(default = "default_max_bytes")]
    max_bytes: u64,
    #[serde(default = "default_timeout_ms")]
    timeout_ms: u64,
    /// 可选代理 / mTLS,语义同 `http.request`(一个客户端打全部页)。
    #[serde(default)]
    proxy: Option<String>,
    #[serde(default)]
    mtls: Option<Mtls>,
}

/// `http.paginate` next-page strategy. A tagged enum so the derived schema keeps
/// the `style: ["next_url","page_number"]` constraint and the dispatch is
/// exhaustive. (cursor-token style is a future addition.)
#[derive(Deserialize, JsonSchema)]
#[serde(tag = "style", rename_all = "snake_case", deny_unknown_fields)]
enum Pagination {
    /// Follow a "next page URL" found in each response — either a JSON Pointer
    /// into the body (`next_path`) and/or the `Link: <…>; rel="next"` header.
    /// Stops when neither yields a URL.
    NextUrl {
        /// JSON Pointer to a string "next URL" field in the body (optional if
        /// you rely on the `Link` header).
        #[serde(default)]
        next_path: Option<String>,
        /// Also honor the `Link: rel="next"` header (default true).
        #[serde(default = "default_true")]
        use_link_header: bool,
    },
    /// Increment a `page` query param starting at `start` until a page yields no
    /// items (or `max_pages` is hit).
    PageNumber {
        /// Query parameter name to carry the page number (default `page`).
        #[serde(default = "default_page_param")]
        param: String,
        /// First page number (default 1).
        #[serde(default = "default_page_start")]
        start: u32,
    },
}

fn default_max_pages() -> u32 {
    50
}
fn default_max_items() -> u64 {
    10_000
}
fn default_true() -> bool {
    true
}
fn default_page_param() -> String {
    "page".into()
}
fn default_page_start() -> u32 {
    1
}

/// Pull the items array out of one page body via `items_path` (a JSON Pointer),
/// or the whole body when no path is given. A missing pointer / non-array yields
/// an empty slice so a page with no items simply contributes nothing.
fn page_items(body: &Value, items_path: &Option<String>) -> Vec<Value> {
    let node = match items_path {
        Some(ptr) => body.pointer(ptr),
        None => Some(body),
    };
    match node {
        Some(Value::Array(arr)) => arr.clone(),
        _ => Vec::new(),
    }
}

/// Extract the `rel="next"` target from an RFC 8288 `Link` header value, e.g.
/// `<https://api/x?page=2>; rel="next", <…>; rel="last"`.
fn link_header_next(link: &str) -> Option<String> {
    // 手扫而非 split(','):URL 里可以合法出现逗号,只认 `<…>` 之外的逗号为段界;
    // rel 按参数精确比对(值可以是引号包裹的空格分隔列表),`rel=next-archive`
    // 这类前缀串不会误中。
    let mut rest = link;
    while let Some(start) = rest.find('<') {
        let after = &rest[start + 1..];
        let end = after.find('>')?;
        let url = &after[..end];
        let tail = &after[end + 1..];
        let seg_end = tail.find(',').unwrap_or(tail.len());
        let is_next = tail[..seg_end].split(';').any(|p| {
            let p = p.trim().to_ascii_lowercase();
            p.strip_prefix("rel=")
                .map(|v| v.trim_matches('"').split_whitespace().any(|w| w == "next"))
                .unwrap_or(false)
        });
        if is_next {
            return Some(url.to_string());
        }
        rest = tail[seg_end..].strip_prefix(',').unwrap_or(&tail[seg_end..]);
    }
    None
}

#[async_trait]
impl Action for PaginateAction {
    fn id(&self) -> &'static str {
        "http.paginate"
    }
    fn summary(&self) -> &'static str {
        "Fetch and aggregate items across paginated HTTP responses"
    }
    fn schema(&self) -> &'static serde_json::Value {
        static SCHEMA: Lazy<Value> = Lazy::new(crate::schema::derive::<PaginateIn>);
        &SCHEMA
    }
    async fn execute(&self, ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let PaginateIn {
            url,
            method,
            headers,
            query,
            auth,
            items_path,
            pagination,
            max_pages,
            max_items,
            max_bytes,
            timeout_ms,
            proxy,
            mtls,
        } = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("http.paginate input invalid: {e}")))?;

        let http_method: reqwest::Method = method
            .parse()
            .map_err(|e| StepError::msg(format!("http.paginate bad method: {e}")))?;
        let client = client_for_with(ctx, timeout_ms, proxy.as_deref(), mtls.as_ref()).await?;

        let mut items: Vec<Value> = Vec::new();
        let mut pages: u32 = 0;
        let mut truncated = false;
        // For next-url: the absolute URL of the next page (None ⇒ done).
        let mut next_url: Option<String> = Some(url.clone());
        // For page-number: the current page index.
        let (page_param, mut page_num) = match &pagination {
            Pagination::PageNumber { param, start } => (Some(param.clone()), *start),
            Pagination::NextUrl { .. } => (None, 0),
        };

        loop {
            if pages >= max_pages {
                // Cap hit AND there is (or may be) more to fetch ⇒ truncated.
                truncated = true;
                break;
            }
            if items.len() as u64 >= max_items {
                // 总量闸(P2-2):聚合条数已达上限且(可能)还有下一页 ⇒ 截断。
                // 与 max_pages 同放在循环顶部:若上一页恰好是自然末页,循环在
                // 上一轮尾部就 break 了,不会走到这里被误标 truncated。
                truncated = true;
                break;
            }

            // Resolve the URL for this page.
            let page_url = match (&pagination, &next_url) {
                (Pagination::NextUrl { .. }, Some(u)) => u.clone(),
                (Pagination::NextUrl { .. }, None) => break,
                (Pagination::PageNumber { .. }, _) => url.clone(),
            };
            // Gate EVERY page URL — a next-url/redirect to an ungranted host is
            // blocked here before any socket is opened.
            ctx.ensure_network_url(&page_url)?;

            let mut req = client.request(http_method.clone(), &page_url);
            // next_url 模式下 next 链接通常已自带完整 query,再附加初始 query 会
            // 产生重复参数(部分 API 报 400)—— 初始 query 只用于第一页。
            // page_number 模式每页都打同一基础 URL,query 每页保留。
            match &pagination {
                Pagination::NextUrl { .. } if pages > 0 => {}
                _ => req = req.query(&query),
            }
            if let Pagination::PageNumber { .. } = pagination {
                if let Some(param) = &page_param {
                    req = req.query(&[(param.as_str(), page_num.to_string())]);
                }
            }
            if let Some(auth) = &auth {
                req = auth.apply(req);
            }
            for (k, v) in &headers {
                req = req.header(k, v);
            }

            let resp = req.send().await.map_err(|e| {
                if e.is_redirect() {
                    StepError::msg(
                        "http.paginate: blocked redirect to ungranted host (network capability)",
                    )
                } else {
                    StepError::msg(format!("http.paginate send: {}", e.without_url()))
                }
            })?;

            // 非 2xx 即失败:错误页(429/5xx)的 body 取不出 items 会被误判成
            // 「分页自然结束」,静默返回部分数据 —— 必须显式报错。
            let status = resp.status();
            if !status.is_success() {
                return Err(StepError::msg(format!(
                    "http.paginate: page {} returned HTTP {status}",
                    pages + 1
                )));
            }

            // Grab the next-url Link header before consuming the body.
            let link_next = resp
                .headers()
                .get(reqwest::header::LINK)
                .and_then(|v| v.to_str().ok())
                .and_then(link_header_next);

            if let Some(len) = resp.content_length() {
                if len > max_bytes {
                    return Err(StepError::msg(format!(
                        "http.paginate: page Content-Length {len} exceeds max_bytes {max_bytes}"
                    )));
                }
            }
            let text = resp
                .text()
                .await
                .map_err(|e| StepError::msg(format!("http.paginate body: {}", e.without_url())))?;
            if text.len() as u64 > max_bytes {
                return Err(StepError::msg(format!(
                    "http.paginate: page body {} bytes exceeds max_bytes {max_bytes}",
                    text.len()
                )));
            }
            let body: Value = serde_json::from_str(&text)
                .map_err(|e| StepError::msg(format!("http.paginate: page is not JSON: {e}")))?;

            let page_batch = page_items(&body, &items_path);
            let batch_len = page_batch.len();
            items.extend(page_batch);
            pages += 1;

            // Decide whether another page exists.
            match &pagination {
                Pagination::NextUrl {
                    next_path,
                    use_link_header,
                } => {
                    let body_next = next_path
                        .as_deref()
                        .and_then(|ptr| body.pointer(ptr))
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string());
                    let header_next = if *use_link_header { link_next } else { None };
                    next_url = body_next.or(header_next);
                    if next_url.is_none() {
                        break;
                    }
                }
                Pagination::PageNumber { .. } => {
                    // Stop on the first empty page; otherwise advance.
                    if batch_len == 0 {
                        break;
                    }
                    page_num += 1;
                }
            }
        }

        Ok(ActionResult::from(serde_json::json!({
            "items": items,
            "pages": pages,
            "truncated": truncated,
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
            "run-http".into(),
            "flow-http".into(),
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
    fn headers_to_json_keeps_singles_and_aggregates_repeats() {
        // P2-3:单值头 → 字符串(形状不变,向后兼容),重复头 → 按序数组;
        // 第三个值走「追加进已有数组」分支,这里一并覆盖。
        use reqwest::header::{HeaderMap, HeaderValue};
        let mut h = HeaderMap::new();
        h.insert("x-one", HeaderValue::from_static("a"));
        h.append("set-cookie", HeaderValue::from_static("a=1"));
        h.append("set-cookie", HeaderValue::from_static("b=2"));
        h.append("set-cookie", HeaderValue::from_static("c=3"));
        let v = headers_to_json(&h);
        assert_eq!(v["x-one"], serde_json::json!("a"));
        assert_eq!(v["set-cookie"], serde_json::json!(["a=1", "b=2", "c=3"]));
    }

    #[test]
    fn link_header_next_matches_exact_rel_only() {
        // `rel=next-archive` 不是 next;带引号/不带引号/rel 值列表都要认。
        assert_eq!(
            link_header_next(r#"<https://a/2>; rel="next", <https://a/9>; rel="last""#).as_deref(),
            Some("https://a/2")
        );
        assert_eq!(
            link_header_next("<https://a/2>; rel=next").as_deref(),
            Some("https://a/2")
        );
        assert_eq!(
            link_header_next(r#"<https://a/2>; rel="prev next""#).as_deref(),
            Some("https://a/2")
        );
        assert_eq!(
            link_header_next(r#"<https://a/x>; rel="next-archive""#),
            None
        );
        assert_eq!(link_header_next(r#"<https://a/x>; rel="prev""#), None);
    }

    #[test]
    fn link_header_next_survives_commas_inside_urls() {
        // URL 里合法出现的逗号不能被当成段界截断。
        assert_eq!(
            link_header_next(r#"<https://a/p?ids=1,2,3&page=2>; rel="next""#).as_deref(),
            Some("https://a/p?ids=1,2,3&page=2")
        );
        assert_eq!(
            link_header_next(
                r#"<https://a/p?ids=1,2>; rel="prev", <https://a/p?ids=1,2&page=3>; rel="next""#
            )
            .as_deref(),
            Some("https://a/p?ids=1,2&page=3")
        );
    }

    #[test]
    fn http_slot_selects_only_http_kind_bindings() {
        let resources = &[
            ("api", "kind: http\ntimeout_ms: 5000\n"),
            ("db", "kind: sqlite\npath: /tmp/x.db\n"),
        ];
        assert_eq!(
            http_slot(&ctx_with(resources, Some("api"))).as_deref(),
            Some("api")
        );
        // Unbound / non-http / undeclared ⇒ None ⇒ per-call client (back-compat).
        assert_eq!(http_slot(&ctx_with(resources, None)), None);
        assert_eq!(http_slot(&ctx_with(resources, Some("db"))), None);
        assert_eq!(http_slot(&ctx_with(resources, Some("ghost"))), None);
    }

    #[test]
    fn http_timeout_from_decl_reads_or_defaults() {
        assert_eq!(
            http_timeout_from_decl(&decl("kind: http\ntimeout_ms: 5000\n")),
            5000
        );
        // Omitted ⇒ the same default a per-step request uses.
        assert_eq!(
            http_timeout_from_decl(&decl("kind: http\n")),
            default_timeout_ms()
        );
    }

    #[test]
    fn http_factory_kind_matches() {
        assert_eq!(HttpFactory.kind(), HTTP_KIND);
        assert_eq!(HTTP_KIND, "http");
    }

    // Building a `reqwest::Client` is lazy (no socket until a request), so the
    // reuse + teardown contract is testable without a server.
    #[tokio::test]
    async fn http_resource_opens_one_client_then_reuses_it() {
        let run = "http-reuse-unit";
        assert!(!http_client_open(run));
        let c1 = ensure_client(run, "api", &[], 1_000, None).await.unwrap();
        // A second open of the same slot reuses the first client (later args ignored).
        let c2 = ensure_client(run, "api", &[], 9_999, None).await.unwrap();
        assert!(Arc::ptr_eq(&c1, &c2), "second ensure reuses the cached client");
        assert!(http_client_open(run));
        // Teardown drops it; idempotent.
        close_run_clients(run);
        assert!(!http_client_open(run));
        close_run_clients(run); // no-op, must not panic
    }

    #[test]
    fn http_proxy_from_decl_reads_optional_proxy() {
        assert_eq!(
            http_proxy_from_decl(&decl("kind: http\nproxy: http://127.0.0.1:3128\n")).as_deref(),
            Some("http://127.0.0.1:3128")
        );
        assert_eq!(http_proxy_from_decl(&decl("kind: http\n")), None);
    }

    #[test]
    fn gated_client_rejects_invalid_proxy_url() {
        // 非法代理 URL(解析不了 / scheme 不被支持)必须在客户端构造处显式报错,
        // 不能静默忽略后直连 —— 直连会绕开操作者预期的出口路径。
        for bad in ["not a proxy", "ftp://127.0.0.1:21"] {
            let err = build_gated_client_with(&[], 1_000, Some(bad), None).unwrap_err();
            assert!(err.to_string().contains("proxy"), "got: {err}");
        }
    }
}
