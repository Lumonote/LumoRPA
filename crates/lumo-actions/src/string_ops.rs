//! String manipulation actions (`string.*`).
//!
//! YingDao's "字符串" family parity: case, trim, split/join, slice, contains,
//! replace, repeat, pad, format. All sync over `&str`.

use async_trait::async_trait;
use lumo_core::error::StepError;
use lumo_core::{Action, ActionRegistry, ActionResult, StepCtx};
use once_cell::sync::Lazy;
use serde::Deserialize;
use serde_json::Value;

pub fn register(r: &mut ActionRegistry) {
    r.register(UpperAction);
    r.register(LowerAction);
    r.register(TrimAction);
    r.register(LengthAction);
    r.register(SplitAction);
    r.register(JoinAction);
    r.register(ReplaceAction);
    r.register(ContainsAction);
    r.register(StartsWithAction);
    r.register(EndsWithAction);
    r.register(SubstringAction);
    r.register(RepeatAction);
    r.register(PadLeftAction);
    r.register(PadRightAction);
    r.register(FormatAction);
}

fn text_only_schema() -> &'static Value {
    static S: Lazy<Value> = Lazy::new(|| {
        serde_json::json!({
            "type": "object",
            "required": ["text"],
            "properties": { "text": { "type": "string" } },
            "additionalProperties": false
        })
    });
    &S
}

#[derive(Deserialize)]
struct TextIn {
    text: String,
}

pub struct UpperAction;
#[async_trait]
impl Action for UpperAction {
    fn id(&self) -> &'static str {
        "string.upper"
    }
    fn summary(&self) -> &'static str {
        "Uppercase a string"
    }
    fn schema(&self) -> &'static Value {
        text_only_schema()
    }
    async fn execute(&self, _ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let TextIn { text } = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("string.upper invalid: {e}")))?;
        Ok(ActionResult::from(Value::String(text.to_uppercase())))
    }
}

pub struct LowerAction;
#[async_trait]
impl Action for LowerAction {
    fn id(&self) -> &'static str {
        "string.lower"
    }
    fn summary(&self) -> &'static str {
        "Lowercase a string"
    }
    fn schema(&self) -> &'static Value {
        text_only_schema()
    }
    async fn execute(&self, _ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let TextIn { text } = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("string.lower invalid: {e}")))?;
        Ok(ActionResult::from(Value::String(text.to_lowercase())))
    }
}

pub struct TrimAction;
#[async_trait]
impl Action for TrimAction {
    fn id(&self) -> &'static str {
        "string.trim"
    }
    fn summary(&self) -> &'static str {
        "Trim leading/trailing whitespace"
    }
    fn schema(&self) -> &'static Value {
        text_only_schema()
    }
    async fn execute(&self, _ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let TextIn { text } = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("string.trim invalid: {e}")))?;
        Ok(ActionResult::from(Value::String(text.trim().to_string())))
    }
}

pub struct LengthAction;
#[async_trait]
impl Action for LengthAction {
    fn id(&self) -> &'static str {
        "string.length"
    }
    fn summary(&self) -> &'static str {
        "Count characters (not bytes)"
    }
    fn schema(&self) -> &'static Value {
        text_only_schema()
    }
    async fn execute(&self, _ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let TextIn { text } = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("string.length invalid: {e}")))?;
        Ok(ActionResult::from(Value::from(text.chars().count() as u64)))
    }
}

pub struct SplitAction;
#[derive(Deserialize)]
struct SplitIn {
    text: String,
    #[serde(default = "default_sep")]
    sep: String,
    #[serde(default)]
    limit: Option<usize>,
}
fn default_sep() -> String {
    ",".into()
}

#[async_trait]
impl Action for SplitAction {
    fn id(&self) -> &'static str {
        "string.split"
    }
    fn summary(&self) -> &'static str {
        "Split text by separator into an array"
    }
    fn schema(&self) -> &'static Value {
        static S: Lazy<Value> = Lazy::new(|| {
            serde_json::json!({
                "type": "object",
                "required": ["text"],
                "properties": {
                    "text":  { "type": "string" },
                    "sep":   { "type": "string", "default": "," },
                    "limit": { "type": "integer", "minimum": 1 }
                },
                "additionalProperties": false
            })
        });
        &S
    }
    async fn execute(&self, _ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let SplitIn { text, sep, limit } = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("string.split invalid: {e}")))?;
        let items: Vec<Value> = if let Some(n) = limit {
            text.splitn(n, sep.as_str())
                .map(|p| Value::String(p.to_string()))
                .collect()
        } else {
            text.split(sep.as_str())
                .map(|p| Value::String(p.to_string()))
                .collect()
        };
        Ok(ActionResult::from(Value::Array(items)))
    }
}

pub struct JoinAction;
#[derive(Deserialize)]
struct JoinIn {
    items: Vec<Value>,
    #[serde(default = "default_sep")]
    sep: String,
}
#[async_trait]
impl Action for JoinAction {
    fn id(&self) -> &'static str {
        "string.join"
    }
    fn summary(&self) -> &'static str {
        "Join array elements into a single string"
    }
    fn schema(&self) -> &'static Value {
        static S: Lazy<Value> = Lazy::new(|| {
            serde_json::json!({
                "type": "object",
                "required": ["items"],
                "properties": {
                    "items": { "type": "array" },
                    "sep":   { "type": "string", "default": "," }
                },
                "additionalProperties": false
            })
        });
        &S
    }
    async fn execute(&self, _ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let JoinIn { items, sep } = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("string.join invalid: {e}")))?;
        let parts: Vec<String> = items
            .iter()
            .map(|v| match v {
                Value::String(s) => s.clone(),
                other => other.to_string(),
            })
            .collect();
        Ok(ActionResult::from(Value::String(parts.join(&sep))))
    }
}

pub struct ReplaceAction;
#[derive(Deserialize)]
struct ReplaceIn {
    text: String,
    from: String,
    to: String,
    #[serde(default)]
    once: bool,
}
#[async_trait]
impl Action for ReplaceAction {
    fn id(&self) -> &'static str {
        "string.replace"
    }
    fn summary(&self) -> &'static str {
        "Replace `from` with `to` (literal, not regex)"
    }
    fn schema(&self) -> &'static Value {
        static S: Lazy<Value> = Lazy::new(|| {
            serde_json::json!({
                "type": "object",
                "required": ["text", "from", "to"],
                "properties": {
                    "text": { "type": "string" },
                    "from": { "type": "string" },
                    "to":   { "type": "string" },
                    "once": { "type": "boolean", "default": false }
                },
                "additionalProperties": false
            })
        });
        &S
    }
    async fn execute(&self, _ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let ReplaceIn {
            text,
            from,
            to,
            once,
        } = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("string.replace invalid: {e}")))?;
        let out = if once {
            text.replacen(&from, &to, 1)
        } else {
            text.replace(&from, &to)
        };
        Ok(ActionResult::from(Value::String(out)))
    }
}

pub struct ContainsAction;
#[derive(Deserialize)]
struct ContainsIn {
    text: String,
    needle: String,
    #[serde(default = "default_true")]
    case_sensitive: bool,
}
fn default_true() -> bool {
    true
}

#[async_trait]
impl Action for ContainsAction {
    fn id(&self) -> &'static str {
        "string.contains"
    }
    fn summary(&self) -> &'static str {
        "Test whether `text` contains `needle`"
    }
    fn schema(&self) -> &'static Value {
        static S: Lazy<Value> = Lazy::new(|| {
            serde_json::json!({
                "type": "object",
                "required": ["text", "needle"],
                "properties": {
                    "text":   { "type": "string" },
                    "needle": { "type": "string" },
                    "case_sensitive": { "type": "boolean", "default": true }
                },
                "additionalProperties": false
            })
        });
        &S
    }
    async fn execute(&self, _ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let ContainsIn {
            text,
            needle,
            case_sensitive,
        } = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("string.contains invalid: {e}")))?;
        let hit = if case_sensitive {
            text.contains(&needle)
        } else {
            text.to_lowercase().contains(&needle.to_lowercase())
        };
        Ok(ActionResult::from(Value::Bool(hit)))
    }
}

pub struct StartsWithAction;
#[derive(Deserialize)]
struct StartsIn {
    text: String,
    prefix: String,
}
#[async_trait]
impl Action for StartsWithAction {
    fn id(&self) -> &'static str {
        "string.starts_with"
    }
    fn summary(&self) -> &'static str {
        "Test whether `text` starts with prefix"
    }
    fn schema(&self) -> &'static Value {
        static S: Lazy<Value> = Lazy::new(|| {
            serde_json::json!({
                "type": "object",
                "required": ["text", "prefix"],
                "properties": {
                    "text":   { "type": "string" },
                    "prefix": { "type": "string" }
                },
                "additionalProperties": false
            })
        });
        &S
    }
    async fn execute(&self, _ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let StartsIn { text, prefix } = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("string.starts_with invalid: {e}")))?;
        Ok(ActionResult::from(Value::Bool(text.starts_with(&prefix))))
    }
}

pub struct EndsWithAction;
#[derive(Deserialize)]
struct EndsIn {
    text: String,
    suffix: String,
}
#[async_trait]
impl Action for EndsWithAction {
    fn id(&self) -> &'static str {
        "string.ends_with"
    }
    fn summary(&self) -> &'static str {
        "Test whether `text` ends with suffix"
    }
    fn schema(&self) -> &'static Value {
        static S: Lazy<Value> = Lazy::new(|| {
            serde_json::json!({
                "type": "object",
                "required": ["text", "suffix"],
                "properties": {
                    "text":   { "type": "string" },
                    "suffix": { "type": "string" }
                },
                "additionalProperties": false
            })
        });
        &S
    }
    async fn execute(&self, _ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let EndsIn { text, suffix } = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("string.ends_with invalid: {e}")))?;
        Ok(ActionResult::from(Value::Bool(text.ends_with(&suffix))))
    }
}

pub struct SubstringAction;
#[derive(Deserialize)]
struct SubIn {
    text: String,
    start: i64,
    #[serde(default)]
    end: Option<i64>,
}
#[async_trait]
impl Action for SubstringAction {
    fn id(&self) -> &'static str {
        "string.substring"
    }
    fn summary(&self) -> &'static str {
        "Character slice; negatives count from end"
    }
    fn schema(&self) -> &'static Value {
        static S: Lazy<Value> = Lazy::new(|| {
            serde_json::json!({
                "type": "object",
                "required": ["text", "start"],
                "properties": {
                    "text":  { "type": "string" },
                    "start": { "type": "integer" },
                    "end":   { "type": "integer" }
                },
                "additionalProperties": false
            })
        });
        &S
    }
    async fn execute(&self, _ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let SubIn { text, start, end } = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("string.substring invalid: {e}")))?;
        let chars: Vec<char> = text.chars().collect();
        let len = chars.len() as i64;
        let norm = |i: i64| {
            if i < 0 {
                (len + i).max(0) as usize
            } else {
                i.min(len).max(0) as usize
            }
        };
        let s = norm(start);
        let e = end.map(norm).unwrap_or(chars.len());
        let lo = s.min(e);
        let hi = s.max(e);
        let out: String = chars[lo..hi].iter().collect();
        Ok(ActionResult::from(Value::String(out)))
    }
}

pub struct RepeatAction;
#[derive(Deserialize)]
struct RepIn {
    text: String,
    times: u32,
}
#[async_trait]
impl Action for RepeatAction {
    fn id(&self) -> &'static str {
        "string.repeat"
    }
    fn summary(&self) -> &'static str {
        "Repeat `text` n times"
    }
    fn schema(&self) -> &'static Value {
        static S: Lazy<Value> = Lazy::new(|| {
            serde_json::json!({
                "type": "object",
                "required": ["text", "times"],
                "properties": {
                    "text":  { "type": "string" },
                    "times": { "type": "integer", "minimum": 0 }
                },
                "additionalProperties": false
            })
        });
        &S
    }
    async fn execute(&self, _ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let RepIn { text, times } = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("string.repeat invalid: {e}")))?;
        Ok(ActionResult::from(Value::String(
            text.repeat(times as usize),
        )))
    }
}

#[derive(Deserialize)]
struct PadIn {
    text: String,
    width: usize,
    #[serde(default = "default_pad")]
    pad: String,
}
fn default_pad() -> String {
    " ".into()
}

pub struct PadLeftAction;
pub struct PadRightAction;

fn pad_schema() -> &'static Value {
    static S: Lazy<Value> = Lazy::new(|| {
        serde_json::json!({
            "type": "object",
            "required": ["text", "width"],
            "properties": {
                "text":  { "type": "string" },
                "width": { "type": "integer", "minimum": 0 },
                "pad":   { "type": "string", "default": " " }
            },
            "additionalProperties": false
        })
    });
    &S
}

fn do_pad(text: &str, width: usize, pad: &str, left: bool) -> String {
    let len = text.chars().count();
    if len >= width || pad.is_empty() {
        return text.to_string();
    }
    let pad_chars: Vec<char> = pad.chars().collect();
    let need = width - len;
    let mut prefix = String::new();
    for i in 0..need {
        prefix.push(pad_chars[i % pad_chars.len()]);
    }
    if left {
        format!("{prefix}{text}")
    } else {
        format!("{text}{prefix}")
    }
}

#[async_trait]
impl Action for PadLeftAction {
    fn id(&self) -> &'static str {
        "string.pad_left"
    }
    fn summary(&self) -> &'static str {
        "Pad string to fixed width from the left"
    }
    fn schema(&self) -> &'static Value {
        pad_schema()
    }
    async fn execute(&self, _ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let PadIn { text, width, pad } = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("string.pad_left invalid: {e}")))?;
        Ok(ActionResult::from(Value::String(do_pad(
            &text, width, &pad, true,
        ))))
    }
}

#[async_trait]
impl Action for PadRightAction {
    fn id(&self) -> &'static str {
        "string.pad_right"
    }
    fn summary(&self) -> &'static str {
        "Pad string to fixed width from the right"
    }
    fn schema(&self) -> &'static Value {
        pad_schema()
    }
    async fn execute(&self, _ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let PadIn { text, width, pad } = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("string.pad_right invalid: {e}")))?;
        Ok(ActionResult::from(Value::String(do_pad(
            &text, width, &pad, false,
        ))))
    }
}

pub struct FormatAction;
#[derive(Deserialize)]
struct FmtIn {
    template: String,
    #[serde(default)]
    values: serde_json::Map<String, Value>,
}
#[async_trait]
impl Action for FormatAction {
    fn id(&self) -> &'static str {
        "string.format"
    }
    fn summary(&self) -> &'static str {
        "Replace {key} placeholders in template"
    }
    fn schema(&self) -> &'static Value {
        static S: Lazy<Value> = Lazy::new(|| {
            serde_json::json!({
                "type": "object",
                "required": ["template"],
                "properties": {
                    "template": { "type": "string" },
                    "values":   { "type": "object" }
                },
                "additionalProperties": false
            })
        });
        &S
    }
    async fn execute(&self, _ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let FmtIn { template, values } = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("string.format invalid: {e}")))?;
        let mut out = template;
        for (k, v) in &values {
            let needle = format!("{{{k}}}");
            let rep = match v {
                Value::String(s) => s.clone(),
                other => other.to_string(),
            };
            out = out.replace(&needle, &rep);
        }
        Ok(ActionResult::from(Value::String(out)))
    }
}
