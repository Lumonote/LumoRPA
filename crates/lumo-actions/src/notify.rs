//! Notification action — `notify.send` (S-class F-8).
//!
//! One unified action over four providers (DingTalk / Feishu / WeCom / generic
//! webhook). DingTalk and Feishu support HMAC-SHA256 request signing; the
//! `secret` arrives already resolved from `${{ vault.* }}` (P1-3), so it never
//! touches argv or run snapshots. A non-2xx HTTP status or a provider error code
//! fails the step so flows surface delivery failures instead of swallowing them.

use async_trait::async_trait;
use base64::Engine;
use hmac::{Hmac, Mac};
use lumo_core::error::StepError;
use lumo_core::{Action, ActionRegistry, ActionResult, StepCtx};
use once_cell::sync::Lazy;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::Value;
use sha2::Sha256;

type HmacSha256 = Hmac<Sha256>;

pub fn register(r: &mut ActionRegistry) {
    r.register(SendAction);
    r.register(DingtalkAction);
    r.register(FeishuAction);
    r.register(WecomAction);
}

pub struct SendAction;

#[derive(Deserialize)]
struct SendIn {
    provider: String,
    url: String,
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    payload: Option<Value>,
    #[serde(default)]
    title: Option<String>,
    #[serde(default = "default_msgtype")]
    msgtype: String,
    #[serde(default)]
    secret: Option<String>,
    #[serde(default = "default_timeout_ms")]
    timeout_ms: u64,
}
fn default_msgtype() -> String {
    "text".into()
}
fn default_timeout_ms() -> u64 {
    30_000
}

fn now_ms() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn dingtalk_body(text: &str, title: Option<&str>, msgtype: &str) -> Value {
    if msgtype == "markdown" {
        serde_json::json!({
            "msgtype": "markdown",
            "markdown": { "title": title.unwrap_or("notification"), "text": text }
        })
    } else {
        serde_json::json!({ "msgtype": "text", "text": { "content": text } })
    }
}
fn feishu_body(text: &str) -> Value {
    serde_json::json!({ "msg_type": "text", "content": { "text": text } })
}
fn wecom_body(text: &str, msgtype: &str) -> Value {
    if msgtype == "markdown" {
        serde_json::json!({ "msgtype": "markdown", "markdown": { "content": text } })
    } else {
        serde_json::json!({ "msgtype": "text", "text": { "content": text } })
    }
}

/// DingTalk: `sign = base64(HMAC_SHA256(key=secret, msg="{ts}\n{secret}"))`.
fn dingtalk_sign(ts: u64, secret: &str) -> String {
    let string_to_sign = format!("{ts}\n{secret}");
    let mut mac =
        HmacSha256::new_from_slice(secret.as_bytes()).expect("HMAC accepts any key length");
    mac.update(string_to_sign.as_bytes());
    base64::engine::general_purpose::STANDARD.encode(mac.finalize().into_bytes())
}

/// Feishu: `sign = base64(HMAC_SHA256(key="{ts_s}\n{secret}", msg=""))`.
fn feishu_sign(ts_s: u64, secret: &str) -> String {
    let key = format!("{ts_s}\n{secret}");
    let mut mac = HmacSha256::new_from_slice(key.as_bytes()).expect("HMAC accepts any key length");
    mac.update(b"");
    base64::engine::general_purpose::STANDARD.encode(mac.finalize().into_bytes())
}

/// Compose `(final_url, body)` for a provider, applying signing when a secret is
/// present. A caller-supplied `payload` is sent verbatim (advanced escape hatch).
fn build_request(
    provider: &str,
    url: &str,
    text: Option<&str>,
    payload: Option<Value>,
    title: Option<&str>,
    msgtype: &str,
    secret: Option<&str>,
) -> Result<(String, Value), StepError> {
    let base_body = match provider {
        "dingtalk" => payload.unwrap_or_else(|| dingtalk_body(text.unwrap_or(""), title, msgtype)),
        "feishu" => payload.unwrap_or_else(|| feishu_body(text.unwrap_or(""))),
        "wecom" => payload.unwrap_or_else(|| wecom_body(text.unwrap_or(""), msgtype)),
        "webhook" => payload.unwrap_or_else(|| serde_json::json!({ "text": text.unwrap_or("") })),
        other => {
            return Err(StepError::msg(format!(
                "notify.send: unknown provider `{other}`"
            )))
        }
    };

    match provider {
        "dingtalk" => {
            if let Some(secret) = secret {
                let ts = now_ms();
                let sign = dingtalk_sign(ts, secret);
                let mut u = reqwest::Url::parse(url)
                    .map_err(|e| StepError::msg(format!("notify.send bad url: {e}")))?;
                u.query_pairs_mut()
                    .append_pair("timestamp", &ts.to_string())
                    .append_pair("sign", &sign);
                Ok((u.to_string(), base_body))
            } else {
                Ok((url.to_string(), base_body))
            }
        }
        "feishu" => {
            if let Some(secret) = secret {
                let ts_s = now_ms() / 1000;
                let sign = feishu_sign(ts_s, secret);
                let mut body = base_body;
                if let Value::Object(m) = &mut body {
                    m.insert("timestamp".into(), Value::String(ts_s.to_string()));
                    m.insert("sign".into(), Value::String(sign));
                } else {
                    // 非 object payload 无处插入 timestamp/sign:与其静默发出未签名
                    // 请求(飞书侧 401、本地无提示),不如显式报错暴露配置问题。
                    return Err(StepError::msg(
                        "notify.send feishu: signing requires a JSON object payload \
                         (cannot add timestamp/sign to a non-object body)",
                    ));
                }
                Ok((url.to_string(), body))
            } else {
                Ok((url.to_string(), base_body))
            }
        }
        _ => Ok((url.to_string(), base_body)),
    }
}

/// Provider-level success: DingTalk/WeCom use `errcode`, Feishu uses `code`
/// (older webhooks `StatusCode`). Absent ⇒ assume success (rely on HTTP status).
fn provider_success(provider: &str, response: &Value) -> bool {
    match provider {
        "dingtalk" | "wecom" => response
            .get("errcode")
            .and_then(Value::as_i64)
            .map(|c| c == 0)
            .unwrap_or(true),
        "feishu" => {
            if let Some(c) = response.get("code").and_then(Value::as_i64) {
                return c == 0;
            }
            if let Some(c) = response.get("StatusCode").and_then(Value::as_i64) {
                return c == 0;
            }
            true
        }
        _ => true,
    }
}

#[async_trait]
impl Action for SendAction {
    fn id(&self) -> &'static str {
        "notify.send"
    }
    fn summary(&self) -> &'static str {
        "Send a notification (dingtalk/feishu/wecom/webhook), with optional HMAC signing"
    }
    fn schema(&self) -> &'static Value {
        static SCHEMA: Lazy<Value> = Lazy::new(|| {
            serde_json::json!({
                "type": "object",
                "required": ["provider", "url"],
                "properties": {
                    "provider": { "type": "string", "enum": ["dingtalk", "feishu", "wecom", "webhook"] },
                    "url": { "type": "string" },
                    "text": { "type": "string" },
                    "payload": {},
                    "title": { "type": "string" },
                    "msgtype": { "type": "string", "enum": ["text", "markdown"] },
                    "secret": { "type": "string" },
                    "timeout_ms": { "type": "integer" }
                },
                "additionalProperties": false
            })
        });
        &SCHEMA
    }
    async fn execute(&self, ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        send_value(ctx, "notify.send", input).await
    }
}

/// `notify.send` 的发送内核，供 `human.approve` 的 `notify` 字段复用：输入与
/// `notify.send` 同形（provider/url/text/payload/...），走同一网络门禁与投递
/// 路径。`label` 用于错误信息归属（`notify.send` / `human.approve.notify`）。
pub(crate) async fn send_value(
    ctx: &StepCtx,
    label: &str,
    input: Value,
) -> Result<ActionResult, StepError> {
    let SendIn {
        provider,
        url,
        text,
        payload,
        title,
        msgtype,
        secret,
        timeout_ms,
    } = serde_json::from_value(input)
        .map_err(|e| StepError::msg(format!("{label} input invalid: {e}")))?;
    ctx.ensure_network_url(&url)?;
    if text.is_none() && payload.is_none() {
        return Err(StepError::msg(format!("{label} requires `text` or `payload`")));
    }
    deliver(
        ctx, &provider, &url, text, payload, title, &msgtype, secret, timeout_ms,
    )
    .await
}

/// Shared delivery path for `notify.send` and the per-platform robot actions:
/// build (and sign) the request, POST it through the SSRF-gated client, and map
/// HTTP/provider failures to a step error. Caller must have run the network gate
/// and the `text`/`payload` presence check already.
#[allow(clippy::too_many_arguments)]
async fn deliver(
    ctx: &StepCtx,
    provider: &str,
    url: &str,
    text: Option<String>,
    payload: Option<Value>,
    title: Option<String>,
    msgtype: &str,
    secret: Option<String>,
    timeout_ms: u64,
) -> Result<ActionResult, StepError> {
    let (final_url, body) = build_request(
        provider,
        url,
        text.as_deref(),
        payload,
        title.as_deref(),
        msgtype,
        secret.as_deref(),
    )?;

    // 复用 http 模块的 SSRF 网关:逐跳重定向重新鉴权,防止授权 host 302
    // 跳到未授权内网地址(如 169.254.169.254 云元数据)绕过网络沙箱。
    let client = crate::http::build_gated_client(ctx.network_grants(), timeout_ms)?;
    let resp = client
        .post(&final_url)
        .json(&body)
        .send()
        .await
        .map_err(|e| {
            if e.is_redirect() {
                StepError::msg(
                    "notify: blocked redirect to ungranted host (network capability)",
                )
            } else {
                // reqwest 的 Error::Display 会带上完整 URL(含 query);dingtalk 带
                // secret 时 URL 含 ?sign=<HMAC>。without_url() 剥掉 URL,防签名落日志/快照。
                StepError::msg(format!("notify send: {}", e.without_url()))
            }
        })?;
    let status = resp.status().as_u16();
    let text_resp = resp
        .text()
        .await
        .map_err(|e| StepError::msg(format!("notify body: {}", e.without_url())))?;
    let response: Value =
        serde_json::from_str(&text_resp).unwrap_or(Value::String(text_resp.clone()));

    let ok = (200..300).contains(&status) && provider_success(provider, &response);
    if !ok {
        return Err(StepError::msg(format!(
            "notify `{provider}` failed: status={status} response={response}"
        )));
    }

    Ok(ActionResult::from(serde_json::json!({
        "status": status,
        "ok": ok,
        "response": response,
    })))
}

/// Shared input for the per-platform robot actions (`notify.dingtalk` /
/// `notify.feishu` / `notify.wecom`): same fields as `notify.send` minus the
/// `provider` discriminator (fixed by the action id). Secrets arrive pre-resolved
/// as plaintext, exactly like `notify.send`.
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct RobotIn {
    url: String,
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    payload: Option<Value>,
    #[serde(default)]
    title: Option<String>,
    #[serde(default = "default_msgtype")]
    msgtype: String,
    #[serde(default)]
    secret: Option<String>,
    #[serde(default = "default_timeout_ms")]
    timeout_ms: u64,
}

async fn robot_execute(
    ctx: &mut StepCtx,
    action: &str,
    provider: &str,
    input: Value,
) -> Result<ActionResult, StepError> {
    let RobotIn {
        url,
        text,
        payload,
        title,
        msgtype,
        secret,
        timeout_ms,
    } = serde_json::from_value(input)
        .map_err(|e| StepError::msg(format!("{action} input invalid: {e}")))?;
    ctx.ensure_network_url(&url)?;
    if text.is_none() && payload.is_none() {
        return Err(StepError::msg(format!("{action} requires `text` or `payload`")));
    }
    deliver(
        ctx, provider, &url, text, payload, title, &msgtype, secret, timeout_ms,
    )
    .await
}

pub struct DingtalkAction;
#[async_trait]
impl Action for DingtalkAction {
    fn id(&self) -> &'static str {
        "notify.dingtalk"
    }
    fn summary(&self) -> &'static str {
        "Send a text/markdown message to a DingTalk group robot webhook (optional HMAC signing)"
    }
    fn schema(&self) -> &'static Value {
        static S: Lazy<Value> = Lazy::new(crate::schema::derive::<RobotIn>);
        &S
    }
    async fn execute(&self, ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        robot_execute(ctx, "notify.dingtalk", "dingtalk", input).await
    }
}

pub struct FeishuAction;
#[async_trait]
impl Action for FeishuAction {
    fn id(&self) -> &'static str {
        "notify.feishu"
    }
    fn summary(&self) -> &'static str {
        "Send a text message to a Feishu group robot webhook (optional HMAC signing)"
    }
    fn schema(&self) -> &'static Value {
        static S: Lazy<Value> = Lazy::new(crate::schema::derive::<RobotIn>);
        &S
    }
    async fn execute(&self, ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        robot_execute(ctx, "notify.feishu", "feishu", input).await
    }
}

pub struct WecomAction;
#[async_trait]
impl Action for WecomAction {
    fn id(&self) -> &'static str {
        "notify.wecom"
    }
    fn summary(&self) -> &'static str {
        "Send a text/markdown message to a WeCom group robot webhook"
    }
    fn schema(&self) -> &'static Value {
        static S: Lazy<Value> = Lazy::new(crate::schema::derive::<RobotIn>);
        &S
    }
    async fn execute(&self, ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        robot_execute(ctx, "notify.wecom", "wecom", input).await
    }
}
