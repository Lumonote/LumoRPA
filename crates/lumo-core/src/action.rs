use crate::error::StepError;
use async_trait::async_trait;
use serde_json::Value;
use std::sync::Arc;

use crate::ctx::StepCtx;

#[derive(Debug, Clone)]
pub struct ActionResult {
    pub output: Value,
}

impl ActionResult {
    pub fn null() -> Self {
        Self {
            output: Value::Null,
        }
    }
    pub fn from(v: Value) -> Self {
        Self { output: v }
    }
}

/// All actions implement this trait.
///
/// `execute` receives the *rendered* input (template expressions already
/// substituted, vault placeholders preserved) and a mutable `StepCtx`
/// that exposes shared resources (browser pool, http client, etc.) and
/// allows recording sub-events.
#[async_trait]
pub trait Action: Send + Sync + 'static {
    fn id(&self) -> &'static str;

    fn summary(&self) -> &'static str {
        ""
    }

    /// JSON schema for the `with:` block. Used by `lumo actions --show <id>`
    /// and by future Studio UIs to render forms.
    fn schema(&self) -> &'static serde_json::Value {
        static EMPTY: once_cell::sync::Lazy<serde_json::Value> =
            once_cell::sync::Lazy::new(|| serde_json::json!({ "type": "object" }));
        &EMPTY
    }

    async fn execute(&self, ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError>;
}

pub type ActionRef = Arc<dyn Action>;
