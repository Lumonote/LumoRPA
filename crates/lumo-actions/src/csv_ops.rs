//! CSV parse / write (`csv.*`). Pure Rust, no extra crate — handles quoted
//! fields with `""` escapes and CRLF line endings.

use async_trait::async_trait;
use lumo_core::error::StepError;
use lumo_core::{Action, ActionRegistry, ActionResult, StepCtx};
use once_cell::sync::Lazy;
use serde::Deserialize;
use serde_json::{Map, Value};
use std::path::PathBuf;

pub fn register(r: &mut ActionRegistry) {
    r.register(ParseAction);
    r.register(StringifyAction);
    r.register(ReadAction);
    r.register(WriteAction);
}

fn parse_csv(text: &str, sep: char) -> Vec<Vec<String>> {
    let mut rows: Vec<Vec<String>> = Vec::new();
    let mut row: Vec<String> = Vec::new();
    let mut field = String::new();
    let mut in_quotes = false;
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if in_quotes {
            if c == '"' {
                if chars.peek() == Some(&'"') {
                    field.push('"');
                    chars.next();
                } else {
                    in_quotes = false;
                }
            } else {
                field.push(c);
            }
            continue;
        }
        match c {
            '"' => in_quotes = true,
            '\r' => {} // swallow; \n handles row terminator
            '\n' => {
                row.push(std::mem::take(&mut field));
                rows.push(std::mem::take(&mut row));
            }
            ch if ch == sep => {
                row.push(std::mem::take(&mut field));
            }
            _ => field.push(c),
        }
    }
    if !field.is_empty() || !row.is_empty() {
        row.push(field);
        rows.push(row);
    }
    rows
}

fn stringify_csv(rows: &[Vec<String>], sep: char) -> String {
    let mut out = String::new();
    for row in rows {
        let line: Vec<String> = row
            .iter()
            .map(|f| {
                if f.contains(sep) || f.contains('"') || f.contains('\n') {
                    let esc = f.replace('"', "\"\"");
                    format!("\"{esc}\"")
                } else {
                    f.clone()
                }
            })
            .collect();
        out.push_str(&line.join(&sep.to_string()));
        out.push('\n');
    }
    out
}

fn rows_to_value(rows: Vec<Vec<String>>, headers: bool) -> Value {
    if !headers {
        return Value::Array(
            rows.into_iter()
                .map(|r| Value::Array(r.into_iter().map(Value::String).collect()))
                .collect(),
        );
    }
    let mut iter = rows.into_iter();
    let Some(header) = iter.next() else {
        return Value::Array(Vec::new());
    };
    let body: Vec<Value> = iter
        .map(|row| {
            let mut m = Map::new();
            for (i, key) in header.iter().enumerate() {
                let v = row.get(i).cloned().unwrap_or_default();
                m.insert(key.clone(), Value::String(v));
            }
            Value::Object(m)
        })
        .collect();
    Value::Array(body)
}

fn value_to_rows(value: &Value, headers: Option<&Vec<String>>) -> Result<Vec<Vec<String>>, StepError> {
    let arr = value
        .as_array()
        .ok_or_else(|| StepError::msg("csv input: expected an array"))?;
    if arr.is_empty() {
        return Ok(Vec::new());
    }
    // Branch: array-of-arrays vs array-of-objects.
    if arr.iter().all(|v| v.is_array()) {
        let mut out = Vec::with_capacity(arr.len());
        for row in arr {
            let inner: Vec<String> = row
                .as_array()
                .unwrap()
                .iter()
                .map(stringify_scalar)
                .collect();
            out.push(inner);
        }
        return Ok(out);
    }
    if arr.iter().all(|v| v.is_object()) {
        let cols: Vec<String> = if let Some(h) = headers {
            h.clone()
        } else {
            let mut seen = std::collections::BTreeSet::new();
            let mut order: Vec<String> = Vec::new();
            for v in arr {
                if let Some(m) = v.as_object() {
                    for k in m.keys() {
                        if seen.insert(k.clone()) {
                            order.push(k.clone());
                        }
                    }
                }
            }
            order
        };
        let mut out: Vec<Vec<String>> = Vec::with_capacity(arr.len() + 1);
        out.push(cols.clone());
        for v in arr {
            let m = v.as_object().unwrap();
            let row: Vec<String> = cols
                .iter()
                .map(|k| stringify_scalar(m.get(k).unwrap_or(&Value::Null)))
                .collect();
            out.push(row);
        }
        return Ok(out);
    }
    Err(StepError::msg(
        "csv input: expected array of arrays or array of objects",
    ))
}

fn stringify_scalar(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

pub struct ParseAction;
#[derive(Deserialize)]
struct ParseIn {
    text: String,
    #[serde(default = "default_sep")]
    sep: char,
    #[serde(default)]
    headers: bool,
}
fn default_sep() -> char { ',' }

#[async_trait]
impl Action for ParseAction {
    fn id(&self) -> &'static str { "csv.parse" }
    fn summary(&self) -> &'static str { "Parse CSV text; with `headers: true` returns array of objects" }
    fn schema(&self) -> &'static Value {
        static S: Lazy<Value> = Lazy::new(|| serde_json::json!({
            "type": "object",
            "required": ["text"],
            "properties": {
                "text":    { "type": "string" },
                "sep":     { "type": "string", "default": "," },
                "headers": { "type": "boolean", "default": false }
            },
            "additionalProperties": false
        }));
        &S
    }
    async fn execute(&self, _ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let ParseIn { text, sep, headers } = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("csv.parse invalid: {e}")))?;
        let rows = parse_csv(&text, sep);
        Ok(ActionResult::from(rows_to_value(rows, headers)))
    }
}

pub struct StringifyAction;
#[derive(Deserialize)]
struct StringifyIn {
    value: Value,
    #[serde(default = "default_sep")]
    sep: char,
    #[serde(default)]
    headers: Option<Vec<String>>,
}
#[async_trait]
impl Action for StringifyAction {
    fn id(&self) -> &'static str { "csv.stringify" }
    fn summary(&self) -> &'static str { "Render array-of-array or array-of-object as CSV text" }
    fn schema(&self) -> &'static Value {
        static S: Lazy<Value> = Lazy::new(|| serde_json::json!({
            "type": "object",
            "required": ["value"],
            "properties": {
                "value":   {},
                "sep":     { "type": "string", "default": "," },
                "headers": { "type": "array", "items": { "type": "string" } }
            },
            "additionalProperties": false
        }));
        &S
    }
    async fn execute(&self, _ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let StringifyIn { value, sep, headers } = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("csv.stringify invalid: {e}")))?;
        let rows = value_to_rows(&value, headers.as_ref())?;
        Ok(ActionResult::from(Value::String(stringify_csv(&rows, sep))))
    }
}

pub struct ReadAction;
#[derive(Deserialize)]
struct ReadIn {
    path: PathBuf,
    #[serde(default = "default_sep")]
    sep: char,
    #[serde(default)]
    headers: bool,
}
#[async_trait]
impl Action for ReadAction {
    fn id(&self) -> &'static str { "csv.read" }
    fn summary(&self) -> &'static str { "Read a CSV file and parse to rows / objects" }
    fn schema(&self) -> &'static Value {
        static S: Lazy<Value> = Lazy::new(|| serde_json::json!({
            "type": "object",
            "required": ["path"],
            "properties": {
                "path":    { "type": "string" },
                "sep":     { "type": "string", "default": "," },
                "headers": { "type": "boolean", "default": false }
            },
            "additionalProperties": false
        }));
        &S
    }
    async fn execute(&self, ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let ReadIn { path, sep, headers } = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("csv.read invalid: {e}")))?;
        ctx.ensure_fs_read(&path)?;
        let text = tokio::fs::read_to_string(&path)
            .await
            .map_err(|e| StepError::msg(format!("read {}: {e}", path.display())))?;
        Ok(ActionResult::from(rows_to_value(parse_csv(&text, sep), headers)))
    }
}

pub struct WriteAction;
#[derive(Deserialize)]
struct WriteIn {
    path: PathBuf,
    value: Value,
    #[serde(default = "default_sep")]
    sep: char,
    #[serde(default)]
    headers: Option<Vec<String>>,
}
#[async_trait]
impl Action for WriteAction {
    fn id(&self) -> &'static str { "csv.write" }
    fn summary(&self) -> &'static str { "Write `value` (array of arrays/objects) to a CSV file" }
    fn schema(&self) -> &'static Value {
        static S: Lazy<Value> = Lazy::new(|| serde_json::json!({
            "type": "object",
            "required": ["path", "value"],
            "properties": {
                "path":    { "type": "string" },
                "value":   {},
                "sep":     { "type": "string", "default": "," },
                "headers": { "type": "array", "items": { "type": "string" } }
            },
            "additionalProperties": false
        }));
        &S
    }
    async fn execute(&self, ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let WriteIn { path, value, sep, headers } = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("csv.write invalid: {e}")))?;
        ctx.ensure_fs_write(&path)?;
        let rows = value_to_rows(&value, headers.as_ref())?;
        let text = stringify_csv(&rows, sep);
        if let Some(parent) = path.parent() {
            let _ = tokio::fs::create_dir_all(parent).await;
        }
        tokio::fs::write(&path, text.as_bytes())
            .await
            .map_err(|e| StepError::msg(format!("write {}: {e}", path.display())))?;
        Ok(ActionResult::from(serde_json::json!({
            "path": path,
            "rows": rows.len(),
        })))
    }
}
