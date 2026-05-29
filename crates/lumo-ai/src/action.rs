use crate::{
    cost::cost_micro,
    provider::{ChatMessage, ChatRequest, Role},
    router::AiRouter,
};
use async_trait::async_trait;
use lumo_core::error::StepError;
use lumo_core::{Action, ActionResult, StepCtx};
use lumo_storage::AiCallInsert;
use once_cell::sync::Lazy;
use serde::Deserialize;
use serde_json::Value;
use std::sync::Arc;
use std::time::Instant;
use tracing::Instrument;

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
        // X-05: OTel GenAI semconv — emit a `gen_ai.chat` span around the LLM
        // round-trip. The request model may be empty (router falls back to the
        // active profile default), so we record it as-is for the request side;
        // the resolved model lands on the X-10 cost row below.
        let req_model = req.model.clone();
        let t0 = Instant::now();
        let resp = self
            .router
            .chat(req)
            .instrument(tracing::info_span!(
                "gen_ai.chat",
                "otel.name" = "gen_ai.chat",
                "gen_ai.operation.name" = "chat",
                "gen_ai.system" = "claude",
                "gen_ai.request.model" = %req_model,
            ))
            .await
            .map_err(|e| StepError::msg(format!("ai.chat: {e}")))?;
        let latency_ms = t0.elapsed().as_millis() as i64;
        let cost = cost_micro(&resp.provider, &resp.model, resp.input_tokens, resp.output_tokens);

        // X-10: persist a row per call so `lumo runs cost` + Studio can show
        // tokens / $ per step. Best-effort — a failed insert never blocks the
        // flow.
        if let Some(repo) = ctx.repo() {
            let step_id = ctx.current_step_id();
            let _ = repo.record_ai_call(AiCallInsert {
                flow_run_id: ctx.run_id(),
                step_id: step_id.as_deref(),
                helper: "chat",
                provider: &resp.provider,
                model: &resp.model,
                input_tokens: resp.input_tokens as i64,
                output_tokens: resp.output_tokens as i64,
                latency_ms,
                cost_usd_micro: cost,
            });
        }

        Ok(ActionResult::from(serde_json::json!({
            "content": resp.content,
            "model": resp.model,
            "provider": resp.provider,
            "usage": {
                "input_tokens":  resp.input_tokens,
                "output_tokens": resp.output_tokens,
                "latency_ms":    latency_ms,
                "cost_usd_micro": cost,
            }
        })))
    }
}
