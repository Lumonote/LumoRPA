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
use std::collections::{BTreeMap, HashMap};

pub fn register(r: &mut ActionRegistry) {
    r.register(FilterAction);
    r.register(GroupByAction);
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

// ---- data.group_by --------------------------------------------------------

pub struct GroupByAction;

#[derive(Deserialize)]
struct GroupByIn {
    items: Vec<Value>,
    by: ByFields,
    #[serde(default)]
    aggregations: BTreeMap<String, AggSpec>,
}

/// `by` accepts a single field name or a list of them.
#[derive(Deserialize)]
#[serde(untagged)]
enum ByFields {
    One(String),
    Many(Vec<String>),
}

impl ByFields {
    fn into_vec(self) -> Vec<String> {
        match self {
            ByFields::One(s) => vec![s],
            ByFields::Many(v) => v,
        }
    }
}

#[derive(Deserialize)]
struct AggSpec {
    op: String,
    #[serde(default)]
    field: Option<String>,
}

#[async_trait]
impl Action for GroupByAction {
    fn id(&self) -> &'static str {
        "data.group_by"
    }
    fn summary(&self) -> &'static str {
        "Group an array of objects by one or more fields with aggregations"
    }
    fn schema(&self) -> &'static Value {
        static S: Lazy<Value> = Lazy::new(|| {
            serde_json::json!({
                "type": "object",
                "required": ["items", "by"],
                "properties": {
                    "items": { "type": "array" },
                    "by": {
                        "oneOf": [
                            { "type": "string" },
                            { "type": "array", "items": { "type": "string" } }
                        ]
                    },
                    "aggregations": {
                        "type": "object",
                        "additionalProperties": {
                            "type": "object",
                            "required": ["op"],
                            "properties": {
                                "op": {
                                    "type": "string",
                                    "enum": [
                                        "count", "sum", "avg", "min", "max",
                                        "first", "last", "collect"
                                    ]
                                },
                                "field": { "type": "string" }
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
        let GroupByIn {
            items,
            by,
            aggregations,
        } = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("data.group_by invalid: {e}")))?;
        let by = by.into_vec();

        // Preserve first-seen group order: `order` keeps the group identity
        // strings in insertion order; `groups` maps each to its (key values,
        // member rows).
        let mut order: Vec<String> = Vec::new();
        let mut groups: HashMap<String, (Vec<Value>, Vec<Value>)> = HashMap::new();
        for row in items {
            let key_vals: Vec<Value> = by
                .iter()
                .map(|f| row.get(f).cloned().unwrap_or(Value::Null))
                .collect();
            let kstr = Value::Array(key_vals.clone()).to_string();
            if let Some(g) = groups.get_mut(&kstr) {
                g.1.push(row);
            } else {
                order.push(kstr.clone());
                groups.insert(kstr, (key_vals, vec![row]));
            }
        }

        let mut out = Vec::with_capacity(order.len());
        for kstr in &order {
            let (key_vals, rows) = &groups[kstr];
            let mut obj = serde_json::Map::new();
            for (f, v) in by.iter().zip(key_vals.iter()) {
                obj.insert(f.clone(), v.clone());
            }
            for (name, spec) in &aggregations {
                obj.insert(name.clone(), aggregate(spec, rows)?);
            }
            out.push(Value::Object(obj));
        }
        Ok(ActionResult::from(Value::Array(out)))
    }
}

/// Compute one aggregation over a group's member `rows`.
fn aggregate(spec: &AggSpec, rows: &[Value]) -> Result<Value, StepError> {
    match spec.op.as_str() {
        "count" => Ok(Value::from(rows.len() as u64)),
        "sum" => {
            let field = agg_field(spec)?;
            let mut total = 0.0;
            for r in rows {
                if let Some(v) = r.get(field) {
                    total += as_number(v, "sum", field)?;
                }
            }
            Ok(num(total))
        }
        "avg" => {
            let field = agg_field(spec)?;
            let mut total = 0.0;
            let mut count = 0u64;
            for r in rows {
                if let Some(v) = r.get(field) {
                    total += as_number(v, "avg", field)?;
                    count += 1;
                }
            }
            Ok(if count == 0 {
                Value::Null
            } else {
                num(total / count as f64)
            })
        }
        "min" | "max" => {
            let field = agg_field(spec)?;
            let want_min = spec.op == "min";
            let mut best: Option<&Value> = None;
            for r in rows {
                if let Some(v) = r.get(field) {
                    best = Some(match best {
                        None => v,
                        Some(b) => {
                            let ord = cmp_value(v, b);
                            if (want_min && ord == Ordering::Less)
                                || (!want_min && ord == Ordering::Greater)
                            {
                                v
                            } else {
                                b
                            }
                        }
                    });
                }
            }
            Ok(best.cloned().unwrap_or(Value::Null))
        }
        "first" | "last" => {
            let field = agg_field(spec)?;
            let row = if spec.op == "first" {
                rows.first()
            } else {
                rows.last()
            };
            Ok(row
                .and_then(|r| r.get(field).cloned())
                .unwrap_or(Value::Null))
        }
        "collect" => {
            let field = agg_field(spec)?;
            let arr: Vec<Value> = rows
                .iter()
                .map(|r| r.get(field).cloned().unwrap_or(Value::Null))
                .collect();
            Ok(Value::Array(arr))
        }
        other => Err(StepError::msg(format!(
            "data.group_by: unknown aggregation op `{other}`"
        ))),
    }
}

/// Resolve the `field` an aggregation operates on (required for everything but
/// `count`).
fn agg_field(spec: &AggSpec) -> Result<&str, StepError> {
    spec.field.as_deref().ok_or_else(|| {
        StepError::msg(format!(
            "data.group_by: `{}` aggregation requires `field`",
            spec.op
        ))
    })
}

/// Coerce a JSON value to f64 for numeric aggregations, erroring on non-numbers.
fn as_number(v: &Value, op: &str, field: &str) -> Result<f64, StepError> {
    v.as_f64().ok_or_else(|| {
        StepError::msg(format!(
            "data.group_by: `{op}` on non-number field `{field}`"
        ))
    })
}

/// Render an f64 as an integer `Value` when it is exactly whole (so sums/avgs
/// over integer columns stay integers), else as a float. NaN/∞ → null.
fn num(f: f64) -> Value {
    if f.is_finite() && f.fract() == 0.0 && f.abs() < 9_007_199_254_740_992.0 {
        Value::from(f as i64)
    } else {
        serde_json::Number::from_f64(f)
            .map(Value::Number)
            .unwrap_or(Value::Null)
    }
}
