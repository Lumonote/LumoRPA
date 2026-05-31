//! Data-table actions (`data.filter` / `data.group_by` / `data.join`) — F-12.
//!
//! Pure-logic predicates and reshaping over arrays of objects: no capability
//! gate, no new dependencies. Numeric comparisons reuse
//! [`crate::list_ops::cmp_value`] so ordering matches `list.sort` (numbers
//! compare by value, never lexically).

use crate::list_ops::cmp_value;
use async_trait::async_trait;
use lumo_core::error::StepError;
use lumo_core::{Action, ActionRegistry, ActionResult, StepCtx};
use once_cell::sync::Lazy;
use serde::Deserialize;
use serde_json::Value;
use std::cmp::Ordering;

pub fn register(r: &mut ActionRegistry) {
    r.register(FilterAction);
}

// ---- data.filter ----------------------------------------------------------

pub struct FilterAction;

#[derive(Deserialize)]
struct FilterIn {
    items: Vec<Value>,
    #[serde(rename = "where", default)]
    predicates: Vec<Predicate>,
}

#[derive(Deserialize)]
struct Predicate {
    field: String,
    op: String,
    #[serde(default)]
    value: Value,
}

#[async_trait]
impl Action for FilterAction {
    fn id(&self) -> &'static str {
        "data.filter"
    }
    fn summary(&self) -> &'static str {
        "Filter an array of objects by AND-combined field predicates"
    }
    fn schema(&self) -> &'static Value {
        static S: Lazy<Value> = Lazy::new(|| {
            serde_json::json!({
                "type": "object",
                "required": ["items"],
                "properties": {
                    "items": { "type": "array" },
                    "where": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "required": ["field", "op"],
                            "properties": {
                                "field": { "type": "string" },
                                "op": {
                                    "type": "string",
                                    "enum": [
                                        "eq", "ne", "gt", "gte", "lt", "lte",
                                        "contains", "starts_with", "ends_with",
                                        "in", "not_in", "exists", "not_exists"
                                    ]
                                },
                                "value": {}
                            },
                            "additionalProperties": false
                        }
                    }
                },
                "additionalProperties": false
            })
        });
        &S
    }
    async fn execute(&self, _ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let FilterIn { items, predicates } = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("data.filter invalid: {e}")))?;
        let mut out = Vec::new();
        for row in items {
            let mut keep = true;
            for p in &predicates {
                if !eval_predicate(&row, p)? {
                    keep = false;
                    break;
                }
            }
            if keep {
                out.push(row);
            }
        }
        Ok(ActionResult::from(Value::Array(out)))
    }
}

/// Evaluate one predicate against `row`.
///
/// `exists`/`not_exists` test key presence. For every value operator a missing
/// field never matches (the row is dropped rather than erroring). An unknown
/// operator is a hard error.
fn eval_predicate(row: &Value, p: &Predicate) -> Result<bool, StepError> {
    let field = row.get(&p.field);
    match p.op.as_str() {
        "exists" => return Ok(field.is_some()),
        "not_exists" => return Ok(field.is_none()),
        _ => {}
    }
    // Value operators: a missing field never matches.
    let fv = match field {
        Some(v) => v,
        None => return Ok(false),
    };
    let matched = match p.op.as_str() {
        "eq" => cmp_value(fv, &p.value) == Ordering::Equal,
        "ne" => cmp_value(fv, &p.value) != Ordering::Equal,
        "gt" => cmp_value(fv, &p.value) == Ordering::Greater,
        "gte" => cmp_value(fv, &p.value) != Ordering::Less,
        "lt" => cmp_value(fv, &p.value) == Ordering::Less,
        "lte" => cmp_value(fv, &p.value) != Ordering::Greater,
        "contains" => value_contains(fv, &p.value),
        "starts_with" => match (fv.as_str(), p.value.as_str()) {
            (Some(s), Some(pat)) => s.starts_with(pat),
            _ => false,
        },
        "ends_with" => match (fv.as_str(), p.value.as_str()) {
            (Some(s), Some(pat)) => s.ends_with(pat),
            _ => false,
        },
        "in" => value_in(fv, &p.value),
        "not_in" => !value_in(fv, &p.value),
        other => {
            return Err(StepError::msg(format!(
                "data.filter: unknown operator `{other}`"
            )))
        }
    };
    Ok(matched)
}

/// `contains`: substring test for strings, element membership for arrays,
/// otherwise no match.
fn value_contains(field: &Value, needle: &Value) -> bool {
    match field {
        Value::String(s) => needle.as_str().map(|n| s.contains(n)).unwrap_or(false),
        Value::Array(arr) => arr.iter().any(|e| cmp_value(e, needle) == Ordering::Equal),
        _ => false,
    }
}

/// `in`: whether `field` equals any element of the `set` array (numeric-aware
/// via [`cmp_value`]). A non-array `set` matches nothing.
fn value_in(field: &Value, set: &Value) -> bool {
    match set {
        Value::Array(arr) => arr.iter().any(|e| cmp_value(field, e) == Ordering::Equal),
        _ => false,
    }
}
