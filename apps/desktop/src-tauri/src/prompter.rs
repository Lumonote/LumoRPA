use super::*;

pub(super) fn desktop_prompter(
    app: &AppHandle,
    state: &State<'_, DesktopState>,
) -> Arc<dyn HumanPrompter> {
    Arc::new(TauriPrompter {
        app: app.clone(),
        prompts: state.prompts.clone(),
    })
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct HumanPromptEvent {
    prompt_id: String,
    kind: String,
    message: String,
    default: Option<Value>,
    timeout_ms: u64,
    run_id: String,
    step_path: String,
}

struct TauriPrompter {
    app: AppHandle,
    prompts: PromptMap,
}

struct PromptCleanup {
    prompts: PromptMap,
    id: String,
}

impl Drop for PromptCleanup {
    fn drop(&mut self) {
        self.prompts
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&self.id);
    }
}

fn decode_human_response(value: Value) -> HumanResponse {
    if let Value::Object(map) = &value {
        let inner = map
            .get("value")
            .or_else(|| map.get("approved"))
            .or_else(|| map.get("confirmed"));
        if let Some(inner) = inner {
            return HumanResponse {
                value: inner.clone(),
                by: map.get("by").and_then(Value::as_str).map(str::to_string),
                comment: map
                    .get("comment")
                    .and_then(Value::as_str)
                    .map(str::to_string),
            };
        }
    }
    HumanResponse {
        value,
        by: None,
        comment: None,
    }
}

#[async_trait::async_trait]
impl HumanPrompter for TauriPrompter {
    async fn prompt(&self, req: HumanPromptRequest) -> Result<HumanResponse, StepError> {
        let prompt_id = ulid::Ulid::new().to_string();
        let (tx, rx) = tokio::sync::oneshot::channel::<Value>();
        self.prompts
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(prompt_id.clone(), tx);
        let _cleanup = PromptCleanup {
            prompts: self.prompts.clone(),
            id: prompt_id.clone(),
        };
        let kind = match req.kind {
            HumanPromptKind::Input => "input",
            HumanPromptKind::Confirm => "confirm",
            HumanPromptKind::Approve => "approve",
        };
        self.app
            .emit(
                "human-prompt",
                &HumanPromptEvent {
                    prompt_id,
                    kind: kind.into(),
                    message: req.message,
                    default: req.default,
                    timeout_ms: req.timeout_ms,
                    run_id: req.run_id,
                    step_path: req.step_path,
                },
            )
            .map_err(|e| StepError::msg(format!("emit human-prompt: {e}")))?;
        let value = rx
            .await
            .map_err(|_| StepError::msg("human prompt channel closed without a response"))?;
        Ok(decode_human_response(value))
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct HumanRespondResult {
    ok: bool,
}

#[tauri::command]
pub(super) fn human_respond(
    state: State<'_, DesktopState>,
    prompt_id: String,
    value: Value,
) -> HumanRespondResult {
    let sender = state
        .prompts
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(&prompt_id);
    HumanRespondResult {
        ok: sender.is_some_and(|tx| tx.send(value).is_ok()),
    }
}
