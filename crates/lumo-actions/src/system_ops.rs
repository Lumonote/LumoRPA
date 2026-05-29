//! System actions (`system.*`).
//!
//! Shell execution is opt-in via `LUMO_ALLOW_SHELL=1` since giving a flow an
//! arbitrary process spawn is a meaningful escalation of trust. Sleep / env /
//! platform are pure-info and require no opt-in.

use async_trait::async_trait;
use lumo_core::error::StepError;
use lumo_core::{Action, ActionRegistry, ActionResult, StepCtx};
use once_cell::sync::Lazy;
use serde::Deserialize;
use serde_json::Value;
use std::time::Duration;

pub fn register(r: &mut ActionRegistry) {
    r.register(ShellAction);
    r.register(EnvGetAction);
    r.register(SleepAction);
    r.register(PlatformAction);
}

pub struct ShellAction;
#[derive(Deserialize)]
struct ShellIn {
    command: String,
    #[serde(default)]
    cwd: Option<String>,
    #[serde(default = "default_shell_timeout")]
    timeout_ms: u64,
}
fn default_shell_timeout() -> u64 { 30_000 }

#[async_trait]
impl Action for ShellAction {
    fn id(&self) -> &'static str { "system.shell" }
    fn summary(&self) -> &'static str { "Run `command` in the platform shell (requires LUMO_ALLOW_SHELL=1)" }
    fn schema(&self) -> &'static Value {
        static S: Lazy<Value> = Lazy::new(|| serde_json::json!({
            "type": "object",
            "required": ["command"],
            "properties": {
                "command":    { "type": "string" },
                "cwd":        { "type": "string" },
                "timeout_ms": { "type": "integer", "minimum": 100, "default": 30000 }
            },
            "additionalProperties": false
        }));
        &S
    }
    async fn execute(&self, _ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let ShellIn { command, cwd, timeout_ms } = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("system.shell invalid: {e}")))?;
        if std::env::var("LUMO_ALLOW_SHELL").ok().as_deref() != Some("1") {
            return Err(StepError::msg(
                "system.shell is disabled: set LUMO_ALLOW_SHELL=1 to allow",
            ));
        }
        let (program, flag) = if cfg!(target_os = "windows") {
            ("cmd", "/C")
        } else {
            ("sh", "-c")
        };
        let mut cmd = tokio::process::Command::new(program);
        cmd.arg(flag).arg(&command);
        if let Some(d) = &cwd {
            cmd.current_dir(d);
        }
        let fut = cmd.output();
        let output = tokio::time::timeout(Duration::from_millis(timeout_ms), fut)
            .await
            .map_err(|_| StepError::msg(format!("system.shell: timed out after {timeout_ms}ms")))?
            .map_err(|e| StepError::msg(format!("system.shell: {e}")))?;
        Ok(ActionResult::from(serde_json::json!({
            "code":   output.status.code(),
            "stdout": String::from_utf8_lossy(&output.stdout).into_owned(),
            "stderr": String::from_utf8_lossy(&output.stderr).into_owned(),
        })))
    }
}

pub struct EnvGetAction;
#[derive(Deserialize)]
struct EnvIn {
    name: String,
    #[serde(default)]
    default: Option<String>,
}
#[async_trait]
impl Action for EnvGetAction {
    fn id(&self) -> &'static str { "system.env_get" }
    fn summary(&self) -> &'static str { "Read env var by name; optional `default` when missing" }
    fn schema(&self) -> &'static Value {
        static S: Lazy<Value> = Lazy::new(|| serde_json::json!({
            "type": "object",
            "required": ["name"],
            "properties": {
                "name":    { "type": "string" },
                "default": { "type": "string" }
            },
            "additionalProperties": false
        }));
        &S
    }
    async fn execute(&self, _ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let EnvIn { name, default } = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("system.env_get invalid: {e}")))?;
        let val = std::env::var(&name).ok().or(default).unwrap_or_default();
        Ok(ActionResult::from(Value::String(val)))
    }
}

pub struct SleepAction;
#[derive(Deserialize)]
struct SleepIn { ms: u64 }
#[async_trait]
impl Action for SleepAction {
    fn id(&self) -> &'static str { "system.sleep" }
    fn summary(&self) -> &'static str { "Pause for `ms` milliseconds" }
    fn schema(&self) -> &'static Value {
        static S: Lazy<Value> = Lazy::new(|| serde_json::json!({
            "type": "object",
            "required": ["ms"],
            "properties": { "ms": { "type": "integer", "minimum": 0, "maximum": 600000 } },
            "additionalProperties": false
        }));
        &S
    }
    async fn execute(&self, _ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let SleepIn { ms } = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("system.sleep invalid: {e}")))?;
        tokio::time::sleep(Duration::from_millis(ms.min(600_000))).await;
        Ok(ActionResult::from(serde_json::json!({ "slept_ms": ms })))
    }
}

pub struct PlatformAction;
#[async_trait]
impl Action for PlatformAction {
    fn id(&self) -> &'static str { "system.platform" }
    fn summary(&self) -> &'static str { "Report `{ os, arch, family }`" }
    fn schema(&self) -> &'static Value {
        static S: Lazy<Value> = Lazy::new(|| serde_json::json!({
            "type": "object",
            "properties": {},
            "additionalProperties": false
        }));
        &S
    }
    async fn execute(&self, _ctx: &mut StepCtx, _input: Value) -> Result<ActionResult, StepError> {
        Ok(ActionResult::from(serde_json::json!({
            "os":     std::env::consts::OS,
            "arch":   std::env::consts::ARCH,
            "family": std::env::consts::FAMILY,
        })))
    }
}
