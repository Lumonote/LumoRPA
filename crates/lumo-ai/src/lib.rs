//! LumoRPA AI router with multi-provider adapters (OpenAI-compatible /
//! Anthropic) and a cc-switch style profile config.

pub mod action;
pub mod config;
pub mod provider;
pub mod router;

pub use config::{ConfigError, ProviderProfile, ProvidersConfig};
pub use provider::{
    AnthropicProvider, ChatMessage, ChatRequest, ChatResponse, LlmProvider,
    OpenAiProvider, ProviderError, ProviderId, Role,
};
pub use router::{AiRouter, AiRouterBuilder};
pub use action::ChatAction;
