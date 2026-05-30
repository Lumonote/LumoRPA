//! Regex actions (`regex.*`). Powered by the `regex` crate (Rust syntax).

use async_trait::async_trait;
use lumo_core::error::StepError;
use lumo_core::{Action, ActionRegistry, ActionResult, StepCtx};
use once_cell::sync::Lazy;
use regex::Regex;
use serde::Deserialize;
use serde_json::{Map, Value};

pub fn register(r: &mut ActionRegistry) {
    r.register(MatchAction);
    r.register(FindAllAction);
    r.register(ReplaceAction);
    r.register(CapturesAction);
}

#[derive(Deserialize)]
struct PatTextIn {
    pattern: String,
    text: String,
}

fn compile(pattern: &str) -> Result<Regex, StepError> {
    Regex::new(pattern).map_err(|e| StepError::msg(format!("regex compile error: {e}")))
}

fn pat_text_schema() -> &'static Value {
    static S: Lazy<Value> = Lazy::new(|| {
        serde_json::json!({
            "type": "object",
            "required": ["pattern", "text"],
            "properties": {
                "pattern": { "type": "string" },
                "text":    { "type": "string" }
            },
            "additionalProperties": false
        })
    });
    &S
}

pub struct MatchAction;
#[async_trait]
impl Action for MatchAction {
    fn id(&self) -> &'static str {
        "regex.match"
    }
    fn summary(&self) -> &'static str {
        "Return true when `pattern` matches any part of `text`"
    }
    fn schema(&self) -> &'static Value {
        pat_text_schema()
    }
    async fn execute(&self, _ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let PatTextIn { pattern, text } = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("regex.match invalid: {e}")))?;
        let re = compile(&pattern)?;
        Ok(ActionResult::from(Value::Bool(re.is_match(&text))))
    }
}

pub struct FindAllAction;
#[async_trait]
impl Action for FindAllAction {
    fn id(&self) -> &'static str {
        "regex.find_all"
    }
    fn summary(&self) -> &'static str {
        "Return every non-overlapping match as a string array"
    }
    fn schema(&self) -> &'static Value {
        pat_text_schema()
    }
    async fn execute(&self, _ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let PatTextIn { pattern, text } = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("regex.find_all invalid: {e}")))?;
        let re = compile(&pattern)?;
        let arr: Vec<Value> = re
            .find_iter(&text)
            .map(|m| Value::String(m.as_str().to_string()))
            .collect();
        Ok(ActionResult::from(Value::Array(arr)))
    }
}

pub struct ReplaceAction;
#[derive(Deserialize)]
struct ReplaceIn {
    pattern: String,
    text: String,
    replacement: String,
    #[serde(default)]
    once: bool,
}
#[async_trait]
impl Action for ReplaceAction {
    fn id(&self) -> &'static str {
        "regex.replace"
    }
    fn summary(&self) -> &'static str {
        "Replace pattern with `replacement` (supports $1, $2 capture refs)"
    }
    fn schema(&self) -> &'static Value {
        static S: Lazy<Value> = Lazy::new(|| {
            serde_json::json!({
                "type": "object",
                "required": ["pattern", "text", "replacement"],
                "properties": {
                    "pattern":     { "type": "string" },
                    "text":        { "type": "string" },
                    "replacement": { "type": "string" },
                    "once":        { "type": "boolean", "default": false }
                },
                "additionalProperties": false
            })
        });
        &S
    }
    async fn execute(&self, _ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let ReplaceIn {
            pattern,
            text,
            replacement,
            once,
        } = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("regex.replace invalid: {e}")))?;
        let re = compile(&pattern)?;
        let out = if once {
            re.replace(&text, replacement.as_str()).into_owned()
        } else {
            re.replace_all(&text, replacement.as_str()).into_owned()
        };
        Ok(ActionResult::from(Value::String(out)))
    }
}

pub struct CapturesAction;
#[async_trait]
impl Action for CapturesAction {
    fn id(&self) -> &'static str {
        "regex.captures"
    }
    fn summary(&self) -> &'static str {
        "Return first match's groups: { full, groups[], named{} }"
    }
    fn schema(&self) -> &'static Value {
        pat_text_schema()
    }
    async fn execute(&self, _ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let PatTextIn { pattern, text } = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("regex.captures invalid: {e}")))?;
        let re = compile(&pattern)?;
        let Some(caps) = re.captures(&text) else {
            return Ok(ActionResult::from(Value::Null));
        };
        let full = caps
            .get(0)
            .map(|m| m.as_str().to_string())
            .unwrap_or_default();
        let groups: Vec<Value> = caps
            .iter()
            .skip(1)
            .map(|m| {
                m.map(|m| Value::String(m.as_str().to_string()))
                    .unwrap_or(Value::Null)
            })
            .collect();
        let mut named = Map::new();
        for name in re.capture_names().flatten() {
            if let Some(m) = caps.name(name) {
                named.insert(name.to_string(), Value::String(m.as_str().to_string()));
            }
        }
        Ok(ActionResult::from(serde_json::json!({
            "full": full,
            "groups": groups,
            "named": Value::Object(named),
        })))
    }
}
