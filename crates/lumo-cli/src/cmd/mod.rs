pub mod actions;
pub mod copilot;
pub mod hotkey;
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
use lumo_core::{ActionRegistry, FlowVm};
use lumo_dsl::Flow;
use lumo_skills::{register_skill_actions, SkillRegistry};
use std::path::{Path, PathBuf};
use std::sync::Arc;

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

pub(crate) fn build_action_registry(home: &Path, flow_path: Option<&Path>) -> ActionRegistry {
    let providers_cfg = ProvidersConfig::load(providers_path(home)).unwrap_or_default();
    let router = Arc::new(AiRouter::from_config(&providers_cfg));

    let mut registry = ActionRegistry::new();
    lumo_actions::register_all(&mut registry);
    registry.register(ChatAction::new(router));

    let skill_reg = load_skill_registry(home, flow_path);
    register_skill_actions(&mut registry, skill_reg);
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
