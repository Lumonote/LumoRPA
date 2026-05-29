//! Math actions (`math.*`). Small calculator-style helpers.

use async_trait::async_trait;
use lumo_core::error::StepError;
use lumo_core::{Action, ActionRegistry, ActionResult, StepCtx};
use once_cell::sync::Lazy;
use serde::Deserialize;
use serde_json::Value;

pub fn register(r: &mut ActionRegistry) {
    r.register(RoundAction);
    r.register(RandomAction);
    r.register(MinAction);
    r.register(MaxAction);
    r.register(SumAction);
    r.register(AvgAction);
    r.register(AbsAction);
}

fn n(v: &Value) -> Option<f64> {
    v.as_f64().or_else(|| v.as_i64().map(|i| i as f64))
}

pub struct RoundAction;
#[derive(Deserialize)]
struct RoundIn {
    value: f64,
    #[serde(default)]
    digits: i32,
}
#[async_trait]
impl Action for RoundAction {
    fn id(&self) -> &'static str { "math.round" }
    fn summary(&self) -> &'static str { "Round a number to N digits (default 0)" }
    fn schema(&self) -> &'static Value {
        static S: Lazy<Value> = Lazy::new(|| serde_json::json!({
            "type": "object",
            "required": ["value"],
            "properties": {
                "value":  { "type": "number" },
                "digits": { "type": "integer", "default": 0 }
            },
            "additionalProperties": false
        }));
        &S
    }
    async fn execute(&self, _ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let RoundIn { value, digits } = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("math.round invalid: {e}")))?;
        let factor = 10f64.powi(digits);
        let rounded = (value * factor).round() / factor;
        Ok(ActionResult::from(
            serde_json::Number::from_f64(rounded)
                .map(Value::Number)
                .unwrap_or(Value::Null),
        ))
    }
}

pub struct RandomAction;
#[derive(Deserialize, Default)]
struct RandIn {
    #[serde(default)]
    min: Option<f64>,
    #[serde(default)]
    max: Option<f64>,
    #[serde(default)]
    integer: bool,
}
#[async_trait]
impl Action for RandomAction {
    fn id(&self) -> &'static str { "math.random" }
    fn summary(&self) -> &'static str { "Random number in [min, max) (default [0,1) float)" }
    fn schema(&self) -> &'static Value {
        static S: Lazy<Value> = Lazy::new(|| serde_json::json!({
            "type": "object",
            "properties": {
                "min":     { "type": "number" },
                "max":     { "type": "number" },
                "integer": { "type": "boolean", "default": false }
            },
            "additionalProperties": false
        }));
        &S
    }
    async fn execute(&self, _ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let RandIn { min, max, integer } = if input.is_null() {
            RandIn::default()
        } else {
            serde_json::from_value(input)
                .map_err(|e| StepError::msg(format!("math.random invalid: {e}")))?
        };
        let lo = min.unwrap_or(0.0);
        let hi = max.unwrap_or(1.0);
        if hi <= lo {
            return Err(StepError::msg("math.random: max must be > min"));
        }
        // Simple LCG-quality randomness sourced from system time + uuid.
        let r: f64 = {
            let bytes = uuid::Uuid::new_v4().as_bytes()[..8].try_into().unwrap();
            let n = u64::from_le_bytes(bytes);
            (n as f64) / (u64::MAX as f64)
        };
        let value = lo + (hi - lo) * r;
        if integer {
            Ok(ActionResult::from(Value::from(value.floor() as i64)))
        } else {
            Ok(ActionResult::from(
                serde_json::Number::from_f64(value)
                    .map(Value::Number)
                    .unwrap_or(Value::Null),
            ))
        }
    }
}

#[derive(Deserialize)]
struct ListIn {
    items: Vec<Value>,
}

pub struct MinAction;
#[async_trait]
impl Action for MinAction {
    fn id(&self) -> &'static str { "math.min" }
    fn summary(&self) -> &'static str { "Smallest number in `items`" }
    fn schema(&self) -> &'static Value { items_schema() }
    async fn execute(&self, _ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let ListIn { items } = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("math.min invalid: {e}")))?;
        let mut best: Option<f64> = None;
        for v in &items {
            if let Some(x) = n(v) {
                best = Some(best.map(|b| b.min(x)).unwrap_or(x));
            }
        }
        Ok(ActionResult::from(
            best.and_then(serde_json::Number::from_f64)
                .map(Value::Number)
                .unwrap_or(Value::Null),
        ))
    }
}

pub struct MaxAction;
#[async_trait]
impl Action for MaxAction {
    fn id(&self) -> &'static str { "math.max" }
    fn summary(&self) -> &'static str { "Largest number in `items`" }
    fn schema(&self) -> &'static Value { items_schema() }
    async fn execute(&self, _ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let ListIn { items } = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("math.max invalid: {e}")))?;
        let mut best: Option<f64> = None;
        for v in &items {
            if let Some(x) = n(v) {
                best = Some(best.map(|b| b.max(x)).unwrap_or(x));
            }
        }
        Ok(ActionResult::from(
            best.and_then(serde_json::Number::from_f64)
                .map(Value::Number)
                .unwrap_or(Value::Null),
        ))
    }
}

pub struct SumAction;
#[async_trait]
impl Action for SumAction {
    fn id(&self) -> &'static str { "math.sum" }
    fn summary(&self) -> &'static str { "Sum of numeric entries (non-numbers ignored)" }
    fn schema(&self) -> &'static Value { items_schema() }
    async fn execute(&self, _ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let ListIn { items } = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("math.sum invalid: {e}")))?;
        let total: f64 = items.iter().filter_map(n).sum();
        Ok(ActionResult::from(
            serde_json::Number::from_f64(total)
                .map(Value::Number)
                .unwrap_or(Value::Null),
        ))
    }
}

pub struct AvgAction;
#[async_trait]
impl Action for AvgAction {
    fn id(&self) -> &'static str { "math.avg" }
    fn summary(&self) -> &'static str { "Arithmetic mean of numeric entries" }
    fn schema(&self) -> &'static Value { items_schema() }
    async fn execute(&self, _ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let ListIn { items } = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("math.avg invalid: {e}")))?;
        let nums: Vec<f64> = items.iter().filter_map(n).collect();
        if nums.is_empty() {
            return Ok(ActionResult::from(Value::Null));
        }
        let avg = nums.iter().sum::<f64>() / nums.len() as f64;
        Ok(ActionResult::from(
            serde_json::Number::from_f64(avg)
                .map(Value::Number)
                .unwrap_or(Value::Null),
        ))
    }
}

pub struct AbsAction;
#[derive(Deserialize)]
struct AbsIn { value: f64 }
#[async_trait]
impl Action for AbsAction {
    fn id(&self) -> &'static str { "math.abs" }
    fn summary(&self) -> &'static str { "Absolute value" }
    fn schema(&self) -> &'static Value {
        static S: Lazy<Value> = Lazy::new(|| serde_json::json!({
            "type": "object",
            "required": ["value"],
            "properties": { "value": { "type": "number" } },
            "additionalProperties": false
        }));
        &S
    }
    async fn execute(&self, _ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let AbsIn { value } = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("math.abs invalid: {e}")))?;
        Ok(ActionResult::from(
            serde_json::Number::from_f64(value.abs())
                .map(Value::Number)
                .unwrap_or(Value::Null),
        ))
    }
}

fn items_schema() -> &'static Value {
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
