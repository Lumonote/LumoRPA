//! HTTP request action.

use async_trait::async_trait;
use lumo_core::error::StepError;
use lumo_core::{Action, ActionRegistry, ActionResult, StepCtx};
use once_cell::sync::Lazy;
use serde::Deserialize;
use serde_json::Value;
use std::collections::HashMap;
use std::path::PathBuf;

pub fn register(r: &mut ActionRegistry) {
    r.register(RequestAction);
    r.register(DownloadAction);
    r.register(UploadAction);
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
    let grants = grants.to_vec(); // owned, moved into the closure
    reqwest::Client::builder()
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
        }))
        .build()
        .map_err(|e| StepError::msg(format!("http client: {e}")))
}

pub struct RequestAction;

#[derive(Deserialize)]
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
    #[serde(default = "default_timeout_ms")]
    timeout_ms: u64,
    #[serde(default = "default_max_bytes")]
    max_bytes: u64,
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
        static SCHEMA: Lazy<Value> = Lazy::new(|| {
            serde_json::json!({
                "type": "object",
                "required": ["url"],
                "properties": {
                    "method": { "type": "string" },
                    "url": { "type": "string" },
                    "headers": { "type": "object" },
                    "query": { "type": "object" },
                    "body": {},
                    "timeout_ms": { "type": "integer" },
                    "max_bytes": { "type": "integer" }
                },
                "additionalProperties": false
            })
        });
        &SCHEMA
    }
    async fn execute(&self, ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let ReqIn {
            method,
            url,
            headers,
            query,
            body,
            timeout_ms,
            max_bytes,
        } = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("http.request input invalid: {e}")))?;
        ctx.ensure_network_url(&url)?;

        let client = build_gated_client(ctx.network_grants(), timeout_ms)?;

        let mut req = client
            .request(
                method
                    .parse()
                    .map_err(|e| StepError::msg(format!("bad method: {e}")))?,
                &url,
            )
            .query(&query);

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
                StepError::msg(format!("http send: {e}"))
            }
        })?;
        let status = resp.status().as_u16();
        let resp_headers: HashMap<_, _> = resp
            .headers()
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_str().unwrap_or("").to_string()))
            .collect();
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
            .map_err(|e| StepError::msg(format!("http body: {e}")))?;
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

#[derive(Deserialize)]
struct DownloadIn {
    url: String,
    dest: String,
    #[serde(default = "default_max_bytes")]
    max_bytes: u64,
    #[serde(default)]
    headers: HashMap<String, String>,
    #[serde(default = "default_timeout_ms")]
    timeout_ms: u64,
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
        static SCHEMA: Lazy<Value> = Lazy::new(|| {
            serde_json::json!({
                "type": "object",
                "required": ["url", "dest"],
                "properties": {
                    "url": { "type": "string" },
                    "dest": { "type": "string" },
                    "max_bytes": { "type": "integer" },
                    "headers": { "type": "object" },
                    "timeout_ms": { "type": "integer" }
                },
                "additionalProperties": false
            })
        });
        &SCHEMA
    }
    async fn execute(&self, ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let DownloadIn {
            url,
            dest,
            max_bytes,
            headers,
            timeout_ms,
        } = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("http.download input invalid: {e}")))?;
        ctx.ensure_network_url(&url)?;
        let dest_path = PathBuf::from(&dest);
        ctx.ensure_fs_write(&dest_path)?;

        let client = build_gated_client(ctx.network_grants(), timeout_ms)?;
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
                StepError::msg(format!("http.download send: {e}"))
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
            let chunk = chunk.map_err(|e| StepError::msg(format!("http.download stream: {e}")))?;
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

#[derive(Deserialize)]
struct UploadIn {
    url: String,
    src: String,
    mode: String,
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
        static SCHEMA: Lazy<Value> = Lazy::new(|| {
            serde_json::json!({
                "type": "object",
                "required": ["url", "src", "mode"],
                "properties": {
                    "url": { "type": "string" },
                    "src": { "type": "string" },
                    "mode": { "type": "string", "enum": ["multipart", "body"] },
                    "method": { "type": "string" },
                    "field": { "type": "string" },
                    "filename": { "type": "string" },
                    "headers": { "type": "object" },
                    "max_bytes": { "type": "integer" },
                    "timeout_ms": { "type": "integer" }
                },
                "additionalProperties": false
            })
        });
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
        let client = build_gated_client(ctx.network_grants(), timeout_ms)?;

        let resp = match mode.as_str() {
            "multipart" => {
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
                        StepError::msg(format!("http.upload send: {e}"))
                    }
                })?
            }
            "body" => {
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
                        StepError::msg(format!("http.upload send: {e}"))
                    }
                })?
            }
            other => {
                return Err(StepError::msg(format!(
                    "http.upload: mode must be `multipart` or `body`, got `{other}`"
                )))
            }
        };

        let status = resp.status().as_u16();
        let resp_headers: HashMap<_, _> = resp
            .headers()
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_str().unwrap_or("").to_string()))
            .collect();
        let text = resp
            .text()
            .await
            .map_err(|e| StepError::msg(format!("http.upload body: {e}")))?;
        let body_json: Option<Value> = serde_json::from_str(&text).ok();

        Ok(ActionResult::from(serde_json::json!({
            "status": status,
            "headers": resp_headers,
            "text": text,
            "json": body_json,
        })))
    }
}
