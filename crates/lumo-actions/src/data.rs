//! Data manipulation actions.

use async_trait::async_trait;
use lumo_core::{Action, ActionRegistry, ActionResult, StepCtx};
use lumo_core::error::StepError;
use serde::Deserialize;
use serde_json::Value;

pub fn register(r: &mut ActionRegistry) {
    r.register(JsonParseAction);
    r.register(JsonFormatAction);
}

pub struct JsonParseAction;
#[derive(Deserialize)]
struct ParseIn { text: String }

#[async_trait]
impl Action for JsonParseAction {
    fn id(&self) -> &'static str { "data.json_parse" }
    fn summary(&self) -> &'static str { "Parse a JSON string into a value" }
    async fn execute(&self, _ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let ParseIn { text } = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("json_parse input invalid: {e}")))?;
        let v: Value = serde_json::from_str(&text)
            .map_err(|e| StepError::msg(format!("json parse error: {e}")))?;
        Ok(ActionResult::from(v))
    }
}

pub struct JsonFormatAction;
#[derive(Deserialize)]
struct FmtIn { value: Value, #[serde(default)] pretty: bool }

#[async_trait]
impl Action for JsonFormatAction {
    fn id(&self) -> &'static str { "data.json_format" }
    fn summary(&self) -> &'static str { "Serialize a value to JSON string" }
    async fn execute(&self, _ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let FmtIn { value, pretty } = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("json_format input invalid: {e}")))?;
        let s = if pretty { serde_json::to_string_pretty(&value) } else { serde_json::to_string(&value) }
            .map_err(|e| StepError::msg(format!("json format error: {e}")))?;
        Ok(ActionResult::from(Value::String(s)))
    }
}
