//! LumoRPA AI router with multi-provider adapters (OpenAI-compatible /
//! Anthropic) and a cc-switch style profile config.

pub mod action;
pub mod budget;
pub mod config;
pub(crate) mod cost;
pub mod helpers;
pub mod provider;
pub mod router;

pub use action::ChatAction;
pub use budget::RunBudget;
pub use config::{ConfigError, ProviderProfile, ProvidersConfig};
pub use helpers::{
    build_hook_provider, decide, diagnose, extract_visual, heal_selector, vision_locate, AiHooks,
};
// Re-export the canonical hook types from lumo-core so external callers see
// a single set of `HealedSelector`/`Decision` symbols.
pub use lumo_core::ai_hook::{
    AiHookProvider, Decision, HealedSelector, LocatedTarget, SoMMark,
};
pub use provider::{
    AnthropicProvider, ChatMessage, ChatRequest, ChatResponse, LlmProvider, OpenAiProvider,
    ProviderError, ProviderId, Role,
};
pub use router::{AiRouter, AiRouterBuilder};
