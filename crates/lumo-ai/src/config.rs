//! Provider configuration — cc-switch style profiles.
//!
//! Lives at `~/.lumorpa/providers.toml`:
//!
//! ```toml
//! active = "deepseek"
//!
//! [[providers]]
//! name = "openai"
//! kind = "openai"
//! base_url = "https://api.openai.com/v1"
//! api_key_env = "OPENAI_API_KEY"
//! default_model = "gpt-4o"
//!
//! [[providers]]
//! name = "deepseek"
//! kind = "openai"                       # OpenAI-compatible
//! base_url = "https://api.deepseek.com/v1"
//! api_key_env = "DEEPSEEK_API_KEY"
//! default_model = "deepseek-chat"
//! models = ["deepseek-chat", "deepseek-reasoner"]
//!
//! [[providers]]
//! name = "ollama"
//! kind = "openai"
//! base_url = "http://localhost:11434/v1"
//! api_key = "ollama"                    # inline (NOT recommended)
//! default_model = "qwen2.5:7b"
//!
//! [[providers]]
//! name = "claude"
//! kind = "anthropic"
//! base_url = "https://api.anthropic.com"
//! api_key_env = "ANTHROPIC_API_KEY"
//! default_model = "claude-sonnet-4-6"
//! ```

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("toml parse: {0}")]
    TomlDe(#[from] toml::de::Error),
    #[error("toml emit: {0}")]
    TomlSer(#[from] toml::ser::Error),
    #[error("profile `{0}` not found")]
    NotFound(String),
    #[error("profile `{0}` already exists")]
    Exists(String),
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ProvidersConfig {
    /// Name of the profile used when a flow doesn't specify a model explicitly.
    #[serde(default)]
    pub active: Option<String>,
    #[serde(default, rename = "providers")]
    pub profiles: Vec<ProviderProfile>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderProfile {
    /// Profile alias (e.g. "deepseek"). Used in CLI + model prefix routing.
    pub name: String,
    /// Backend protocol family: "openai" | "anthropic".
    pub kind: String,
    /// For kind=openai: which wire API to use. Defaults to "chat".
    ///   * "chat"      → POST {base_url}/chat/completions  (classic)
    ///   * "responses" → POST {base_url}/responses         (OpenAI Responses API, 2024+)
    #[serde(default)]
    pub wire_api: Option<String>,
    #[serde(default)]
    pub base_url: Option<String>,
    /// Inline API key (NOT recommended; prefer `api_key_env`).
    #[serde(default)]
    pub api_key: Option<String>,
    /// Env var name that holds the API key.
    #[serde(default)]
    pub api_key_env: Option<String>,
    #[serde(default)]
    pub default_model: Option<String>,
    /// Optional list of supported model ids (purely informational; the
    /// router does not enforce this).
    #[serde(default)]
    pub models: Vec<String>,
    /// Extra headers (e.g. `OpenAI-Beta`, `anthropic-version`).
    #[serde(default)]
    pub headers: std::collections::BTreeMap<String, String>,
    /// Optional reasoning_effort hint (passed to Responses API; ignored
    /// by classic chat/completions).
    #[serde(default)]
    pub reasoning_effort: Option<String>,
    /// Free-form notes shown by `lumo providers show`.
    #[serde(default)]
    pub notes: Option<String>,
}

impl ProviderProfile {
    pub fn resolve_api_key(&self) -> Option<String> {
        if let Some(k) = &self.api_key { return Some(k.clone()); }
        if let Some(env) = &self.api_key_env {
            return std::env::var(env).ok();
        }
        // Fallback to well-known env var by kind.
        match self.kind.as_str() {
            "openai"    => std::env::var("OPENAI_API_KEY").ok(),
            "anthropic" => std::env::var("ANTHROPIC_AUTH_TOKEN")
                .ok()
                .or_else(|| std::env::var("ANTHROPIC_API_KEY").ok()),
            _ => None,
        }
    }

    pub fn redacted(&self) -> Self {
        let mut c = self.clone();
        if c.api_key.is_some() { c.api_key = Some("***".into()); }
        c
    }
}

impl ProvidersConfig {
    pub fn default_path() -> PathBuf {
        if let Ok(p) = std::env::var("LUMO_PROVIDERS_PATH") {
            return PathBuf::from(p);
        }
        if let Ok(p) = std::env::var("LUMO_HOME") {
            return PathBuf::from(p).join("providers.toml");
        }
        dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(".lumorpa")
            .join("providers.toml")
    }

    pub fn load(path: impl AsRef<Path>) -> Result<Self, ConfigError> {
        let p = path.as_ref();
        if !p.exists() { return Ok(Self::default()); }
        let s = std::fs::read_to_string(p)?;
        let cfg: ProvidersConfig = toml::from_str(&s)?;
        Ok(cfg)
    }

    pub fn save(&self, path: impl AsRef<Path>) -> Result<(), ConfigError> {
        let p = path.as_ref();
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let s = toml::to_string_pretty(self)?;
        std::fs::write(p, s)?;
        Ok(())
    }

    /// Bootstrap a fresh config with starter profiles for the most common
    /// real-world backends. No mock — flows that need a model must talk to
    /// a real endpoint (with `LUMO_ALLOW_LLM_NETWORK=1`).
    pub fn seed_default() -> Self {
        Self {
            active: Some("anthropic".into()),
            profiles: vec![
                ProviderProfile {
                    name: "openai".into(),
                    kind: "openai".into(),
                    wire_api: Some("chat".into()),
                    base_url: Some("https://api.openai.com/v1".into()),
                    api_key: None,
                    api_key_env: Some("OPENAI_API_KEY".into()),
                    default_model: Some("gpt-4o-mini".into()),
                    models: vec!["gpt-4o".into(), "gpt-4o-mini".into(), "o1-mini".into()],
                    headers: Default::default(),
                    reasoning_effort: None,
                    notes: Some("Official OpenAI; set OPENAI_API_KEY in env.".into()),
                },
                ProviderProfile {
                    name: "anthropic".into(),
                    kind: "anthropic".into(),
                    wire_api: None,
                    base_url: Some("https://api.anthropic.com".into()),
                    api_key: None,
                    api_key_env: Some("ANTHROPIC_AUTH_TOKEN".into()),
                    default_model: Some("claude-opus-4-7".into()),
                    models: vec!["claude-opus-4-7".into(), "claude-sonnet-4-6".into(), "claude-haiku-4-5-20251001".into()],
                    headers: Default::default(),
                    reasoning_effort: None,
                    notes: Some(
                        "Anthropic Messages API. Set ANTHROPIC_AUTH_TOKEN \
                         (Bearer) or ANTHROPIC_API_KEY (x-api-key); both work.".into()
                    ),
                },
                ProviderProfile {
                    name: "deepseek".into(),
                    kind: "openai".into(),
                    wire_api: Some("chat".into()),
                    base_url: Some("https://api.deepseek.com/v1".into()),
                    api_key: None,
                    api_key_env: Some("DEEPSEEK_API_KEY".into()),
                    default_model: Some("deepseek-chat".into()),
                    models: vec!["deepseek-chat".into(), "deepseek-reasoner".into()],
                    headers: Default::default(),
                    reasoning_effort: None,
                    notes: Some("DeepSeek (OpenAI-compatible chat/completions).".into()),
                },
                ProviderProfile {
                    name: "ollama".into(),
                    kind: "openai".into(),
                    wire_api: Some("chat".into()),
                    base_url: Some("http://localhost:11434/v1".into()),
                    api_key: Some("ollama".into()),
                    api_key_env: None,
                    default_model: Some("qwen2.5:7b".into()),
                    models: vec![],
                    headers: Default::default(),
                    reasoning_effort: None,
                    notes: Some("Local Ollama via OpenAI-compatible endpoint.".into()),
                },
            ],
        }
    }

    pub fn get(&self, name: &str) -> Option<&ProviderProfile> {
        self.profiles.iter().find(|p| p.name == name)
    }

    pub fn upsert(&mut self, p: ProviderProfile) {
        if let Some(slot) = self.profiles.iter_mut().find(|x| x.name == p.name) {
            *slot = p;
        } else {
            self.profiles.push(p);
        }
    }

    pub fn remove(&mut self, name: &str) -> Result<(), ConfigError> {
        let before = self.profiles.len();
        self.profiles.retain(|p| p.name != name);
        if self.profiles.len() == before {
            return Err(ConfigError::NotFound(name.into()));
        }
        if self.active.as_deref() == Some(name) {
            self.active = self.profiles.first().map(|p| p.name.clone());
        }
        Ok(())
    }

    pub fn use_(&mut self, name: &str) -> Result<(), ConfigError> {
        if self.get(name).is_none() { return Err(ConfigError::NotFound(name.into())); }
        self.active = Some(name.to_string());
        Ok(())
    }

    pub fn active_profile(&self) -> Option<&ProviderProfile> {
        self.active.as_ref().and_then(|n| self.get(n))
    }
}
