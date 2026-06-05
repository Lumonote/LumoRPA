//! System actions (`system.*`).
//!
//! Shell execution is opt-in via `LUMO_ALLOW_SHELL=1` since giving a flow an
//! arbitrary process spawn is a meaningful escalation of trust. Sleep / env /
//! platform are pure-info and require no opt-in.

use async_trait::async_trait;
use lumo_core::error::StepError;
use lumo_core::{Action, ActionRegistry, ActionResult, StepCtx};
use once_cell::sync::Lazy;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::Value;
use std::time::Duration;

pub fn register(r: &mut ActionRegistry) {
    r.register(ShellAction);
    r.register(EnvGetAction);
    r.register(SleepAction);
    r.register(PlatformAction);
    r.register(ProcessListAction);
}

pub struct ShellAction;
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ShellIn {
    command: String,
    #[serde(default)]
    cwd: Option<String>,
    #[serde(default = "default_shell_timeout")]
    timeout_ms: u64,
}
fn default_shell_timeout() -> u64 {
    30_000
}

#[async_trait]
impl Action for ShellAction {
    fn id(&self) -> &'static str {
        "system.shell"
    }
    fn summary(&self) -> &'static str {
        "Run `command` in the platform shell (requires LUMO_ALLOW_SHELL=1)"
    }
    fn schema(&self) -> &'static Value {
        static S: Lazy<Value> = Lazy::new(crate::schema::derive::<ShellIn>);
        &S
    }
    async fn execute(&self, _ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let ShellIn {
            command,
            cwd,
            timeout_ms,
        } = serde_json::from_value(input)
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
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct EnvIn {
    name: String,
    #[serde(default)]
    default: Option<String>,
}
#[async_trait]
impl Action for EnvGetAction {
    fn id(&self) -> &'static str {
        "system.env_get"
    }
    fn summary(&self) -> &'static str {
        "Read env var by name; optional `default` when missing"
    }
    fn schema(&self) -> &'static Value {
        static S: Lazy<Value> = Lazy::new(crate::schema::derive::<EnvIn>);
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
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SleepIn {
    ms: u64,
}
#[async_trait]
impl Action for SleepAction {
    fn id(&self) -> &'static str {
        "system.sleep"
    }
    fn summary(&self) -> &'static str {
        "Pause for `ms` milliseconds"
    }
    fn schema(&self) -> &'static Value {
        static S: Lazy<Value> = Lazy::new(crate::schema::derive::<SleepIn>);
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
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct PlatformIn {}
#[async_trait]
impl Action for PlatformAction {
    fn id(&self) -> &'static str {
        "system.platform"
    }
    fn summary(&self) -> &'static str {
        "Report `{ os, arch, family }`"
    }
    fn schema(&self) -> &'static Value {
        static S: Lazy<Value> = Lazy::new(crate::schema::derive::<PlatformIn>);
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

// ─── system.process_list ──────────────────────────────────────────────────────
// 纯 Rust `sysinfo`(仅启用 `system` 特性)枚举进程。与 platform/env/sleep 同属
// 纯只读信息,不写文件/不触网,故无能力闸门 —— 不像 system.shell 那样可派生任意
// 进程,只读已有进程的元数据,与读取本机平台信息同级。

pub struct ProcessListAction;
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ProcessListIn {
    /// 仅返回名字(不区分大小写)包含此子串的进程;缺省返回全部。
    #[serde(default)]
    name: Option<String>,
    /// 返回条目上限(默认 1000),防超大进程表撑爆 step 快照。
    #[serde(default = "default_proc_limit")]
    limit: usize,
}
fn default_proc_limit() -> usize {
    1_000
}

#[async_trait]
impl Action for ProcessListAction {
    fn id(&self) -> &'static str {
        "system.process_list"
    }
    fn summary(&self) -> &'static str {
        "List running processes (pid/name/cpu/mem), optionally filtered by name"
    }
    fn schema(&self) -> &'static Value {
        static S: Lazy<Value> = Lazy::new(crate::schema::derive::<ProcessListIn>);
        &S
    }
    async fn execute(&self, _ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let ProcessListIn { name, limit } = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("system.process_list invalid: {e}")))?;
        // sysinfo 的刷新是阻塞 syscall,挪到阻塞线程池。
        let procs = tokio::task::spawn_blocking(move || collect_processes(name.as_deref(), limit))
            .await
            .map_err(|e| StepError::msg(format!("system.process_list join: {e}")))?;
        let total = procs.len();
        Ok(ActionResult::from(serde_json::json!({
            "processes": procs,
            "count": total,
        })))
    }
}

fn collect_processes(name_filter: Option<&str>, limit: usize) -> Vec<Value> {
    use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, RefreshKind, System};
    let mut sys = System::new_with_specifics(
        RefreshKind::new().with_processes(ProcessRefreshKind::everything()),
    );
    sys.refresh_processes(ProcessesToUpdate::All, true);
    let needle = name_filter.map(|n| n.to_lowercase());
    let mut out = Vec::new();
    for (pid, p) in sys.processes() {
        let name = p.name().to_string_lossy().into_owned();
        if let Some(n) = &needle {
            if !name.to_lowercase().contains(n.as_str()) {
                continue;
            }
        }
        out.push(serde_json::json!({
            "pid": pid.as_u32(),
            "name": name,
            // 即时 CPU 占用百分比(单次刷新为相对上一采样,首刷可能为 0)。
            "cpu": p.cpu_usage(),
            // 常驻内存,字节。
            "memory": p.memory(),
        }));
        if out.len() >= limit {
            break;
        }
    }
    out
}
