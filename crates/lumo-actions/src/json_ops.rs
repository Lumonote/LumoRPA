//! Extra JSON helpers (`json.*`). Complements `data.json_parse / json_format`.

use async_trait::async_trait;
use lumo_core::error::StepError;
use lumo_core::{Action, ActionRegistry, ActionResult, StepCtx};
use once_cell::sync::Lazy;
use serde::Deserialize;
use serde_json::{Map, Value};

pub fn register(r: &mut ActionRegistry) {
    r.register(GetPathAction);
    r.register(SetPathAction);
    r.register(MergeAction);
    r.register(KeysAction);
    r.register(ValuesAction);
    r.register(DeleteAction);
}

fn split_path(p: &str) -> Vec<String> {
    if p.is_empty() {
        return Vec::new();
    }
    p.split('.').map(|s| s.to_string()).collect()
}

pub struct GetPathAction;
#[derive(Deserialize)]
struct GetIn {
    value: Value,
    path: String,
    #[serde(default)]
    default: Option<Value>,
}
#[async_trait]
impl Action for GetPathAction {
    fn id(&self) -> &'static str { "json.get" }
    fn summary(&self) -> &'static str { "Read `value` at dotted path (`a.b.0.c`)" }
    fn schema(&self) -> &'static Value {
        static S: Lazy<Value> = Lazy::new(|| serde_json::json!({
            "type": "object",
            "required": ["value", "path"],
            "properties": {
                "value":   {},
                "path":    { "type": "string" },
                "default": {}
            },
            "additionalProperties": false
        }));
        &S
    }
    async fn execute(&self, _ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let GetIn { value, path, default } = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("json.get invalid: {e}")))?;
        let mut cur = &value;
        for part in split_path(&path) {
            cur = match cur {
                Value::Object(m) => m.get(&part).unwrap_or(&Value::Null),
                Value::Array(a) => part
                    .parse::<usize>()
                    .ok()
                    .and_then(|i| a.get(i))
                    .unwrap_or(&Value::Null),
                _ => &Value::Null,
            };
            if cur.is_null() {
                break;
            }
        }
        let out = if cur.is_null() {
            default.unwrap_or(Value::Null)
        } else {
            cur.clone()
        };
        Ok(ActionResult::from(out))
    }
}

pub struct SetPathAction;
#[derive(Deserialize)]
struct SetIn {
    value: Value,
    path: String,
    new: Value,
}
#[async_trait]
impl Action for SetPathAction {
    fn id(&self) -> &'static str { "json.set" }
    fn summary(&self) -> &'static str { "Set `new` at dotted path inside `value`; returns new value" }
    fn schema(&self) -> &'static Value {
        static S: Lazy<Value> = Lazy::new(|| serde_json::json!({
            "type": "object",
            "required": ["value", "path", "new"],
            "properties": {
                "value": {},
                "path":  { "type": "string" },
                "new":   {}
            },
            "additionalProperties": false
        }));
        &S
    }
    async fn execute(&self, _ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let SetIn { mut value, path, new } = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("json.set invalid: {e}")))?;
        let parts = split_path(&path);
        if parts.is_empty() {
            return Ok(ActionResult::from(new));
        }
        set_in(&mut value, &parts, new);
        Ok(ActionResult::from(value))
    }
}

fn set_in(target: &mut Value, parts: &[String], new: Value) {
    if parts.is_empty() {
        *target = new;
        return;
    }
    let (head, tail) = (parts[0].as_str(), &parts[1..]);
    if let Ok(idx) = head.parse::<usize>() {
        if let Value::Array(arr) = target {
            while arr.len() <= idx {
                arr.push(Value::Null);
            }
            set_in(&mut arr[idx], tail, new);
            return;
        }
    }
    let map = match target {
        Value::Object(m) => m,
        _ => {
            *target = Value::Object(Map::new());
            target.as_object_mut().unwrap()
        }
    };
    let entry = map.entry(head.to_string()).or_insert(Value::Null);
    set_in(entry, tail, new);
}

pub struct MergeAction;
#[derive(Deserialize)]
struct MergeIn { a: Value, b: Value }
#[async_trait]
impl Action for MergeAction {
    fn id(&self) -> &'static str { "json.merge" }
    fn summary(&self) -> &'static str { "Shallow-merge object `b` into object `a` (b wins on key collision)" }
    fn schema(&self) -> &'static Value {
        static S: Lazy<Value> = Lazy::new(|| serde_json::json!({
            "type": "object",
            "required": ["a", "b"],
            "properties": { "a": { "type": "object" }, "b": { "type": "object" } },
            "additionalProperties": false
        }));
        &S
    }
    async fn execute(&self, _ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let MergeIn { a, b } = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("json.merge invalid: {e}")))?;
        let mut out = a.as_object().cloned().unwrap_or_default();
        if let Some(bm) = b.as_object() {
            for (k, v) in bm {
                out.insert(k.clone(), v.clone());
            }
        }
        Ok(ActionResult::from(Value::Object(out)))
    }
}

#[derive(Deserialize)]
struct ValueIn { value: Value }
fn value_schema() -> &'static Value {
    static S: Lazy<Value> = Lazy::new(|| {
        serde_json::json!({
            "type": "object",
            "required": ["value"],
            "properties": { "value": {} },
            "additionalProperties": false
        })
    });
    &S
}

pub struct KeysAction;
#[async_trait]
impl Action for KeysAction {
    fn id(&self) -> &'static str { "json.keys" }
    fn summary(&self) -> &'static str { "List keys of an object value" }
    fn schema(&self) -> &'static Value { value_schema() }
    async fn execute(&self, _ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let ValueIn { value } = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("json.keys invalid: {e}")))?;
        let out = value
            .as_object()
            .map(|m| Value::Array(m.keys().map(|k| Value::String(k.clone())).collect()))
            .unwrap_or(Value::Array(Vec::new()));
        Ok(ActionResult::from(out))
    }
}

pub struct ValuesAction;
#[async_trait]
impl Action for ValuesAction {
    fn id(&self) -> &'static str { "json.values" }
    fn summary(&self) -> &'static str { "List values of an object" }
    fn schema(&self) -> &'static Value { value_schema() }
    async fn execute(&self, _ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let ValueIn { value } = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("json.values invalid: {e}")))?;
        let out = value
            .as_object()
            .map(|m| Value::Array(m.values().cloned().collect()))
            .unwrap_or(Value::Array(Vec::new()));
        Ok(ActionResult::from(out))
    }
}

pub struct DeleteAction;
#[derive(Deserialize)]
struct DelIn { value: Value, path: String }
#[async_trait]
impl Action for DeleteAction {
    fn id(&self) -> &'static str { "json.delete" }
    fn summary(&self) -> &'static str { "Remove `path` from `value`, return result" }
    fn schema(&self) -> &'static Value {
        static S: Lazy<Value> = Lazy::new(|| serde_json::json!({
            "type": "object",
            "required": ["value", "path"],
            "properties": { "value": {}, "path": { "type": "string" } },
            "additionalProperties": false
        }));
        &S
    }
    async fn execute(&self, _ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let DelIn { mut value, path } = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("json.delete invalid: {e}")))?;
        let parts = split_path(&path);
        if parts.is_empty() {
            return Ok(ActionResult::from(Value::Null));
        }
        delete_in(&mut value, &parts);
        Ok(ActionResult::from(value))
    }
}

fn delete_in(target: &mut Value, parts: &[String]) {
    if parts.is_empty() {
        return;
    }
    let head = parts[0].as_str();
    if parts.len() == 1 {
        if let Value::Object(m) = target {
            m.remove(head);
        }
        return;
    }
    if let Value::Object(m) = target {
        if let Some(child) = m.get_mut(head) {
            delete_in(child, &parts[1..]);
        }
    }
}
