use crate::audio::AudioFrame;
use async_trait::async_trait;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SttEvent {
    Partial(String),
    Final(String),
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ProviderError {
    #[error("operation cancelled")]
    Cancelled,
    #[error("required voice models are missing: {asset_ids:?}")]
    ModelMissing { asset_ids: Vec<String> },
    #[error("voice model `{asset_id}` is invalid: {reason}")]
    ModelInvalid { asset_id: String, reason: String },
    #[error("wake word was not detected before the audio stream ended")]
    NoWakeDetected,
    #[error("provider timed out after {timeout_ms}ms")]
    Timeout { timeout_ms: u64 },
    #[error("cloud speech recognition is disabled by privacy policy")]
    PrivacyDenied,
    #[error("cloud speech duration budget exceeded ({limit_seconds}s)")]
    CloudDurationExceeded { limit_seconds: u64 },
    #[error("cloud speech cost budget exceeded ({limit_usd_micro} micro-USD)")]
    CostBudgetExceeded { limit_usd_micro: u64 },
    #[error("no speech recognition provider is available")]
    Unavailable,
    #[error("native voice backend `{backend}` is unavailable in this build")]
    NativeUnavailable { backend: String },
    #[error("invalid provider input: {message}")]
    InvalidInput { message: String },
    #[error("provider error: {0}")]
    Other(String),
}

#[async_trait]
pub trait WakeWordProvider: Send + Sync {
    async fn wait_for_wake(
        &self,
        audio: mpsc::Receiver<AudioFrame>,
        cancel: CancellationToken,
    ) -> Result<(), ProviderError>;
}
#[async_trait]
pub trait SttProvider: Send + Sync {
    fn readiness(&self) -> Result<(), ProviderError> {
        Ok(())
    }

    async fn transcribe(
        &self,
        audio: mpsc::Receiver<AudioFrame>,
        events: mpsc::Sender<SttEvent>,
        cancel: CancellationToken,
    ) -> Result<(), ProviderError>;
}
#[async_trait]
pub trait TtsProvider: Send + Sync {
    async fn speak(&self, text: &str, cancel: CancellationToken) -> Result<(), ProviderError>;
}
#[async_trait]
pub trait AudioCapture: Send + Sync {
    async fn capture(
        &self,
        frames: mpsc::Sender<AudioFrame>,
        cancel: CancellationToken,
    ) -> Result<(), ProviderError>;
}
