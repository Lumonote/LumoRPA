//! Date/time actions (`date.*`). All ops use `chrono` and operate on RFC3339
//! / ISO-8601 strings on the wire so flows stay JSON-friendly.

use async_trait::async_trait;
use chrono::{DateTime, Datelike, Duration, NaiveDate, NaiveDateTime, TimeZone, Utc, Weekday};
use lumo_core::error::StepError;
use lumo_core::{Action, ActionRegistry, ActionResult, StepCtx};
use once_cell::sync::Lazy;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::Value;

pub fn register(r: &mut ActionRegistry) {
    r.register(NowAction);
    r.register(ParseAction);
    r.register(FormatAction);
    r.register(AddAction);
    r.register(DiffAction);
    r.register(WeekdayAction);
    r.register(WorkdayAddAction);
}

fn parse_any(value: &str) -> Result<DateTime<Utc>, StepError> {
    if let Ok(dt) = DateTime::parse_from_rfc3339(value) {
        return Ok(dt.with_timezone(&Utc));
    }
    for fmt in [
        "%Y-%m-%d %H:%M:%S",
        "%Y-%m-%dT%H:%M:%S",
        "%Y/%m/%d %H:%M:%S",
    ] {
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
#[derive(Deserialize, Default, JsonSchema)]
#[serde(deny_unknown_fields)]
struct NowIn {
    #[serde(default)]
    format: Option<String>,
}
#[async_trait]
impl Action for NowAction {
    fn id(&self) -> &'static str {
        "date.now"
    }
    fn summary(&self) -> &'static str {
        "Current UTC timestamp (RFC3339 or custom strftime)"
    }
    fn schema(&self) -> &'static Value {
        static S: Lazy<Value> = Lazy::new(crate::schema::derive::<NowIn>);
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
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ParseIn {
    value: String,
}
#[async_trait]
impl Action for ParseAction {
    fn id(&self) -> &'static str {
        "date.parse"
    }
    fn summary(&self) -> &'static str {
        "Normalize a date string into RFC3339 UTC"
    }
    fn schema(&self) -> &'static Value {
        static S: Lazy<Value> = Lazy::new(crate::schema::derive::<ParseIn>);
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
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct FmtIn {
    value: String,
    format: String,
}
#[async_trait]
impl Action for FormatAction {
    fn id(&self) -> &'static str {
        "date.format"
    }
    fn summary(&self) -> &'static str {
        "Format an RFC3339 timestamp via strftime"
    }
    fn schema(&self) -> &'static Value {
        static S: Lazy<Value> = Lazy::new(crate::schema::derive::<FmtIn>);
        &S
    }
    async fn execute(&self, _ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let FmtIn { value, format } = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("date.format invalid: {e}")))?;
        let dt = parse_any(&value)?;
        Ok(ActionResult::from(Value::String(
            dt.format(&format).to_string(),
        )))
    }
}

pub struct AddAction;
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
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
    fn id(&self) -> &'static str {
        "date.add"
    }
    fn summary(&self) -> &'static str {
        "Offset a timestamp by days/hours/minutes/seconds (may be negative)"
    }
    fn schema(&self) -> &'static Value {
        static S: Lazy<Value> = Lazy::new(crate::schema::derive::<AddIn>);
        &S
    }
    async fn execute(&self, _ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let AddIn {
            value,
            days,
            hours,
            minutes,
            seconds,
        } = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("date.add invalid: {e}")))?;
        let dt = parse_any(&value)?;
        let d = dt
            + Duration::days(days)
            + Duration::hours(hours)
            + Duration::minutes(minutes)
            + Duration::seconds(seconds);
        Ok(ActionResult::from(Value::String(d.to_rfc3339())))
    }
}

pub struct DiffAction;
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct DiffIn {
    a: String,
    b: String,
    #[serde(default)]
    unit: DiffUnit,
}

/// Unit for `date.diff`'s result. Modeling it as an enum (not a free `String`)
/// keeps the `enum` constraint in the derived schema — the validator rejects an
/// unknown unit, which a plain `String` field could not express.
#[derive(Deserialize, JsonSchema, Default)]
#[serde(rename_all = "lowercase")]
enum DiffUnit {
    Days,
    Hours,
    Minutes,
    #[default]
    Seconds,
}

#[async_trait]
impl Action for DiffAction {
    fn id(&self) -> &'static str {
        "date.diff"
    }
    fn summary(&self) -> &'static str {
        "Return a - b in the chosen unit (days/hours/minutes/seconds)"
    }
    fn schema(&self) -> &'static Value {
        static S: Lazy<Value> = Lazy::new(crate::schema::derive::<DiffIn>);
        &S
    }
    async fn execute(&self, _ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let DiffIn { a, b, unit } = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("date.diff invalid: {e}")))?;
        let da = parse_any(&a)?;
        let db = parse_any(&b)?;
        let secs = (da - db).num_seconds();
        let out = match unit {
            DiffUnit::Days => secs as f64 / 86_400.0,
            DiffUnit::Hours => secs as f64 / 3_600.0,
            DiffUnit::Minutes => secs as f64 / 60.0,
            DiffUnit::Seconds => secs as f64,
        };
        Ok(ActionResult::from(
            serde_json::Number::from_f64(out)
                .map(Value::Number)
                .unwrap_or(Value::Null),
        ))
    }
}

pub struct WeekdayAction;
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct WIn {
    value: String,
}
#[async_trait]
impl Action for WeekdayAction {
    fn id(&self) -> &'static str {
        "date.weekday"
    }
    fn summary(&self) -> &'static str {
        "Return weekday (1=Mon..7=Sun) for the given date"
    }
    fn schema(&self) -> &'static Value {
        static S: Lazy<Value> = Lazy::new(crate::schema::derive::<WIn>);
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

pub struct WorkdayAddAction;
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct WorkdayAddIn {
    value: String,
    /// Number of business days to add (may be negative to count backwards).
    days: i64,
}
#[async_trait]
impl Action for WorkdayAddAction {
    fn id(&self) -> &'static str {
        "date.workday_add"
    }
    fn summary(&self) -> &'static str {
        "Add N business days to a date, skipping Saturdays and Sundays"
    }
    fn schema(&self) -> &'static Value {
        static S: Lazy<Value> = Lazy::new(crate::schema::derive::<WorkdayAddIn>);
        &S
    }
    async fn execute(&self, _ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let WorkdayAddIn { value, days } = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("date.workday_add invalid: {e}")))?;
        let mut dt = parse_any(&value)?;
        // Step one calendar day at a time toward the target, counting only
        // weekdays. The anchor day itself is never counted; the result lands on
        // a weekday. `days == 0` returns the input unchanged.
        let step = if days >= 0 { 1 } else { -1 };
        let mut remaining = days.abs();
        while remaining > 0 {
            dt += Duration::days(step);
            if !matches!(dt.weekday(), Weekday::Sat | Weekday::Sun) {
                remaining -= 1;
            }
        }
        Ok(ActionResult::from(Value::String(dt.to_rfc3339())))
    }
}
