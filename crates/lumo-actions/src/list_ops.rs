//! List actions (`list.*`).

use async_trait::async_trait;
use lumo_core::error::StepError;
use lumo_core::{Action, ActionRegistry, ActionResult, StepCtx};
use once_cell::sync::Lazy;
use serde::Deserialize;
use serde_json::Value;
use std::collections::HashSet;

pub fn register(r: &mut ActionRegistry) {
    r.register(LengthAction);
    r.register(AppendAction);
    r.register(SortAction);
    r.register(UniqueAction);
    r.register(RangeAction);
    r.register(ContainsAction);
    r.register(GetAction);
    r.register(SliceAction);
    r.register(ReverseAction);
    r.register(PluckAction);
}

fn list_schema() -> &'static Value {
    static S: Lazy<Value> = Lazy::new(|| {
        serde_json::json!({
            "type": "object",
            "required": ["items"],
            "properties": { "items": { "type": "array" } },
            "additionalProperties": false
        })
    });
    &S
}

#[derive(Deserialize)]
struct ItemsIn {
    items: Vec<Value>,
}

pub struct LengthAction;
#[async_trait]
impl Action for LengthAction {
    fn id(&self) -> &'static str {
        "list.length"
    }
    fn summary(&self) -> &'static str {
        "Number of elements"
    }
    fn schema(&self) -> &'static Value {
        list_schema()
    }
    async fn execute(&self, _ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let ItemsIn { items } = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("list.length invalid: {e}")))?;
        Ok(ActionResult::from(Value::from(items.len() as u64)))
    }
}

pub struct AppendAction;
#[derive(Deserialize)]
struct AppendIn {
    items: Vec<Value>,
    value: Value,
}
#[async_trait]
impl Action for AppendAction {
    fn id(&self) -> &'static str {
        "list.append"
    }
    fn summary(&self) -> &'static str {
        "Return `items` with `value` appended"
    }
    fn schema(&self) -> &'static Value {
        static S: Lazy<Value> = Lazy::new(|| {
            serde_json::json!({
                "type": "object",
                "required": ["items", "value"],
                "properties": {
                    "items": { "type": "array" },
                    "value": {}
                },
                "additionalProperties": false
            })
        });
        &S
    }
    async fn execute(&self, _ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let AppendIn { mut items, value } = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("list.append invalid: {e}")))?;
        items.push(value);
        Ok(ActionResult::from(Value::Array(items)))
    }
}

pub struct SortAction;
#[derive(Deserialize)]
struct SortIn {
    items: Vec<Value>,
    #[serde(default)]
    desc: bool,
    #[serde(default)]
    by: Option<String>,
}
#[async_trait]
impl Action for SortAction {
    fn id(&self) -> &'static str {
        "list.sort"
    }
    fn summary(&self) -> &'static str {
        "Sort items; supports `by: <key>` for arrays of objects"
    }
    fn schema(&self) -> &'static Value {
        static S: Lazy<Value> = Lazy::new(|| {
            serde_json::json!({
                "type": "object",
                "required": ["items"],
                "properties": {
                    "items": { "type": "array" },
                    "desc":  { "type": "boolean", "default": false },
                    "by":    { "type": "string" }
                },
                "additionalProperties": false
            })
        });
        &S
    }
    async fn execute(&self, _ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let SortIn {
            mut items,
            desc,
            by,
        } = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("list.sort invalid: {e}")))?;
        items.sort_by(|a, b| {
            let av = by.as_ref().and_then(|k| a.get(k)).unwrap_or(a);
            let bv = by.as_ref().and_then(|k| b.get(k)).unwrap_or(b);
            cmp_value(av, bv)
        });
        if desc {
            items.reverse();
        }
        Ok(ActionResult::from(Value::Array(items)))
    }
}

fn cmp_value(a: &Value, b: &Value) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => x
            .as_f64()
            .unwrap_or(0.0)
            .partial_cmp(&y.as_f64().unwrap_or(0.0))
            .unwrap_or(Ordering::Equal),
        (Value::String(x), Value::String(y)) => x.cmp(y),
        (Value::Bool(x), Value::Bool(y)) => x.cmp(y),
        (Value::Null, Value::Null) => Ordering::Equal,
        (Value::Null, _) => Ordering::Less,
        (_, Value::Null) => Ordering::Greater,
        _ => a.to_string().cmp(&b.to_string()),
    }
}

pub struct UniqueAction;
#[async_trait]
impl Action for UniqueAction {
    fn id(&self) -> &'static str {
        "list.unique"
    }
    fn summary(&self) -> &'static str {
        "De-duplicate items preserving order"
    }
    fn schema(&self) -> &'static Value {
        list_schema()
    }
    async fn execute(&self, _ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let ItemsIn { items } = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("list.unique invalid: {e}")))?;
        let mut seen: HashSet<String> = HashSet::new();
        let mut out: Vec<Value> = Vec::with_capacity(items.len());
        for v in items {
            let k = v.to_string();
            if seen.insert(k) {
                out.push(v);
            }
        }
        Ok(ActionResult::from(Value::Array(out)))
    }
}

pub struct RangeAction;
#[derive(Deserialize)]
struct RangeIn {
    end: i64,
    #[serde(default)]
    start: i64,
    #[serde(default = "default_step")]
    step: i64,
}
fn default_step() -> i64 {
    1
}

#[async_trait]
impl Action for RangeAction {
    fn id(&self) -> &'static str {
        "list.range"
    }
    fn summary(&self) -> &'static str {
        "Generate [start, end) array with step"
    }
    fn schema(&self) -> &'static Value {
        static S: Lazy<Value> = Lazy::new(|| {
            serde_json::json!({
                "type": "object",
                "required": ["end"],
                "properties": {
                    "start": { "type": "integer", "default": 0 },
                    "end":   { "type": "integer" },
                    "step":  { "type": "integer", "default": 1 }
                },
                "additionalProperties": false
            })
        });
        &S
    }
    async fn execute(&self, _ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let RangeIn { start, end, step } = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("list.range invalid: {e}")))?;
        if step == 0 {
            return Err(StepError::msg("list.range: step must not be 0"));
        }
        let mut out = Vec::new();
        let ascending = step > 0;
        let mut i = start;
        // Hard cap so a misconfigured range can't OOM the worker.
        for _ in 0..1_000_000 {
            if (ascending && i >= end) || (!ascending && i <= end) {
                break;
            }
            out.push(Value::from(i));
            i += step;
        }
        Ok(ActionResult::from(Value::Array(out)))
    }
}

pub struct ContainsAction;
#[derive(Deserialize)]
struct ContainsIn {
    items: Vec<Value>,
    value: Value,
}
#[async_trait]
impl Action for ContainsAction {
    fn id(&self) -> &'static str {
        "list.contains"
    }
    fn summary(&self) -> &'static str {
        "Whether `items` contains `value` (deep eq via JSON)"
    }
    fn schema(&self) -> &'static Value {
        static S: Lazy<Value> = Lazy::new(|| {
            serde_json::json!({
                "type": "object",
                "required": ["items", "value"],
                "properties": {
                    "items": { "type": "array" },
                    "value": {}
                },
                "additionalProperties": false
            })
        });
        &S
    }
    async fn execute(&self, _ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let ContainsIn { items, value } = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("list.contains invalid: {e}")))?;
        Ok(ActionResult::from(Value::Bool(
            items.iter().any(|v| v == &value),
        )))
    }
}

pub struct GetAction;
#[derive(Deserialize)]
struct GetIn {
    items: Vec<Value>,
    index: i64,
}
#[async_trait]
impl Action for GetAction {
    fn id(&self) -> &'static str {
        "list.get"
    }
    fn summary(&self) -> &'static str {
        "Element at index (negatives count from end); null if out of range"
    }
    fn schema(&self) -> &'static Value {
        static S: Lazy<Value> = Lazy::new(|| {
            serde_json::json!({
                "type": "object",
                "required": ["items", "index"],
                "properties": {
                    "items": { "type": "array" },
                    "index": { "type": "integer" }
                },
                "additionalProperties": false
            })
        });
        &S
    }
    async fn execute(&self, _ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let GetIn { items, index } = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("list.get invalid: {e}")))?;
        let len = items.len() as i64;
        let idx = if index < 0 { len + index } else { index };
        let out = if idx >= 0 && idx < len {
            items[idx as usize].clone()
        } else {
            Value::Null
        };
        Ok(ActionResult::from(out))
    }
}

pub struct SliceAction;
#[derive(Deserialize)]
struct SliceIn {
    items: Vec<Value>,
    start: i64,
    #[serde(default)]
    end: Option<i64>,
}
#[async_trait]
impl Action for SliceAction {
    fn id(&self) -> &'static str {
        "list.slice"
    }
    fn summary(&self) -> &'static str {
        "Slice items[start:end]; negatives count from end"
    }
    fn schema(&self) -> &'static Value {
        static S: Lazy<Value> = Lazy::new(|| {
            serde_json::json!({
                "type": "object",
                "required": ["items", "start"],
                "properties": {
                    "items": { "type": "array" },
                    "start": { "type": "integer" },
                    "end":   { "type": "integer" }
                },
                "additionalProperties": false
            })
        });
        &S
    }
    async fn execute(&self, _ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let SliceIn { items, start, end } = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("list.slice invalid: {e}")))?;
        let len = items.len() as i64;
        let norm = |i: i64| {
            if i < 0 {
                (len + i).max(0) as usize
            } else {
                i.min(len).max(0) as usize
            }
        };
        let s = norm(start);
        let e = end.map(norm).unwrap_or(items.len());
        let lo = s.min(e);
        let hi = s.max(e);
        Ok(ActionResult::from(Value::Array(items[lo..hi].to_vec())))
    }
}

pub struct ReverseAction;
#[async_trait]
impl Action for ReverseAction {
    fn id(&self) -> &'static str {
        "list.reverse"
    }
    fn summary(&self) -> &'static str {
        "Reverse the array"
    }
    fn schema(&self) -> &'static Value {
        list_schema()
    }
    async fn execute(&self, _ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let ItemsIn { mut items } = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("list.reverse invalid: {e}")))?;
        items.reverse();
        Ok(ActionResult::from(Value::Array(items)))
    }
}

pub struct PluckAction;
#[derive(Deserialize)]
struct PluckIn {
    items: Vec<Value>,
    key: String,
}
#[async_trait]
impl Action for PluckAction {
    fn id(&self) -> &'static str {
        "list.pluck"
    }
    fn summary(&self) -> &'static str {
        "Extract `key` field from every object element"
    }
    fn schema(&self) -> &'static Value {
        static S: Lazy<Value> = Lazy::new(|| {
            serde_json::json!({
                "type": "object",
                "required": ["items", "key"],
                "properties": {
                    "items": { "type": "array" },
                    "key":   { "type": "string" }
                },
                "additionalProperties": false
            })
        });
        &S
    }
    async fn execute(&self, _ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let PluckIn { items, key } = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("list.pluck invalid: {e}")))?;
        let out: Vec<Value> = items
            .iter()
            .map(|v| v.get(&key).cloned().unwrap_or(Value::Null))
            .collect();
        Ok(ActionResult::from(Value::Array(out)))
    }
}
