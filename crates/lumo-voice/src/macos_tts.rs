//! Cross-platform contract for the macOS system TTS adapter.
//!
//! A host can supply an AVSpeechSynthesizer-backed [`SystemTtsBackend`] on
//! macOS. Keeping the backend injected lets cancellation, quiet mode and
//! response limits remain deterministic on every CI platform.

use crate::provider::{ProviderError, TtsProvider};
use async_trait::async_trait;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

#[derive(Debug, Clone, PartialEq)]
pub struct MacOsTtsConfig {
    pub quiet: bool,
    pub max_chars: usize,
    pub voice: Option<String>,
    pub rate: f32,
}

impl Default for MacOsTtsConfig {
    fn default() -> Self {
        Self {
            quiet: false,
            max_chars: 280,
            voice: None,
            rate: 0.5,
        }
    }
}

#[async_trait]
pub trait SystemTtsBackend: Send + Sync {
    async fn speak(
        &self,
        text: &str,
        voice: Option<&str>,
        rate: f32,
        cancel: CancellationToken,
    ) -> Result<(), ProviderError>;

    async fn stop(&self);
}

pub fn native_macos_tts_backend() -> Result<Arc<dyn SystemTtsBackend>, ProviderError> {
    Err(ProviderError::NativeUnavailable {
        backend: "AVSpeechSynthesizer".into(),
    })
}

pub struct MacOsTtsProvider {
    backend: Arc<dyn SystemTtsBackend>,
    config: MacOsTtsConfig,
}

impl MacOsTtsProvider {
    pub fn new(backend: Arc<dyn SystemTtsBackend>, config: MacOsTtsConfig) -> Self {
        Self { backend, config }
    }
}

#[async_trait]
impl TtsProvider for MacOsTtsProvider {
    async fn speak(&self, text: &str, cancel: CancellationToken) -> Result<(), ProviderError> {
        if self.config.quiet || text.is_empty() {
            return Ok(());
        }
        let char_count = text.chars().count();
        if char_count > self.config.max_chars {
            return Err(ProviderError::InvalidInput {
                message: format!(
                    "TTS result has {char_count} characters; maximum is {}",
                    self.config.max_chars
                ),
            });
        }
        if cancel.is_cancelled() {
            self.backend.stop().await;
            return Err(ProviderError::Cancelled);
        }

        let operation_cancel = cancel.child_token();
        let operation = self.backend.speak(
            text,
            self.config.voice.as_deref(),
            self.config.rate,
            operation_cancel.clone(),
        );
        tokio::select! {
            biased;
            _ = cancel.cancelled() => {
                operation_cancel.cancel();
                self.backend.stop().await;
                Err(ProviderError::Cancelled)
            }
            result = operation => result,
        }
    }
}
