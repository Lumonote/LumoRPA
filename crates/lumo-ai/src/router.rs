use crate::{
    config::{ProviderProfile, ProvidersConfig},
    provider::{
        AnthropicProvider, ChatRequest, ChatResponse, LlmProvider,
        OpenAiProvider, ProviderError, ProviderId,
    },
};
use std::sync::Arc;

/// Multi-provider router built either programmatically (`builder()`) or
/// from a `ProvidersConfig` (`from_config()`).
///
/// Model routing rules (first match wins):
///   1. Exact `<profile_name>/<model>` prefix.
///   2. `provider.supports(model)` heuristics (e.g. `gpt-*` → openai).
///   3. Active profile's `default_model` if `req.model` is empty.
#[derive(Clone, Default)]
pub struct AiRouter {
    providers: Vec<Arc<dyn LlmProvider>>,
    active: Option<String>,
    active_default_model: Option<String>,
}

impl AiRouter {
    pub fn builder() -> AiRouterBuilder { AiRouterBuilder::default() }

    pub fn from_config(cfg: &ProvidersConfig) -> Self {
        let mut providers: Vec<Arc<dyn LlmProvider>> = Vec::new();
        for p in &cfg.profiles {
            if let Some(prov) = build_provider(p) {
                providers.push(prov);
            }
        }
        let active_default_model = cfg.active_profile().and_then(|p| p.default_model.clone());
        Self {
            providers,
            active: cfg.active.clone(),
            active_default_model,
        }
    }

    pub async fn chat(&self, mut req: ChatRequest) -> Result<ChatResponse, ProviderError> {
        let used_default = req.model.is_empty();
        if used_default {
            if let Some(m) = &self.active_default_model { req.model = m.clone(); }
        }
        // 1. Explicit `name/...` prefix wins.
        if let Some((pname, _)) = req.model.split_once('/') {
            if let Some(p) = self.providers.iter().find(|p| p.name() == pname) {
                return p.chat(req).await;
            }
        }
        // 2. If the model came from the active profile's default, prefer the
        //    active profile itself (don't let heuristic supports() steal it
        //    — e.g. `gpt-5.5` would otherwise be hijacked by a profile named
        //    "openai" even when active is "picpi").
        if used_default {
            if let Some(name) = &self.active {
                if let Some(p) = self.providers.iter().find(|p| p.name() == name) {
                    return p.chat(req).await;
                }
            }
        }
        // 3. Heuristic supports() (bare `gpt-*`, `claude-*`, ...).
        if let Some(p) = self.providers.iter().find(|p| p.supports(&req.model)) {
            return p.chat(req).await;
        }
        // 4. Fall back to the active profile regardless.
        if let Some(name) = &self.active {
            if let Some(p) = self.providers.iter().find(|p| p.name() == name) {
                return p.chat(req).await;
            }
        }
        Err(ProviderError::Other(format!("no provider for model `{}`", req.model)))
    }

    pub fn provider_names(&self) -> Vec<String> {
        self.providers.iter().map(|p| p.name().to_string()).collect()
    }
    pub fn active(&self) -> Option<&str> { self.active.as_deref() }
    pub fn ids(&self) -> Vec<ProviderId> { self.providers.iter().map(|p| p.id()).collect() }
}

fn build_provider(p: &ProviderProfile) -> Option<Arc<dyn LlmProvider>> {
    match p.kind.as_str() {
        "openai"    => Some(Arc::new(OpenAiProvider::from_profile(p))),
        "anthropic" => Some(Arc::new(AnthropicProvider::from_profile(p))),
        other => {
            tracing::warn!("provider `{}`: unknown kind `{}`, skipping", p.name, other);
            None
        }
    }
}

#[derive(Default)]
pub struct AiRouterBuilder {
    providers: Vec<Arc<dyn LlmProvider>>,
    active: Option<String>,
    active_default_model: Option<String>,
}

impl AiRouterBuilder {
    pub fn with<P: LlmProvider + 'static>(mut self, p: P) -> Self {
        self.providers.push(Arc::new(p));
        self
    }
    pub fn active(mut self, name: impl Into<String>) -> Self {
        self.active = Some(name.into()); self
    }
    pub fn active_default_model(mut self, m: impl Into<String>) -> Self {
        self.active_default_model = Some(m.into()); self
    }
    pub fn build(self) -> AiRouter {
        AiRouter {
            providers: self.providers,
            active: self.active,
            active_default_model: self.active_default_model,
        }
    }
}
