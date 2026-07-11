use crate::audio::AudioFrame;
use async_trait::async_trait;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SttEvent {
    Partial(String),
    Final(String),
}

#[derive(Debug, thiserror::Error)]
pub enum ProviderError {
    #[error("operation cancelled")]
    Cancelled,
    #[error("provider error: {0}")]
    Other(String),
}

#[async_trait]
pub trait WakeWordProvider: Send + Sync {
    async fn wait_for_wake(&self, cancel: CancellationToken) -> Result<(), ProviderError>;
}
#[async_trait]
pub trait SttProvider: Send + Sync {
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
