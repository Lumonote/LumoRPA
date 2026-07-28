//! Control-flow actions.

use async_trait::async_trait;
use lumo_core::error::StepError;
use lumo_core::{Action, ActionRegistry, ActionResult, StepCtx};
use once_cell::sync::Lazy;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::Value;

pub fn register(r: &mut ActionRegistry) {
    r.register(LogAction);
    r.register(SetVarAction);
    r.register(SleepAction);
    r.register(IfAction);
    r.register(ForAction);
    r.register(ForEachAction);
    r.register(WhileAction);
    r.register(BreakAction);
    r.register(ContinueAction);
    r.register(TryAction);
    r.register(ParallelAction);
    r.register(FailAction);
}

// ─── control.log ────────────────────────────────────────────────────────────

pub struct LogAction;
#[derive(Deserialize)]
struct LogIn {
    #[serde(default)]
    message: String,
    #[serde(default)]
    level: Option<String>,
}

#[async_trait]
impl Action for LogAction {
    fn id(&self) -> &'static str {
        "control.log"
    }
    fn summary(&self) -> &'static str {
        "Write a message to the run log"
    }
    fn schema(&self) -> &'static serde_json::Value {
        static SCHEMA: Lazy<Value> = Lazy::new(|| {
            serde_json::json!({
                "type": "object",
                "properties": {
                    "message": { "type": "string" },
                    "level": { "type": "string", "enum": ["debug", "info", "warn", "error"] }
                },
                "additionalProperties": false
            })
        });
        &SCHEMA
    }
    async fn execute(&self, ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let LogIn { message, level } = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("log input invalid: {e}")))?;
        let level = level.unwrap_or_else(|| "info".into());
        match level.as_str() {
            "warn" => tracing::warn!(target: "lumo.flow", "{}", message),
            "error" => tracing::error!(target: "lumo.flow", "{}", message),
            "debug" => tracing::debug!(target: "lumo.flow", "{}", message),
            _ => tracing::info!(target: "lumo.flow", "{}", message),
        }
        ctx.log(&message);
        println!("[log] {message}");
        Ok(ActionResult::from(serde_json::json!({
            "message": message,
            "level": level
        })))
    }
}

// ─── control.set_var ────────────────────────────────────────────────────────

pub struct SetVarAction;
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SetVarIn {
    name: String,
    value: Value,
}

#[async_trait]
impl Action for SetVarAction {
    fn id(&self) -> &'static str {
        "control.set_var"
    }
    fn summary(&self) -> &'static str {
        "Set a flow variable"
    }
    fn schema(&self) -> &'static serde_json::Value {
        static SCHEMA: Lazy<Value> = Lazy::new(crate::schema::derive::<SetVarIn>);
        &SCHEMA
    }
    async fn execute(&self, ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let SetVarIn { name, value } = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("set_var input invalid: {e}")))?;
        ctx.set_var(&name, value.clone());
        Ok(ActionResult::from(value))
    }
}

// ─── control.sleep ──────────────────────────────────────────────────────────

pub struct SleepAction;
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SleepIn {
    ms: u64,
}

#[async_trait]
impl Action for SleepAction {
    fn id(&self) -> &'static str {
        "control.sleep"
    }
    fn summary(&self) -> &'static str {
        "Sleep for N milliseconds"
    }
    fn schema(&self) -> &'static serde_json::Value {
        static SCHEMA: Lazy<Value> = Lazy::new(crate::schema::derive::<SleepIn>);
        &SCHEMA
    }
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
#[derive(Deserialize, Default, JsonSchema)]
#[serde(deny_unknown_fields)]
struct IfIn {
    #[serde(default)]
    cond: Value,
}

#[async_trait]
impl Action for IfAction {
    fn id(&self) -> &'static str {
        "control.if"
    }
    fn summary(&self) -> &'static str {
        "Conditional branch (use do: / else:)"
    }
    fn schema(&self) -> &'static serde_json::Value {
        static SCHEMA: Lazy<Value> = Lazy::new(crate::schema::derive::<IfIn>);
        &SCHEMA
    }
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
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[allow(dead_code)]
struct ForIn {
    #[serde(default)]
    from: i64,
    to: i64,
    #[serde(default = "default_step_i64")]
    step: i64,
    #[serde(default = "default_bind")]
    bind: String,
}
fn default_step_i64() -> i64 {
    1
}
fn default_bind() -> String {
    "index".into()
}

#[async_trait]
impl Action for ForAction {
    fn id(&self) -> &'static str {
        "control.for"
    }
    fn summary(&self) -> &'static str {
        "Numeric loop (use do:)"
    }
    fn schema(&self) -> &'static serde_json::Value {
        static SCHEMA: Lazy<Value> = Lazy::new(crate::schema::derive::<ForIn>);
        &SCHEMA
    }
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
struct ForEachIn {
    #[serde(default)]
    r#in: Value,
    #[serde(default = "default_item_bind")]
    bind: String,
    #[serde(default)]
    parallel: bool,
    #[serde(default = "default_max_concurrency")]
    max_concurrency: usize,
}
fn default_item_bind() -> String {
    "item".into()
}

#[async_trait]
impl Action for ForEachAction {
    fn id(&self) -> &'static str {
        "control.for_each"
    }
    fn summary(&self) -> &'static str {
        "Iterate over a list (use do:)"
    }
    fn schema(&self) -> &'static serde_json::Value {
        static SCHEMA: Lazy<Value> = Lazy::new(|| {
            serde_json::json!({
                "type": "object",
                "required": ["in"],
                "properties": {
                    "in": { "type": "array" },
                    "bind": { "type": "string" },
                    "parallel": { "type": "boolean", "default": false },
                    "max_concurrency": { "type": "integer", "minimum": 1, "default": 8 }
                },
                "additionalProperties": false
            })
        });
        &SCHEMA
    }
    async fn execute(&self, _ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let _cfg: ForEachIn = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("for_each input invalid: {e}")))?;
        Ok(ActionResult::null())
    }
}

// ─── control.while ──────────────────────────────────────────────────────────
// 指令集 P1:条件循环。与其余 control.* 一样,VM 在分发点短路接管(vm.rs 的
// run_while 自己求值 cond 并驱动 do: 块),这里只是注册表里的占位标记,
// 让 schema / 文档 / `actions --show` 能看到它。

pub struct WhileAction;
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[allow(dead_code)]
struct WhileIn {
    /// 循环条件,每轮求值(与 control.if 同一求值器)。
    cond: Value,
    /// 防呆死循环上限,默认 1000;到达上限且 cond 仍为真则报错。
    #[serde(default = "default_max_iterations")]
    max_iterations: u64,
}
fn default_max_iterations() -> u64 {
    1000
}

#[async_trait]
impl Action for WhileAction {
    fn id(&self) -> &'static str {
        "control.while"
    }
    fn summary(&self) -> &'static str {
        "Conditional loop: repeat do: while cond holds (use do:)"
    }
    fn schema(&self) -> &'static serde_json::Value {
        static SCHEMA: Lazy<Value> = Lazy::new(crate::schema::derive::<WhileIn>);
        &SCHEMA
    }
    async fn execute(&self, _ctx: &mut StepCtx, _input: Value) -> Result<ActionResult, StepError> {
        Ok(ActionResult::null())
    }
}

// ─── control.break / control.continue ──────────────────────────────────────
// 指令集 P1:循环控制信号。VM 在分发点把它们转成 ExecError::Break / Continue
// 向上 unwind,由最近的循环容器(while / for / for_each)消化——所以正常执行
// 路径永远到不了这里的 execute。直接经注册表调用(无循环上下文)时按
// "循环外使用" 报错,与 validate 的静态检查口径一致。

pub struct BreakAction;
#[derive(Deserialize, Default, JsonSchema)]
#[serde(deny_unknown_fields)]
struct BreakIn {}

#[async_trait]
impl Action for BreakAction {
    fn id(&self) -> &'static str {
        "control.break"
    }
    fn summary(&self) -> &'static str {
        "Exit the nearest enclosing loop (while/for/for_each)"
    }
    fn schema(&self) -> &'static serde_json::Value {
        static SCHEMA: Lazy<Value> = Lazy::new(crate::schema::derive::<BreakIn>);
        &SCHEMA
    }
    async fn execute(&self, _ctx: &mut StepCtx, _input: Value) -> Result<ActionResult, StepError> {
        Err(StepError::msg(
            "`control.break` used outside of a loop (must run inside control.while/for/for_each)",
        ))
    }
}

pub struct ContinueAction;
#[derive(Deserialize, Default, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ContinueIn {}

#[async_trait]
impl Action for ContinueAction {
    fn id(&self) -> &'static str {
        "control.continue"
    }
    fn summary(&self) -> &'static str {
        "Skip to the next iteration of the nearest enclosing loop"
    }
    fn schema(&self) -> &'static serde_json::Value {
        static SCHEMA: Lazy<Value> = Lazy::new(crate::schema::derive::<ContinueIn>);
        &SCHEMA
    }
    async fn execute(&self, _ctx: &mut StepCtx, _input: Value) -> Result<ActionResult, StepError> {
        Err(StepError::msg(
            "`control.continue` used outside of a loop (must run inside control.while/for/for_each)",
        ))
    }
}

// ─── control.try ────────────────────────────────────────────────────────────

pub struct TryAction;

#[async_trait]
impl Action for TryAction {
    fn id(&self) -> &'static str {
        "control.try"
    }
    fn summary(&self) -> &'static str {
        "Try/catch/finally (use do: / catch: / finally:)"
    }
    fn schema(&self) -> &'static serde_json::Value {
        static SCHEMA: Lazy<Value> = Lazy::new(|| {
            serde_json::json!({
                "type": "object",
                "properties": {},
                "additionalProperties": true
            })
        });
        &SCHEMA
    }
    async fn execute(&self, _ctx: &mut StepCtx, _input: Value) -> Result<ActionResult, StepError> {
        Ok(ActionResult::null())
    }
}

// ─── control.parallel ──────────────────────────────────────────────────────

pub struct ParallelAction;

fn default_max_concurrency() -> usize {
    8
}

#[async_trait]
impl Action for ParallelAction {
    fn id(&self) -> &'static str {
        "control.parallel"
    }
    fn summary(&self) -> &'static str {
        "Parallel block marker (M1 runs sequentially)"
    }
    fn schema(&self) -> &'static serde_json::Value {
        static SCHEMA: Lazy<Value> = Lazy::new(|| {
            serde_json::json!({
                "type": "object",
                "properties": {
                    "max_concurrency": { "type": "integer", "minimum": 1, "default": 8 }
                },
                "additionalProperties": false
            })
        });
        &SCHEMA
    }
    async fn execute(&self, _ctx: &mut StepCtx, _input: Value) -> Result<ActionResult, StepError> {
        Ok(ActionResult::null())
    }
}

// ─── control.fail ───────────────────────────────────────────────────────────

pub struct FailAction;
#[derive(Deserialize, Default, JsonSchema)]
#[serde(deny_unknown_fields)]
struct FailIn {
    #[serde(default)]
    message: String,
}

#[async_trait]
impl Action for FailAction {
    fn id(&self) -> &'static str {
        "control.fail"
    }
    fn summary(&self) -> &'static str {
        "Explicitly fail the current flow with a message"
    }
    fn schema(&self) -> &'static serde_json::Value {
        static SCHEMA: Lazy<Value> = Lazy::new(crate::schema::derive::<FailIn>);
        &SCHEMA
    }
    async fn execute(&self, _ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let FailIn { message } = serde_json::from_value(input).unwrap_or_default();
        Err(StepError::UserFail(if message.is_empty() {
            "user fail".into()
        } else {
            message
        }))
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
