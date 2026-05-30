//! Clipboard actions — `clipboard.get` / `clipboard.set` (S-class F-5).
//!
//! Plain-text clipboard via `arboard`. Each call builds a short-lived
//! `Clipboard` inside `spawn_blocking` (arboard's handle is `!Send` and the call
//! is blocking). Headless / no-display environments surface a clear
//! `clipboard unavailable: …` error instead of panicking.
//!
//! No capability gate — these are local, info-only actions like `system.env_get`.
//! Two caveats flow authors should know: (1) reading the clipboard can expose
//! sensitive data (e.g. a password manager's last copy); (2) on Linux/X11 the
//! contents written may not persist after this process exits.

use async_trait::async_trait;
use lumo_core::error::StepError;
use lumo_core::{Action, ActionRegistry, ActionResult, StepCtx};
use once_cell::sync::Lazy;
use serde::Deserialize;
use serde_json::Value;

pub fn register(r: &mut ActionRegistry) {
    r.register(GetAction);
    r.register(SetAction);
}

// Indirection so the arboard backend can be swapped for a per-target stub
// (Task 7 fallback) without touching the action bodies.
fn clipboard_get_text() -> Result<String, String> {
    let mut cb = arboard::Clipboard::new().map_err(|e| format!("clipboard unavailable: {e}"))?;
    cb.get_text().map_err(|e| format!("clipboard read: {e}"))
}
fn clipboard_set_text(text: String) -> Result<(), String> {
    let mut cb = arboard::Clipboard::new().map_err(|e| format!("clipboard unavailable: {e}"))?;
    cb.set_text(text)
        .map_err(|e| format!("clipboard write: {e}"))
}

pub struct GetAction;

#[async_trait]
impl Action for GetAction {
    fn id(&self) -> &'static str {
        "clipboard.get"
    }
    fn summary(&self) -> &'static str {
        "Read text from the system clipboard"
    }
    fn schema(&self) -> &'static Value {
        static SCHEMA: Lazy<Value> = Lazy::new(|| {
            serde_json::json!({
                "type": "object",
                "properties": {},
                "additionalProperties": false
            })
        });
        &SCHEMA
    }
    async fn execute(&self, _ctx: &mut StepCtx, _input: Value) -> Result<ActionResult, StepError> {
        let text = tokio::task::spawn_blocking(clipboard_get_text)
            .await
            .map_err(|e| StepError::msg(format!("clipboard.get task: {e}")))?
            .map_err(StepError::msg)?;
        Ok(ActionResult::from(serde_json::json!({ "text": text })))
    }
}

pub struct SetAction;

#[derive(Deserialize)]
struct SetIn {
    text: String,
}

#[async_trait]
impl Action for SetAction {
    fn id(&self) -> &'static str {
        "clipboard.set"
    }
    fn summary(&self) -> &'static str {
        "Write text to the system clipboard"
    }
    fn schema(&self) -> &'static Value {
        static SCHEMA: Lazy<Value> = Lazy::new(|| {
            serde_json::json!({
                "type": "object",
                "required": ["text"],
                "properties": { "text": { "type": "string" } },
                "additionalProperties": false
            })
        });
        &SCHEMA
    }
    async fn execute(&self, _ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let SetIn { text } = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("clipboard.set input invalid: {e}")))?;
        tokio::task::spawn_blocking(move || clipboard_set_text(text))
            .await
            .map_err(|e| StepError::msg(format!("clipboard.set task: {e}")))?
            .map_err(StepError::msg)?;
        Ok(ActionResult::from(serde_json::json!({ "ok": true })))
    }
}
