//! Control-flow actions.

use async_trait::async_trait;
use lumo_core::{Action, ActionRegistry, ActionResult, StepCtx};
use lumo_core::error::StepError;
use serde::Deserialize;
use serde_json::Value;

pub fn register(r: &mut ActionRegistry) {
    r.register(LogAction);
    r.register(SetVarAction);
    r.register(SleepAction);
    r.register(IfAction);
    r.register(ForAction);
    r.register(ForEachAction);
    r.register(TryAction);
    r.register(FailAction);
}

// ─── control.log ────────────────────────────────────────────────────────────

pub struct LogAction;
#[derive(Deserialize)]
struct LogIn { #[serde(default)] message: String, #[serde(default)] level: Option<String> }

#[async_trait]
impl Action for LogAction {
    fn id(&self) -> &'static str { "control.log" }
    fn summary(&self) -> &'static str { "Write a message to the run log" }
    async fn execute(&self, ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let LogIn { message, level } = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("log input invalid: {e}")))?;
        let level = level.unwrap_or_else(|| "info".into());
        match level.as_str() {
            "warn" => tracing::warn!(target: "lumo.flow", "{}", message),
            "error"=> tracing::error!(target: "lumo.flow", "{}", message),
            "debug"=> tracing::debug!(target: "lumo.flow", "{}", message),
            _      => tracing::info!(target: "lumo.flow", "{}", message),
        }
        ctx.log(&message);
        println!("[log] {message}");
        Ok(ActionResult::null())
    }
}

// ─── control.set_var ────────────────────────────────────────────────────────

pub struct SetVarAction;
#[derive(Deserialize)]
struct SetVarIn { name: String, value: Value }

#[async_trait]
impl Action for SetVarAction {
    fn id(&self) -> &'static str { "control.set_var" }
    fn summary(&self) -> &'static str { "Set a flow variable" }
    async fn execute(&self, ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let SetVarIn { name, value } = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("set_var input invalid: {e}")))?;
        ctx.set_var(&name, value.clone());
        Ok(ActionResult::from(value))
    }
}

// ─── control.sleep ──────────────────────────────────────────────────────────

pub struct SleepAction;
#[derive(Deserialize)]
struct SleepIn { ms: u64 }

#[async_trait]
impl Action for SleepAction {
    fn id(&self) -> &'static str { "control.sleep" }
    fn summary(&self) -> &'static str { "Sleep for N milliseconds" }
    async fn execute(&self, _ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let SleepIn { ms } = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("sleep input invalid: {e}")))?;
        tokio::time::sleep(std::time::Duration::from_millis(ms)).await;
        Ok(ActionResult::null())
    }
}

// ─── control.if ─────────────────────────────────────────────────────────────
// NOTE: condition is evaluated against rendered `with.cond` value (truthy).
//       Children are placed in `do:` / `else:` blocks on the *Step* level.

pub struct IfAction;
#[derive(Deserialize, Default)]
struct IfIn { #[serde(default)] cond: Value }

#[async_trait]
impl Action for IfAction {
    fn id(&self) -> &'static str { "control.if" }
    fn summary(&self) -> &'static str { "Conditional branch (use do: / else:)" }
    async fn execute(&self, _ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let IfIn { cond } = serde_json::from_value(input).unwrap_or_default();
        let truthy = is_truthy(&cond);
        Ok(ActionResult::from(Value::Bool(truthy)))
    }
}

// ─── control.for ────────────────────────────────────────────────────────────
// In M1 the VM dispatches Step.do_ children itself; the action body below is
// a no-op marker so the registry can validate `control.for` references. M2
// will wire the loop semantics through StepCtx::run_block.

pub struct ForAction;
#[derive(Deserialize)]
#[allow(dead_code)]
struct ForIn {
    #[serde(default)] from: i64,
    to: i64,
    #[serde(default = "default_step_i64")] step: i64,
    #[serde(default = "default_bind")] bind: String,
}
fn default_step_i64() -> i64 { 1 }
fn default_bind() -> String { "index".into() }

#[async_trait]
impl Action for ForAction {
    fn id(&self) -> &'static str { "control.for" }
    fn summary(&self) -> &'static str { "Numeric loop (use do:)" }
    async fn execute(&self, ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let cfg: ForIn = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("for input invalid: {e}")))?;
        // We don't have direct access to step.do_ here; control-flow children are
        // executed by the VM via a side channel. In M1, the VM treats control.* as
        // a no-op action and recursively processes `step.do_` itself.
        let _ = (cfg, ctx);
        Ok(ActionResult::null())
    }
}

// ─── control.for_each ───────────────────────────────────────────────────────

pub struct ForEachAction;
#[derive(Deserialize)]
#[allow(dead_code)]
struct ForEachIn { #[serde(default)] r#in: Value, #[serde(default = "default_item_bind")] bind: String }
fn default_item_bind() -> String { "item".into() }

#[async_trait]
impl Action for ForEachAction {
    fn id(&self) -> &'static str { "control.for_each" }
    fn summary(&self) -> &'static str { "Iterate over a list (use do:)" }
    async fn execute(&self, _ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let _cfg: ForEachIn = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("for_each input invalid: {e}")))?;
        Ok(ActionResult::null())
    }
}

// ─── control.try ────────────────────────────────────────────────────────────

pub struct TryAction;

#[async_trait]
impl Action for TryAction {
    fn id(&self) -> &'static str { "control.try" }
    fn summary(&self) -> &'static str { "Try/catch/finally (use do: / catch: / finally:)" }
    async fn execute(&self, _ctx: &mut StepCtx, _input: Value) -> Result<ActionResult, StepError> {
        Ok(ActionResult::null())
    }
}

// ─── control.fail ───────────────────────────────────────────────────────────

pub struct FailAction;
#[derive(Deserialize, Default)]
struct FailIn { #[serde(default)] message: String }

#[async_trait]
impl Action for FailAction {
    fn id(&self) -> &'static str { "control.fail" }
    fn summary(&self) -> &'static str { "Explicitly fail the current flow with a message" }
    async fn execute(&self, _ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let FailIn { message } = serde_json::from_value(input).unwrap_or_default();
        Err(StepError::UserFail(if message.is_empty() { "user fail".into() } else { message }))
    }
}

fn is_truthy(v: &Value) -> bool {
    match v {
        Value::Bool(b) => *b,
        Value::Null => false,
        Value::Number(n) => n.as_f64().map(|f| f != 0.0).unwrap_or(false),
        Value::String(s) => {
            let t = s.trim().to_ascii_lowercase();
            !matches!(t.as_str(), "" | "false" | "0" | "null" | "none" | "no")
        }
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
    }
}
