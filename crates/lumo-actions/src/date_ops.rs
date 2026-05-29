//! Date/time actions (`date.*`). All ops use `chrono` and operate on RFC3339
//! / ISO-8601 strings on the wire so flows stay JSON-friendly.

use async_trait::async_trait;
use chrono::{DateTime, Datelike, Duration, NaiveDate, NaiveDateTime, TimeZone, Utc};
use lumo_core::error::StepError;
use lumo_core::{Action, ActionRegistry, ActionResult, StepCtx};
use once_cell::sync::Lazy;
use serde::Deserialize;
use serde_json::Value;

pub fn register(r: &mut ActionRegistry) {
    r.register(NowAction);
    r.register(ParseAction);
    r.register(FormatAction);
    r.register(AddAction);
    r.register(DiffAction);
    r.register(WeekdayAction);
}

fn parse_any(value: &str) -> Result<DateTime<Utc>, StepError> {
    if let Ok(dt) = DateTime::parse_from_rfc3339(value) {
        return Ok(dt.with_timezone(&Utc));
    }
    for fmt in ["%Y-%m-%d %H:%M:%S", "%Y-%m-%dT%H:%M:%S", "%Y/%m/%d %H:%M:%S"] {
        if let Ok(ndt) = NaiveDateTime::parse_from_str(value, fmt) {
            return Ok(Utc.from_utc_datetime(&ndt));
        }
    }
    if let Ok(date) = NaiveDate::parse_from_str(value, "%Y-%m-%d") {
        let ndt = date.and_hms_opt(0, 0, 0).unwrap();
        return Ok(Utc.from_utc_datetime(&ndt));
    }
    Err(StepError::msg(format!(
        "date.parse: cannot parse `{value}` (try RFC3339)"
    )))
}

pub struct NowAction;
#[derive(Deserialize, Default)]
struct NowIn {
    #[serde(default)]
    format: Option<String>,
}
#[async_trait]
impl Action for NowAction {
    fn id(&self) -> &'static str { "date.now" }
    fn summary(&self) -> &'static str { "Current UTC timestamp (RFC3339 or custom strftime)" }
    fn schema(&self) -> &'static Value {
        static S: Lazy<Value> = Lazy::new(|| serde_json::json!({
            "type": "object",
            "properties": { "format": { "type": "string" } },
            "additionalProperties": false
        }));
        &S
    }
    async fn execute(&self, _ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let NowIn { format } = if input.is_null() {
            NowIn::default()
        } else {
            serde_json::from_value(input)
                .map_err(|e| StepError::msg(format!("date.now invalid: {e}")))?
        };
        let now = Utc::now();
        let out = match format.as_deref() {
            None | Some("") | Some("rfc3339") => now.to_rfc3339(),
            Some(f) => now.format(f).to_string(),
        };
        Ok(ActionResult::from(Value::String(out)))
    }
}

pub struct ParseAction;
#[derive(Deserialize)]
struct ParseIn {
    value: String,
}
#[async_trait]
impl Action for ParseAction {
    fn id(&self) -> &'static str { "date.parse" }
    fn summary(&self) -> &'static str { "Normalize a date string into RFC3339 UTC" }
    fn schema(&self) -> &'static Value {
        static S: Lazy<Value> = Lazy::new(|| serde_json::json!({
            "type": "object",
            "required": ["value"],
            "properties": { "value": { "type": "string" } },
            "additionalProperties": false
        }));
        &S
    }
    async fn execute(&self, _ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let ParseIn { value } = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("date.parse invalid: {e}")))?;
        let dt = parse_any(&value)?;
        Ok(ActionResult::from(Value::String(dt.to_rfc3339())))
    }
}

pub struct FormatAction;
#[derive(Deserialize)]
struct FmtIn {
    value: String,
    format: String,
}
#[async_trait]
impl Action for FormatAction {
    fn id(&self) -> &'static str { "date.format" }
    fn summary(&self) -> &'static str { "Format an RFC3339 timestamp via strftime" }
    fn schema(&self) -> &'static Value {
        static S: Lazy<Value> = Lazy::new(|| serde_json::json!({
            "type": "object",
            "required": ["value", "format"],
            "properties": {
                "value":  { "type": "string" },
                "format": { "type": "string" }
            },
            "additionalProperties": false
        }));
        &S
    }
    async fn execute(&self, _ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let FmtIn { value, format } = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("date.format invalid: {e}")))?;
        let dt = parse_any(&value)?;
        Ok(ActionResult::from(Value::String(dt.format(&format).to_string())))
    }
}

pub struct AddAction;
#[derive(Deserialize)]
struct AddIn {
    value: String,
    #[serde(default)]
    days: i64,
    #[serde(default)]
    hours: i64,
    #[serde(default)]
    minutes: i64,
    #[serde(default)]
    seconds: i64,
}
#[async_trait]
impl Action for AddAction {
    fn id(&self) -> &'static str { "date.add" }
    fn summary(&self) -> &'static str { "Offset a timestamp by days/hours/minutes/seconds (may be negative)" }
    fn schema(&self) -> &'static Value {
        static S: Lazy<Value> = Lazy::new(|| serde_json::json!({
            "type": "object",
            "required": ["value"],
            "properties": {
                "value":   { "type": "string" },
                "days":    { "type": "integer" },
                "hours":   { "type": "integer" },
                "minutes": { "type": "integer" },
                "seconds": { "type": "integer" }
            },
            "additionalProperties": false
        }));
        &S
    }
    async fn execute(&self, _ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let AddIn { value, days, hours, minutes, seconds } = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("date.add invalid: {e}")))?;
        let dt = parse_any(&value)?;
        let d = dt + Duration::days(days)
            + Duration::hours(hours)
            + Duration::minutes(minutes)
            + Duration::seconds(seconds);
        Ok(ActionResult::from(Value::String(d.to_rfc3339())))
    }
}

pub struct DiffAction;
#[derive(Deserialize)]
struct DiffIn {
    a: String,
    b: String,
    #[serde(default = "default_unit")]
    unit: String,
}
fn default_unit() -> String { "seconds".into() }

#[async_trait]
impl Action for DiffAction {
    fn id(&self) -> &'static str { "date.diff" }
    fn summary(&self) -> &'static str { "Return a - b in the chosen unit (days/hours/minutes/seconds)" }
    fn schema(&self) -> &'static Value {
        static S: Lazy<Value> = Lazy::new(|| serde_json::json!({
            "type": "object",
            "required": ["a", "b"],
            "properties": {
                "a":    { "type": "string" },
                "b":    { "type": "string" },
                "unit": { "type": "string", "enum": ["days","hours","minutes","seconds"], "default": "seconds" }
            },
            "additionalProperties": false
        }));
        &S
    }
    async fn execute(&self, _ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let DiffIn { a, b, unit } = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("date.diff invalid: {e}")))?;
        let da = parse_any(&a)?;
        let db = parse_any(&b)?;
        let secs = (da - db).num_seconds();
        let out = match unit.as_str() {
            "days"    => secs as f64 / 86_400.0,
            "hours"   => secs as f64 / 3_600.0,
            "minutes" => secs as f64 / 60.0,
            _ => secs as f64,
        };
        Ok(ActionResult::from(
            serde_json::Number::from_f64(out)
                .map(Value::Number)
                .unwrap_or(Value::Null),
        ))
    }
}

pub struct WeekdayAction;
#[derive(Deserialize)]
struct WIn { value: String }
#[async_trait]
impl Action for WeekdayAction {
    fn id(&self) -> &'static str { "date.weekday" }
    fn summary(&self) -> &'static str { "Return weekday (1=Mon..7=Sun) for the given date" }
    fn schema(&self) -> &'static Value {
        static S: Lazy<Value> = Lazy::new(|| serde_json::json!({
            "type": "object",
            "required": ["value"],
            "properties": { "value": { "type": "string" } },
            "additionalProperties": false
        }));
        &S
    }
    async fn execute(&self, _ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let WIn { value } = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("date.weekday invalid: {e}")))?;
        let dt = parse_any(&value)?;
        Ok(ActionResult::from(Value::from(
            dt.weekday().number_from_monday() as u64,
        )))
    }
}
