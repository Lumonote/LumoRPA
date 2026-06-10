//! System actions (`system.*`).
//!
//! Shell execution is opt-in via `LUMO_ALLOW_SHELL=1` since giving a flow an
//! arbitrary process spawn is a meaningful escalation of trust. Sleep / env /
//! platform are pure-info and require no opt-in. Process control
//! (`system.process_kill` / `system.app_start`) is the same tier of trust
//! escalation and is opt-in via `LUMO_ALLOW_PROCESS=1`.

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
    r.register(ProcessKillAction);
    r.register(AppStartAction);
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

// ─── system.process_kill / system.app_start ──────────────────────────────────
// 进程控制域:杀任意进程 / 拉起任意外部应用,与 system.shell 同级的信任升级,
// 同样走环境变量显式 opt-in。闸门独立于 LUMO_ALLOW_SHELL:授权「能跑 shell」
// 与授权「能杀进程 / 启应用」是两个不同的运维决策,互不隐含。

/// `system.process_kill` / `system.app_start` 共用的 opt-in 闸(默认拒绝)。
fn ensure_process_allowed(action: &str) -> Result<(), StepError> {
    if std::env::var("LUMO_ALLOW_PROCESS").ok().as_deref() != Some("1") {
        return Err(StepError::msg(format!(
            "{action} is disabled: set LUMO_ALLOW_PROCESS=1 to allow"
        )));
    }
    Ok(())
}

pub struct ProcessKillAction;
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ProcessKillIn {
    pid: u32,
    /// 强杀:Unix 发 SIGKILL(缺省 SIGTERM,进程可优雅收尾);Windows 没有
    /// 优雅终止信号,force 与否都是 TerminateProcess(见 kill_process 注释)。
    #[serde(default)]
    force: bool,
}

#[async_trait]
impl Action for ProcessKillAction {
    fn id(&self) -> &'static str {
        "system.process_kill"
    }
    fn summary(&self) -> &'static str {
        "Terminate a process by pid — SIGTERM, or SIGKILL with `force` (requires LUMO_ALLOW_PROCESS=1)"
    }
    fn schema(&self) -> &'static Value {
        static S: Lazy<Value> = Lazy::new(crate::schema::derive::<ProcessKillIn>);
        &S
    }
    async fn execute(&self, _ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let ProcessKillIn { pid, force } = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("system.process_kill invalid: {e}")))?;
        ensure_process_allowed("system.process_kill")?;
        // 防呆:拒杀自身 —— 杀掉宿主进程等于让 run 自毁,永远不是 flow 的本意
        // (多半是表达式算错了 pid)。pid 0 同拒:Unix kill(0) 打的是整个进程组,
        // 同样殃及自身。
        if pid == std::process::id() {
            return Err(StepError::msg(format!(
                "system.process_kill: refusing to kill own process (pid {pid})"
            )));
        }
        if pid == 0 {
            return Err(StepError::msg(
                "system.process_kill: refusing pid 0 (the whole process group on Unix)",
            ));
        }
        // sysinfo 的进程表刷新是阻塞 syscall,与 process_list 同款挪到阻塞线程池。
        let signal = tokio::task::spawn_blocking(move || kill_process(pid, force))
            .await
            .map_err(|e| StepError::msg(format!("system.process_kill join: {e}")))?
            .map_err(StepError::msg)?;
        Ok(ActionResult::from(serde_json::json!({
            "pid": pid,
            // 实际送达的信号:"term"(可优雅处理)或 "kill"(强杀 / Windows 唯一路径)。
            "signal": signal,
        })))
    }
}

/// 按 pid 定位并终止进程(纯 Rust `sysinfo` 路径,无 C 依赖)。pid 不存在是
/// 显式错误,绝不沉默成功 —— 「以为杀掉了其实没这进程」会让后续步骤建立在
/// 假前提上。返回实际送达的信号名。
fn kill_process(pid: u32, force: bool) -> Result<&'static str, String> {
    use sysinfo::{Pid, ProcessesToUpdate, System};
    let target = Pid::from_u32(pid);
    let mut sys = System::new();
    sys.refresh_processes(ProcessesToUpdate::Some(&[target]), true);
    let proc = sys
        .process(target)
        .ok_or_else(|| format!("system.process_kill: no such process: pid {pid}"))?;
    // Unix:force ⇒ SIGKILL,否则 SIGTERM;Windows 的 kill_with 不支持 Term
    // (返回 None),回落到 kill()(TerminateProcess)—— 平台差异收在这一处。
    let (signal, sent) = if force {
        ("kill", proc.kill())
    } else {
        match proc.kill_with(sysinfo::Signal::Term) {
            Some(sent) => ("term", sent),
            None => ("kill", proc.kill()),
        }
    };
    if !sent {
        return Err(format!(
            "system.process_kill: failed to signal pid {pid} (insufficient permission?)"
        ));
    }
    Ok(signal)
}

pub struct AppStartAction;
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct AppStartIn {
    /// 可执行文件路径或名字(按 PATH 解析);macOS 上以 `.app` 结尾时走
    /// `open -a` 语义(LaunchServices 拉起 bundle)。
    program: String,
    #[serde(default)]
    args: Vec<String>,
    #[serde(default)]
    cwd: Option<String>,
}

#[async_trait]
impl Action for AppStartAction {
    fn id(&self) -> &'static str {
        "system.app_start"
    }
    fn summary(&self) -> &'static str {
        "Start an external application detached and return its pid (requires LUMO_ALLOW_PROCESS=1)"
    }
    fn schema(&self) -> &'static Value {
        static S: Lazy<Value> = Lazy::new(crate::schema::derive::<AppStartIn>);
        &S
    }
    async fn execute(&self, _ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let AppStartIn { program, args, cwd } = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("system.app_start invalid: {e}")))?;
        ensure_process_allowed("system.app_start")?;
        if program.trim().is_empty() {
            return Err(StepError::msg("system.app_start: program must not be empty"));
        }
        // macOS 的 .app 是 bundle 目录,不能直接 exec —— 转 `open -a <bundle>
        // [--args …]`。代价:返回的 pid 是 `open` 启动器的(LaunchServices 异步
        // 拉起目标 app,真实 pid 拿不到);需要精确 pid 时请直接给 bundle 内的
        // 可执行文件路径。其余平台(及 macOS 普通可执行文件)一律直接 spawn。
        let (prog, argv, launcher) = if cfg!(target_os = "macos") && program.ends_with(".app") {
            let mut v = vec!["-a".to_string(), program.clone()];
            if !args.is_empty() {
                v.push("--args".into());
                v.extend(args.iter().cloned());
            }
            ("open".to_string(), v, "open")
        } else {
            (program.clone(), args.clone(), "direct")
        };
        let mut cmd = tokio::process::Command::new(&prog);
        cmd.args(&argv);
        if let Some(d) = &cwd {
            cmd.current_dir(d);
        }
        // detached 语义:不接管子进程 stdio(置 null,防其阻塞/写爆宿主终端),
        // 不等退出。kill_on_drop 默认 false,Child 丢弃后子进程照常运行,tokio
        // 会在后台收尸,Unix 上不留僵尸。
        cmd.stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        let child = cmd
            .spawn()
            .map_err(|e| StepError::msg(format!("system.app_start spawn `{program}`: {e}")))?;
        // 未 wait 过的 Child 一定有 pid;0 兜底只为不 panic。
        let pid = child.id().unwrap_or(0);
        drop(child);
        Ok(ActionResult::from(serde_json::json!({
            "pid": pid,
            // "direct" = 直接 spawn 的就是目标进程;"open" = macOS .app 路径,
            // pid 属于 open 启动器。
            "launcher": launcher,
        })))
    }
}
