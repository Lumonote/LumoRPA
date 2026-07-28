pub mod actions;
pub mod copilot;
pub mod hotkey;
pub mod human;
pub mod init;
pub mod lint;
pub mod mcp;
pub mod providers;
pub mod run;
pub mod runs;
pub mod serve;
pub mod skills;
pub mod validate;
pub mod vault;

use lumo_ai::{AiRouter, ChatAction, ProvidersConfig};
use lumo_core::{ActionRegistry, CancelToken, FlowVm};
use lumo_dsl::Flow;
use lumo_skills::{register_flow_call_action, register_skill_actions, SkillRegistry};
use lumo_storage::Repo;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// 架构 P1-1：CLI 四条宿主线（`lumo run`、`lumo serve` 的 webhook/cron/file
/// 触发、`lumo mcp` 的 `run_flow`、hotkey 派发）共用的 VM 组装入口。宿主间的
/// 装配差异只剩两点：有没有 repo（`run --no-store` ⇒ `None`）、取消令牌从哪
/// 来（run=Ctrl-C 信号，serve webhook=HTTP 取消路由，其余=每次运行的新令牌，
/// 以备将来接线）。其余运行时接线统一收敛在这里：
///
///   * step_timeout —— `LUMO_STEP_TIMEOUT_MS` 的解析上移至 CLI 宿主层
///     （[`step_timeout_from_env`]，默认 600s，与桌面端 `step_timeout()` 同
///     语义），防一个卡死的步骤把宿主挂死；
///   * artifacts_dir —— 有 repo 才落 `$LUMO_HOME/artifacts`；无 repo 的归档
///     只会留下没有表行的孤儿 blob，保持 no-op（与 `run --no-store` 的既有
///     语义一致）；
///   * cancel —— 注入调用方的令牌；调用方持同源克隆即可随时协作取消；
///   * vault / AI hooks —— 沿用 [`load_vault_identity`] / [`attach_ai_hooks`]
///     的既有语义。
///
/// 注意：human prompter 有意**不**在这里注入。headless 宿主（serve/mcp/
/// hotkey）没有真人可应答，保持 `None` 让引擎对 `human.*` 步骤显式报错而不是
/// 无限挂起（lumo-core 的既有语义）；只有交互式的 `lumo run` 在调用方链上
/// `.with_human_prompter(CliPrompter)`。
pub fn host_vm(
    home: &Path,
    flow: &Flow,
    registry: ActionRegistry,
    repo: Option<Repo>,
    cancel: CancelToken,
) -> FlowVm {
    let artifacts_dir = repo.is_some().then(|| home.join("artifacts"));
    attach_ai_hooks(FlowVm::new(registry, repo), home, flow)
        .with_vault(load_vault_identity(home))
        .with_step_timeout(step_timeout_from_env())
        .with_artifacts_dir(artifacts_dir)
        .with_cancel(cancel)
}

/// P1-1：步级超时解析 —— 与桌面端 `step_timeout()` 完全同语义：默认 10 分钟
/// （600s）；`LUMO_STEP_TIMEOUT_MS` 可覆盖，解析失败或填 0 一律回退默认
/// （0 会让所有步骤瞬间超时，按配置错误处理）。
pub fn step_timeout_from_env() -> std::time::Duration {
    step_timeout_from_ms(std::env::var("LUMO_STEP_TIMEOUT_MS").ok().as_deref())
}

/// [`step_timeout_from_env`] 的纯函数内核，便于单测解析规则（env 是进程级
/// 全局，测试直接喂字符串可绕开 cargo test 多线程下的环境变量竞争）。
pub fn step_timeout_from_ms(raw: Option<&str>) -> std::time::Duration {
    const DEFAULT_MS: u64 = 600_000;
    let ms = raw
        .and_then(|v| v.parse::<u64>().ok())
        .filter(|ms| *ms > 0)
        .unwrap_or(DEFAULT_MS);
    std::time::Duration::from_millis(ms)
}

pub(crate) fn providers_path(home: &Path) -> PathBuf {
    std::env::var_os("LUMO_PROVIDERS_PATH")
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join("providers.toml"))
}

pub(crate) fn skills_root(home: &Path) -> PathBuf {
    std::env::var_os("LUMO_SKILLS_PATH")
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join("skills"))
}

pub(crate) fn vault_identity_path(home: &Path) -> PathBuf {
    std::env::var_os("LUMO_VAULT_IDENTITY")
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join("age-identity.txt"))
}

/// Load the vault identity for a run if one exists. Missing file ⇒ `None`
/// (env-only resolution); a present-but-corrupt file is warned about and also
/// degrades to `None` so a run never hard-fails on vault wiring alone (P1-3).
pub(crate) fn load_vault_identity(home: &Path) -> Option<Arc<lumo_storage::VaultIdentity>> {
    let path = vault_identity_path(home);
    if !path.exists() {
        return None;
    }
    match lumo_storage::VaultIdentity::load(&path) {
        Ok(id) => Some(Arc::new(id)),
        Err(e) => {
            tracing::warn!("vault identity at {} unreadable: {e}", path.display());
            None
        }
    }
}

pub(crate) fn build_action_registry(home: &Path, flow_path: Option<&Path>) -> ActionRegistry {
    let providers_cfg = ProvidersConfig::load(providers_path(home)).unwrap_or_default();
    let router = Arc::new(AiRouter::from_config(&providers_cfg));

    let mut registry = ActionRegistry::new();
    lumo_actions::register_all(&mut registry);
    registry.register(ChatAction::new(router));

    let skill_reg = load_skill_registry(home, flow_path);
    register_skill_actions(&mut registry, skill_reg);

    // F-15: `flow.call` resolves sub-flow files relative to the running flow's
    // own directory (falling back to `$LUMO_HOME`), confined to that base.
    let flow_base = flow_path
        .and_then(Path::parent)
        .map(Path::to_path_buf)
        .unwrap_or_else(|| home.to_path_buf());
    register_flow_call_action(&mut registry, flow_base);
    registry
}

/// P0-1: attach the AI hook provider (selector heal / visual extract / decide /
/// diagnose / vision-locate) to a freshly built VM based on the flow's
/// `metadata.ai` policy and the configured providers. No-op when AI is disabled
/// or no providers are configured.
///
/// This is the call that was missing: `FlowVm::with_ai_provider` previously had
/// zero callers, so `effective_ai_mode` always resolved to `Off` and the entire
/// AI-hooks subsystem was dead code at runtime.
pub(crate) fn attach_ai_hooks(vm: FlowVm, home: &Path, flow: &Flow) -> FlowVm {
    let ai = flow.metadata.ai.clone().unwrap_or_default();
    let cfg = ProvidersConfig::load(providers_path(home)).unwrap_or_default();
    match lumo_ai::build_hook_provider(&cfg, ai.enabled, ai.budget.max_calls_per_run) {
        Some(provider) => vm.with_ai_provider(provider),
        None => vm,
    }
}

pub(crate) fn load_skill_registry(home: &Path, flow_path: Option<&Path>) -> Arc<SkillRegistry> {
    let skill_reg = Arc::new(SkillRegistry::new());
    if let Err(e) = skill_reg.load_dir(skills_root(home)) {
        tracing::warn!("load installed skills: {e}");
    }
    if let Some(flow_path) = flow_path {
        if let Some(flow_dir) = flow_path.parent() {
            if let Err(e) = skill_reg.load_dir(flow_dir.join("skills")) {
                tracing::warn!("load flow-local skills: {e}");
            }
        }
    }
    skill_reg
}
