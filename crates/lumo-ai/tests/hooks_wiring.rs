//! P0-1 regression guard.
//!
//! Before this, `FlowVm::with_ai_provider` had ZERO callers: every runner built
//! the VM with `FlowVm::new(..)` and never attached an `AiHookProvider`, so
//! `effective_ai_mode` always returned `Off` and the entire AI-hooks subsystem
//! (heal_selector / extract_visual / decide / vision_locate / diagnose) was
//! dead code at runtime. `build_hook_provider` is the shared factory the
//! runners now call; these tests pin its on/off contract.

use lumo_ai::{build_hook_provider, ProviderProfile, ProvidersConfig};

#[test]
fn no_provider_when_ai_disabled() {
    // Flow opted out of AI (`metadata.ai.enabled: false`) ⇒ hooks stay off
    // even with providers configured.
    let cfg = ProvidersConfig::seed_default();
    assert!(build_hook_provider(&cfg, false, 10).is_none());
}

#[test]
fn no_provider_when_no_profiles_configured() {
    // AI enabled but no provider profiles ⇒ don't spin up doomed LLM calls.
    let cfg = ProvidersConfig::default();
    assert!(build_hook_provider(&cfg, true, 10).is_none());
}

#[test]
fn builds_provider_when_enabled_and_configured() {
    // The case that was previously impossible to reach: enabled + configured
    // ⇒ a live `Arc<dyn AiHookProvider>` the VM can use.
    let cfg = ProvidersConfig::seed_default();
    assert!(build_hook_provider(&cfg, true, 10).is_some());
}

#[test]
fn builds_provider_for_local_ocr_profile() {
    // A local OCR-only profile has no LLM router backend, but image.ocr still
    // needs the hook provider so it can dispatch to the local ModelScope worker.
    let cfg = ProvidersConfig {
        active: Some("local-ocr".into()),
        profiles: vec![ProviderProfile {
            name: "local-ocr".into(),
            kind: "local".into(),
            wire_api: None,
            base_url: None,
            api_key: None,
            api_key_env: None,
            default_model: None,
            vision_model: None,
            ocr_model: Some("modelscope/ZhipuAI/GLM-OCR".into()),
            models: vec![],
            headers: Default::default(),
            reasoning_effort: None,
            notes: None,
        }],
    };
    assert!(build_hook_provider(&cfg, true, 10).is_some());
}
