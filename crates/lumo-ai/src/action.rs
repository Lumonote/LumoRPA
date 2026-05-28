use crate::{
    provider::{ChatMessage, ChatRequest, Role},
    router::AiRouter,
};
use async_trait::async_trait;
use lumo_core::error::StepError;
use lumo_core::{Action, ActionResult, StepCtx};
use once_cell::sync::Lazy;
use serde::Deserialize;
use serde_json::Value;
use std::sync::Arc;

/// `ai.chat` action. Backed by a shared `AiRouter`.
pub struct ChatAction {
    pub router: Arc<AiRouter>,
}

impl ChatAction {
    pub fn new(router: Arc<AiRouter>) -> Self {
        Self { router }
    }
}

#[derive(Deserialize)]
struct ChatIn {
    /// Optional. When omitted or empty, the AiRouter falls back to the
    /// active profile's `default_model`. Flows should NOT hard-code a model
    /// id; let provider configuration decide.
    #[serde(default)]
    model: String,
    #[serde(default)]
    system: Option<String>,
    prompt: String,
    #[serde(default)]
    temperature: Option<f32>,
    #[serde(default)]
    max_tokens: Option<u32>,
}

#[async_trait]
impl Action for ChatAction {
    fn id(&self) -> &'static str {
        "ai.chat"
    }
    fn summary(&self) -> &'static str {
        "Send a chat prompt through the AI router"
    }
    fn schema(&self) -> &'static serde_json::Value {
        static SCHEMA: Lazy<Value> = Lazy::new(|| {
            serde_json::json!({
                "type": "object",
                "required": ["prompt"],
                "properties": {
                    "model": { "type": "string" },
                    "system": { "type": "string" },
                    "prompt": { "type": "string" },
                    "temperature": { "type": "number" },
                    "max_tokens": { "type": "integer" }
                },
                "additionalProperties": false
            })
        });
        &SCHEMA
    }
    async fn execute(&self, ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let cfg: ChatIn = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("ai.chat input invalid: {e}")))?;
        ctx.ensure_llm(&cfg.model)?;
        let req = ChatRequest {
            model: cfg.model,
            system: cfg.system,
            temperature: cfg.temperature,
            max_tokens: cfg.max_tokens,
            messages: vec![ChatMessage::text(Role::User, cfg.prompt)],
        };
        let resp = self
            .router
            .chat(req)
            .await
            .map_err(|e| StepError::msg(format!("ai.chat: {e}")))?;
        Ok(ActionResult::from(serde_json::json!({
            "content": resp.content,
            "model": resp.model,
            "provider": resp.provider,
            "usage": {
                "input_tokens": resp.input_tokens,
                "output_tokens": resp.output_tokens,
            }
        })))
    }
}
