//! LLM provider trait + concrete adapters (OpenAI-compatible / Anthropic).

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::time::Duration;
use thiserror::Error;

use crate::config::ProviderProfile;

#[derive(Debug, Error)]
pub enum ProviderError {
    #[error("network is disabled (set LUMO_ALLOW_LLM_NETWORK=1 to opt in)")]
    NetworkDisabled,
    #[error("missing api key for provider `{0}`")]
    MissingApiKey(String),
    #[error("http: {0}")]
    Http(String),
    #[error("api error ({status}): {body}")]
    Api { status: u16, body: String },
    #[error("provider error: {0}")]
    Other(String),
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    System,
    User,
    Assistant,
    Tool,
}

impl Role {
    fn as_str(&self) -> &'static str {
        match self {
            Role::System => "system",
            Role::User => "user",
            Role::Assistant => "assistant",
            Role::Tool => "tool",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: Role,
    pub content: String,
    /// Optional image attachments — sent as additional content blocks to
    /// providers that support multimodal input (Anthropic vision, OpenAI
    /// `image_url`). Ignored by text-only providers (S-11/S-12).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attachments: Vec<ImageAttachment>,
}

/// One image attached to a chat message.
///
/// `data` is either a base64-encoded image (`is_url = false`) or a remote URL
/// (`is_url = true`). The provider chooses the right wire encoding.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImageAttachment {
    pub media_type: String,
    pub data: String,
    #[serde(default)]
    pub is_url: bool,
}

impl ImageAttachment {
    pub fn base64(media_type: impl Into<String>, data: impl Into<String>) -> Self {
        Self {
            media_type: media_type.into(),
            data: data.into(),
            is_url: false,
        }
    }
    pub fn url(media_type: impl Into<String>, url: impl Into<String>) -> Self {
        Self {
            media_type: media_type.into(),
            data: url.into(),
            is_url: true,
        }
    }
}

impl ChatMessage {
    /// Text-only message constructor — the common path for non-vision callers.
    /// Equivalent to `ChatMessage { role, content, attachments: Vec::new() }`.
    pub fn text(role: Role, content: impl Into<String>) -> Self {
        Self {
            role,
            content: content.into(),
            attachments: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatRequest {
    pub model: String,
    pub messages: Vec<ChatMessage>,
    #[serde(default)]
    pub temperature: Option<f32>,
    #[serde(default)]
    pub max_tokens: Option<u32>,
    #[serde(default)]
    pub system: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatResponse {
    pub content: String,
    pub model: String,
    pub provider: String,
    #[serde(default)]
    pub input_tokens: u32,
    #[serde(default)]
    pub output_tokens: u32,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum ProviderId {
    Anthropic,
    OpenaiCompat,
}

#[async_trait]
pub trait LlmProvider: Send + Sync {
    fn name(&self) -> &str;
    fn id(&self) -> ProviderId;
    fn supports(&self, model: &str) -> bool {
        model.starts_with(&format!("{}/", self.name()))
    }
    fn upstream_model(&self, requested: &str) -> String {
        let prefix = format!("{}/", self.name());
        requested
            .strip_prefix(&prefix)
            .unwrap_or(requested)
            .to_string()
    }
    async fn chat(&self, req: ChatRequest) -> Result<ChatResponse, ProviderError>;
}

// ─── Network gating ─────────────────────────────────────────────────────────

fn network_allowed() -> bool {
    matches!(
        std::env::var("LUMO_ALLOW_LLM_NETWORK").as_deref(),
        Ok("1") | Ok("true") | Ok("TRUE") | Ok("yes")
    )
}

fn http_client(timeout_ms: u64) -> Result<reqwest::Client, ProviderError> {
    reqwest::Client::builder()
        .timeout(Duration::from_millis(timeout_ms))
        .user_agent(concat!("lumorpa/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|e| ProviderError::Http(e.to_string()))
}

// ─── Transient-error retry ───────────────────────────────────────────────────
//
// Bounded exponential backoff applied to provider POSTs. Only HTTP 429 and 5xx
// responses, plus transient reqwest send errors, are retried; all other 4xx
// responses (and decode failures) are terminal. Up to `MAX_ATTEMPTS` attempts
// total with sleeps of 200ms, 400ms, ... between them.

const MAX_ATTEMPTS: u32 = 3;

/// True for HTTP statuses we consider worth retrying (rate-limit + server-side).
fn is_retryable_status(status: u16) -> bool {
    status == 429 || (500..600).contains(&status)
}

/// Backoff before attempt `attempt` (1-indexed). attempt=1 is the first retry.
fn backoff_delay(attempt: u32) -> Duration {
    // 200ms, 400ms, 800ms, ...
    Duration::from_millis(200u64 << (attempt - 1))
}

/// Drive `send` (a fresh request builder + send each call) with bounded
/// exponential backoff, retrying only on retryable statuses or transient send
/// errors. Returns the raw body bytes on a 2xx, or the terminal `ProviderError`.
async fn send_with_retry<F, Fut>(mut send: F) -> Result<bytes::Bytes, ProviderError>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<reqwest::Response, reqwest::Error>>,
{
    let mut attempt: u32 = 0;
    loop {
        attempt += 1;
        let outcome = send().await;
        let retryable_err;
        match outcome {
            Ok(resp) => {
                let status = resp.status().as_u16();
                let bytes = resp
                    .bytes()
                    .await
                    .map_err(|e| ProviderError::Http(e.to_string()))?;
                if (200..300).contains(&status) {
                    return Ok(bytes);
                }
                if is_retryable_status(status) && attempt < MAX_ATTEMPTS {
                    tracing::warn!(
                        status,
                        attempt,
                        "provider returned retryable status; backing off"
                    );
                    tokio::time::sleep(backoff_delay(attempt)).await;
                    continue;
                }
                return Err(ProviderError::Api {
                    status,
                    body: String::from_utf8_lossy(&bytes).to_string(),
                });
            }
            Err(e) => {
                // Treat timeouts/connect/request send errors as transient.
                retryable_err = e.is_timeout() || e.is_connect() || e.is_request();
                if retryable_err && attempt < MAX_ATTEMPTS {
                    tracing::warn!(error = %e, attempt, "transient send error; backing off");
                    tokio::time::sleep(backoff_delay(attempt)).await;
                    continue;
                }
                return Err(ProviderError::Http(e.to_string()));
            }
        }
    }
}

// ─── OpenAI-compatible provider ─────────────────────────────────────────────
//
// Supports two wire APIs:
//   * "chat"      → POST {base}/chat/completions   (classic)
//   * "responses" → POST {base}/responses          (new OpenAI Responses API)

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WireApi {
    Chat,
    Responses,
}

pub struct OpenAiProvider {
    pub name: String,
    pub base_url: String,
    pub api_key: Option<String>,
    pub default_model: Option<String>,
    pub extra_headers: std::collections::BTreeMap<String, String>,
    pub reasoning_effort: Option<String>,
    wire: WireApi,
}

impl OpenAiProvider {
    pub fn from_profile(p: &ProviderProfile) -> Self {
        let wire = match p.wire_api.as_deref().unwrap_or("chat") {
            "responses" => WireApi::Responses,
            _ => WireApi::Chat,
        };
        Self {
            name: p.name.clone(),
            base_url: p
                .base_url
                .clone()
                .unwrap_or_else(|| "https://api.openai.com/v1".into()),
            api_key: p.resolve_api_key(),
            default_model: p.default_model.clone(),
            extra_headers: p.headers.clone(),
            reasoning_effort: p.reasoning_effort.clone(),
            wire,
        }
    }
}

// ── classic chat/completions wire types ─────────────────────────────────────

#[derive(Serialize)]
struct OaiReq<'a> {
    model: &'a str,
    messages: Vec<OaiMsg>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_tokens: Option<u32>,
}

#[derive(Serialize)]
struct OaiMsg {
    role: String,
    /// Either a plain string (text-only) or an array of content blocks
    /// (`{type: "text"}` / `{type: "image_url"}`). OpenAI-compat APIs accept
    /// both shapes for vision-capable models.
    content: serde_json::Value,
}

/// Build the OpenAI chat-completions message payload. Falls back to a plain
/// string when there are no attachments so non-vision providers keep working.
fn openai_message(m: &ChatMessage) -> OaiMsg {
    let role = m.role.as_str().to_string();
    if m.attachments.is_empty() {
        return OaiMsg {
            role,
            content: serde_json::Value::String(m.content.clone()),
        };
    }
    let mut blocks: Vec<serde_json::Value> = Vec::with_capacity(m.attachments.len() + 1);
    if !m.content.is_empty() {
        blocks.push(serde_json::json!({ "type": "text", "text": m.content }));
    }
    for att in &m.attachments {
        let url = if att.is_url {
            att.data.clone()
        } else {
            format!("data:{};base64,{}", att.media_type, att.data)
        };
        blocks.push(serde_json::json!({
            "type": "image_url",
            "image_url": { "url": url },
        }));
    }
    OaiMsg {
        role,
        content: serde_json::Value::Array(blocks),
    }
}

#[derive(Deserialize)]
struct OaiResp {
    #[serde(default)]
    model: String,
    #[serde(default)]
    choices: Vec<OaiChoice>,
    #[serde(default)]
    usage: Option<OaiUsage>,
}

#[derive(Deserialize)]
struct OaiChoice {
    message: OaiOutMsg,
}

#[derive(Deserialize)]
struct OaiOutMsg {
    #[serde(default)]
    content: Option<String>,
}

#[derive(Deserialize, Default)]
struct OaiUsage {
    #[serde(default)]
    prompt_tokens: u32,
    #[serde(default)]
    completion_tokens: u32,
}

// ── Responses API wire types ────────────────────────────────────────────────

#[derive(Serialize)]
struct RespReq<'a> {
    model: &'a str,
    /// The Responses API accepts either a string or an array of input items.
    /// We always send an array of typed message items for richer prompts.
    input: Vec<RespInputItem<'a>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_output_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    instructions: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    reasoning: Option<RespReasoning<'a>>,
    /// `store=false` mirrors codex's `disable_response_storage`.
    #[serde(skip_serializing_if = "Option::is_none")]
    store: Option<bool>,
}

#[derive(Serialize)]
struct RespReasoning<'a> {
    effort: &'a str,
}

#[derive(Serialize)]
struct RespInputItem<'a> {
    role: &'a str,
    content: &'a str,
}

#[derive(Deserialize, Default)]
struct RespResp {
    #[serde(default)]
    model: String,
    /// Convenience field returned by some servers (sums of text blocks).
    #[serde(default)]
    output_text: Option<String>,
    #[serde(default)]
    output: Vec<RespOutItem>,
    #[serde(default)]
    usage: Option<RespUsage>,
    /// Some proxies return `{ "error": ... }` with 200 status.
    #[serde(default)]
    error: Option<serde_json::Value>,
}

#[derive(Deserialize)]
struct RespOutItem {
    #[serde(default, rename = "type")]
    kind: String,
    #[serde(default)]
    content: Vec<RespContentBlock>,
}

#[derive(Deserialize)]
struct RespContentBlock {
    #[serde(default, rename = "type")]
    kind: String,
    #[serde(default)]
    text: Option<String>,
}

#[derive(Deserialize, Default)]
struct RespUsage {
    #[serde(default)]
    input_tokens: u32,
    #[serde(default)]
    output_tokens: u32,
}

#[async_trait]
impl LlmProvider for OpenAiProvider {
    fn name(&self) -> &str {
        &self.name
    }
    fn id(&self) -> ProviderId {
        ProviderId::OpenaiCompat
    }
    fn supports(&self, model: &str) -> bool {
        if model.starts_with(&format!("{}/", self.name)) {
            return true;
        }
        if self.name == "openai" {
            return model.starts_with("gpt-")
                || model.starts_with("o1-")
                || model.starts_with("o3-")
                || model.starts_with("o4-")
                || model.starts_with("chatgpt-");
        }
        false
    }
    async fn chat(&self, req: ChatRequest) -> Result<ChatResponse, ProviderError> {
        if !network_allowed() {
            return Err(ProviderError::NetworkDisabled);
        }
        let api_key = self
            .api_key
            .clone()
            .ok_or_else(|| ProviderError::MissingApiKey(self.name.clone()))?;

        let model_in = if req.model.is_empty() {
            self.default_model.clone().unwrap_or_default()
        } else {
            self.upstream_model(&req.model)
        };

        match self.wire {
            WireApi::Chat => self.chat_completions(&api_key, &model_in, req).await,
            WireApi::Responses => self.responses_api(&api_key, &model_in, req).await,
        }
    }
}

impl OpenAiProvider {
    async fn chat_completions(
        &self,
        api_key: &str,
        model_in: &str,
        req: ChatRequest,
    ) -> Result<ChatResponse, ProviderError> {
        let mut messages: Vec<OaiMsg> = Vec::with_capacity(req.messages.len() + 1);
        if let Some(sys) = &req.system {
            messages.push(OaiMsg {
                role: "system".into(),
                content: serde_json::Value::String(sys.clone()),
            });
        }
        for m in &req.messages {
            messages.push(openai_message(m));
        }
        let body = OaiReq {
            model: model_in,
            messages,
            temperature: req.temperature,
            max_tokens: req.max_tokens,
        };
        let url = format!("{}/chat/completions", self.base_url.trim_end_matches('/'));
        let bytes = self.post_json(&url, api_key, &body).await?;
        let parsed: OaiResp = serde_json::from_slice(&bytes)
            .map_err(|e| ProviderError::Other(format!("decode chat resp: {e}")))?;
        let content = parsed
            .choices
            .into_iter()
            .next()
            .and_then(|c| c.message.content)
            .unwrap_or_default();
        let usage = parsed.usage.unwrap_or_default();
        Ok(ChatResponse {
            content,
            model: if parsed.model.is_empty() {
                model_in.to_string()
            } else {
                parsed.model
            },
            provider: self.name.clone(),
            input_tokens: usage.prompt_tokens,
            output_tokens: usage.completion_tokens,
        })
    }

    async fn responses_api(
        &self,
        api_key: &str,
        model_in: &str,
        req: ChatRequest,
    ) -> Result<ChatResponse, ProviderError> {
        let mut input: Vec<RespInputItem<'_>> = Vec::with_capacity(req.messages.len());
        for m in &req.messages {
            input.push(RespInputItem {
                role: m.role.as_str(),
                content: m.content.as_str(),
            });
        }
        let reasoning = self
            .reasoning_effort
            .as_deref()
            .map(|e| RespReasoning { effort: e });
        let body = RespReq {
            model: model_in,
            input,
            temperature: req.temperature,
            max_output_tokens: req.max_tokens,
            instructions: req.system.as_deref(),
            reasoning,
            store: Some(false),
        };
        let url = format!("{}/responses", self.base_url.trim_end_matches('/'));
        let bytes = self.post_json(&url, api_key, &body).await?;
        let parsed: RespResp = serde_json::from_slice(&bytes)
            .map_err(|e| ProviderError::Other(format!("decode responses: {e}")))?;
        if let Some(err) = parsed.error {
            return Err(ProviderError::Api {
                status: 200,
                body: err.to_string(),
            });
        }
        // Prefer `output_text`; fall back to flattening `output[*].content[*].text`.
        let mut content = parsed.output_text.clone().unwrap_or_default();
        if content.is_empty() {
            for item in &parsed.output {
                if item.kind != "message" && item.kind != "output_message" && !item.kind.is_empty()
                {
                    // We only extract text from message-like items.
                }
                for block in &item.content {
                    if block.kind == "output_text" || block.kind == "text" {
                        if let Some(t) = &block.text {
                            content.push_str(t);
                        }
                    }
                }
            }
        }
        let usage = parsed.usage.unwrap_or_default();
        Ok(ChatResponse {
            content,
            model: if parsed.model.is_empty() {
                model_in.to_string()
            } else {
                parsed.model
            },
            provider: self.name.clone(),
            input_tokens: usage.input_tokens,
            output_tokens: usage.output_tokens,
        })
    }

    async fn post_json<T: Serialize>(
        &self,
        url: &str,
        api_key: &str,
        body: &T,
    ) -> Result<bytes::Bytes, ProviderError> {
        let client = http_client(120_000)?;
        // Serialize the body once; reuse across retry attempts.
        let body_bytes = serde_json::to_vec(body)
            .map_err(|e| ProviderError::Other(format!("encode request: {e}")))?;
        send_with_retry(|| {
            let mut rb = client
                .post(url)
                .bearer_auth(api_key)
                .header("Content-Type", "application/json");
            for (k, v) in &self.extra_headers {
                let kl = k.to_ascii_lowercase();
                if matches!(kl.as_str(), "authorization" | "content-type") {
                    continue;
                }
                rb = rb.header(k, v);
            }
            rb.body(body_bytes.clone()).send()
        })
        .await
    }
}

// ─── Anthropic provider ─────────────────────────────────────────────────────
// Supports two auth schemes:
//   * x-api-key (Anthropic direct, default)
//   * Authorization: Bearer <token>  (ANTHROPIC_AUTH_TOKEN proxies)
// And honours:
//   * anthropic-version (default 2023-06-01, override via `headers`)
//   * anthropic-beta (comma-separated from env ANTHROPIC_BETAS or `headers`)

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AuthScheme {
    ApiKey,
    Bearer,
}

pub struct AnthropicProvider {
    pub name: String,
    pub base_url: String,
    pub api_key: Option<String>,
    pub default_model: Option<String>,
    pub anthropic_version: String,
    pub anthropic_beta: Option<String>,
    pub extra_headers: std::collections::BTreeMap<String, String>,
    auth: AuthScheme,
}

impl AnthropicProvider {
    pub fn from_profile(p: &ProviderProfile) -> Self {
        let mut version = "2023-06-01".to_string();
        let mut beta: Option<String> = None;
        for (k, v) in &p.headers {
            if k.eq_ignore_ascii_case("anthropic-version") {
                version = v.clone();
            }
            if k.eq_ignore_ascii_case("anthropic-beta") {
                beta = Some(v.clone());
            }
        }
        if beta.is_none() {
            if let Ok(env_betas) = std::env::var("ANTHROPIC_BETAS") {
                if !env_betas.is_empty() {
                    beta = Some(env_betas);
                }
            }
        }

        // Auth scheme: explicit profile setting wins; otherwise infer from
        // which env var is configured. AUTH_TOKEN ⇒ Bearer, API_KEY ⇒ x-api-key.
        let auth = match p.headers.get("auth").map(String::as_str) {
            Some("bearer") => AuthScheme::Bearer,
            Some("x-api-key") | Some("api_key") => AuthScheme::ApiKey,
            _ => {
                // Inspect which env var (if any) was named.
                let env_name = p.api_key_env.as_deref().unwrap_or("");
                if env_name.contains("AUTH_TOKEN") {
                    AuthScheme::Bearer
                } else if env_name.contains("API_KEY") {
                    AuthScheme::ApiKey
                } else if std::env::var("ANTHROPIC_AUTH_TOKEN").is_ok() {
                    AuthScheme::Bearer
                } else {
                    AuthScheme::ApiKey
                }
            }
        };

        Self {
            name: p.name.clone(),
            base_url: p
                .base_url
                .clone()
                .unwrap_or_else(|| "https://api.anthropic.com".into()),
            api_key: p.resolve_api_key(),
            default_model: p.default_model.clone(),
            anthropic_version: version,
            anthropic_beta: beta,
            extra_headers: p.headers.clone(),
            auth,
        }
    }
}

#[derive(Serialize)]
struct AntReq<'a> {
    model: &'a str,
    messages: Vec<AntMsg>,
    max_tokens: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    system: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
}

#[derive(Serialize)]
struct AntMsg {
    role: String,
    /// Either a plain string (text-only) or an array of content blocks
    /// (`{type: "text"}` / `{type: "image"}`). The Anthropic Messages API
    /// accepts both shapes.
    content: serde_json::Value,
}

/// Build the Anthropic message payload. Plain text when there are no image
/// attachments; otherwise an array of typed content blocks per the vision
/// docs (S-11/S-12).
fn anthropic_message(m: &ChatMessage) -> AntMsg {
    let role = m.role.as_str().to_string();
    if m.attachments.is_empty() {
        return AntMsg {
            role,
            content: serde_json::Value::String(m.content.clone()),
        };
    }
    let mut blocks: Vec<serde_json::Value> = Vec::with_capacity(m.attachments.len() + 1);
    if !m.content.is_empty() {
        blocks.push(serde_json::json!({ "type": "text", "text": m.content }));
    }
    for att in &m.attachments {
        let source = if att.is_url {
            serde_json::json!({ "type": "url", "url": att.data })
        } else {
            serde_json::json!({
                "type": "base64",
                "media_type": att.media_type,
                "data": att.data,
            })
        };
        blocks.push(serde_json::json!({ "type": "image", "source": source }));
    }
    AntMsg {
        role,
        content: serde_json::Value::Array(blocks),
    }
}

#[derive(Deserialize)]
struct AntResp {
    #[serde(default)]
    model: String,
    #[serde(default)]
    content: Vec<AntBlock>,
    #[serde(default)]
    usage: Option<AntUsage>,
}

#[derive(Deserialize)]
#[serde(tag = "type")]
enum AntBlock {
    #[serde(rename = "text")]
    Text {
        #[serde(default)]
        text: String,
    },
    #[serde(other)]
    Other,
}

#[derive(Deserialize, Default)]
struct AntUsage {
    #[serde(default)]
    input_tokens: u32,
    #[serde(default)]
    output_tokens: u32,
}

#[async_trait]
impl LlmProvider for AnthropicProvider {
    fn name(&self) -> &str {
        &self.name
    }
    fn id(&self) -> ProviderId {
        ProviderId::Anthropic
    }
    fn supports(&self, model: &str) -> bool {
        model.starts_with(&format!("{}/", self.name))
            || ((self.name == "anthropic" || self.name == "claude") && model.starts_with("claude-"))
    }
    async fn chat(&self, req: ChatRequest) -> Result<ChatResponse, ProviderError> {
        if !network_allowed() {
            return Err(ProviderError::NetworkDisabled);
        }
        let api_key = self
            .api_key
            .clone()
            .ok_or_else(|| ProviderError::MissingApiKey(self.name.clone()))?;

        let model_in = if req.model.is_empty() {
            self.default_model.clone().unwrap_or_default()
        } else {
            self.upstream_model(&req.model)
        };

        let messages: Vec<AntMsg> = req.messages.iter().map(anthropic_message).collect();

        let body = AntReq {
            model: &model_in,
            messages,
            max_tokens: req.max_tokens.unwrap_or(1024),
            system: req.system.as_deref(),
            temperature: req.temperature,
        };

        let url = format!("{}/v1/messages", self.base_url.trim_end_matches('/'));
        let client = http_client(60_000)?;
        let body_bytes = serde_json::to_vec(&body)
            .map_err(|e| ProviderError::Other(format!("encode request: {e}")))?;
        let bytes = send_with_retry(|| {
            let mut rb = client
                .post(&url)
                .header("anthropic-version", &self.anthropic_version)
                .header("Content-Type", "application/json");
            if let Some(beta) = &self.anthropic_beta {
                rb = rb.header("anthropic-beta", beta);
            }
            rb = match self.auth {
                AuthScheme::ApiKey => rb.header("x-api-key", &api_key),
                AuthScheme::Bearer => rb.bearer_auth(&api_key),
            };
            for (k, v) in &self.extra_headers {
                // Don't double-set headers we manage explicitly.
                let kl = k.to_ascii_lowercase();
                if matches!(
                    kl.as_str(),
                    "anthropic-version"
                        | "anthropic-beta"
                        | "x-api-key"
                        | "authorization"
                        | "content-type"
                        | "auth"
                ) {
                    continue;
                }
                rb = rb.header(k, v);
            }
            rb.body(body_bytes.clone()).send()
        })
        .await?;
        let parsed: AntResp = serde_json::from_slice(&bytes)
            .map_err(|e| ProviderError::Other(format!("decode anthropic resp: {e}")))?;
        let mut content = String::new();
        for block in parsed.content {
            if let AntBlock::Text { text } = block {
                content.push_str(&text);
            }
        }
        let usage = parsed.usage.unwrap_or_default();
        Ok(ChatResponse {
            content,
            model: if parsed.model.is_empty() {
                model_in.clone()
            } else {
                parsed.model
            },
            provider: self.name.clone(),
            input_tokens: usage.input_tokens,
            output_tokens: usage.output_tokens,
        })
    }
}

#[cfg(test)]
mod vision_tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn anthropic_text_only_message_stays_a_string() {
        let m = ChatMessage::text(Role::User, "hello");
        let v = serde_json::to_value(anthropic_message(&m)).unwrap();
        assert_eq!(v["role"], "user");
        assert_eq!(v["content"], "hello");
    }

    #[test]
    fn anthropic_message_with_base64_image_becomes_content_blocks() {
        let m = ChatMessage {
            role: Role::User,
            content: "what is in this?".into(),
            attachments: vec![ImageAttachment::base64("image/png", "AAAA")],
        };
        let v = serde_json::to_value(anthropic_message(&m)).unwrap();
        let blocks = v["content"].as_array().unwrap();
        assert_eq!(blocks.len(), 2, "text + image");
        assert_eq!(blocks[0]["type"], "text");
        assert_eq!(blocks[0]["text"], "what is in this?");
        assert_eq!(blocks[1]["type"], "image");
        assert_eq!(blocks[1]["source"]["type"], "base64");
        assert_eq!(blocks[1]["source"]["media_type"], "image/png");
        assert_eq!(blocks[1]["source"]["data"], "AAAA");
    }

    #[test]
    fn anthropic_message_with_url_image_uses_url_source() {
        let m = ChatMessage {
            role: Role::User,
            content: String::new(),
            attachments: vec![ImageAttachment::url(
                "image/jpeg",
                "https://x.example/y.jpg",
            )],
        };
        let v = serde_json::to_value(anthropic_message(&m)).unwrap();
        let blocks = v["content"].as_array().unwrap();
        assert_eq!(blocks.len(), 1, "empty text dropped, image kept");
        assert_eq!(blocks[0]["type"], "image");
        assert_eq!(blocks[0]["source"]["type"], "url");
        assert_eq!(blocks[0]["source"]["url"], "https://x.example/y.jpg");
    }

    #[test]
    fn openai_text_only_message_stays_a_string() {
        let m = ChatMessage::text(Role::User, "hi");
        let v = serde_json::to_value(openai_message(&m)).unwrap();
        assert_eq!(v["content"], "hi");
    }

    #[test]
    fn openai_message_with_base64_becomes_data_url() {
        let m = ChatMessage {
            role: Role::User,
            content: "tell me about this".into(),
            attachments: vec![ImageAttachment::base64("image/png", "ZZZ")],
        };
        let v = serde_json::to_value(openai_message(&m)).unwrap();
        let blocks = v["content"].as_array().unwrap();
        assert_eq!(blocks.len(), 2);
        assert_eq!(blocks[1]["type"], "image_url");
        assert_eq!(blocks[1]["image_url"]["url"], "data:image/png;base64,ZZZ");
    }

    #[test]
    fn openai_message_with_url_passes_url_through() {
        let m = ChatMessage {
            role: Role::User,
            content: String::new(),
            attachments: vec![ImageAttachment::url("image/png", "https://x/y.png")],
        };
        let v = serde_json::to_value(openai_message(&m)).unwrap();
        let blocks = v["content"].as_array().unwrap();
        assert_eq!(blocks[0]["image_url"]["url"], "https://x/y.png");
    }

    #[test]
    fn chat_message_default_attachments_round_trip_serde() {
        // Older payloads without `attachments` should still deserialize.
        let parsed: ChatMessage = serde_json::from_value(json!({
            "role": "user",
            "content": "ping"
        }))
        .unwrap();
        assert!(parsed.attachments.is_empty());
    }
}
