//! OpenAI-compatible cloud STT boundary with fail-closed privacy controls.

use crate::audio::{AudioFrame, TARGET_SAMPLE_RATE};
use crate::provider::{ProviderError, SttEvent, SttProvider};
use crate::stt_router::VoicePrivacyPolicy;
use async_trait::async_trait;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VaultCredentialRef {
    key: String,
}

impl VaultCredentialRef {
    pub fn parse(value: &str) -> Result<Self, ProviderError> {
        let inner = value
            .trim()
            .strip_prefix("${{")
            .and_then(|value| value.strip_suffix("}}"))
            .map(str::trim)
            .and_then(|value| value.strip_prefix("vault."))
            .ok_or_else(|| ProviderError::InvalidInput {
                message: "cloud STT credential must be a `${{ vault.* }}` reference".into(),
            })?;
        if inner.is_empty()
            || inner.split('.').any(|segment| {
                segment.is_empty()
                    || !segment.chars().all(|character| {
                        character.is_ascii_alphanumeric() || matches!(character, '_' | '-')
                    })
            })
        {
            return Err(ProviderError::InvalidInput {
                message: "cloud STT Vault key contains invalid characters".into(),
            });
        }
        Ok(Self { key: inner.into() })
    }

    pub fn key(&self) -> &str {
        &self.key
    }
}

#[async_trait]
pub trait VoiceSecretResolver: Send + Sync {
    async fn resolve(&self, reference: &VaultCredentialRef) -> Result<String, ProviderError>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloudAudioScope {
    PostWakeOnly,
}

#[derive(Clone, PartialEq, Eq)]
pub struct CloudSttRequest {
    pub endpoint: String,
    pub model: String,
    pub authorization: String,
    pub sample_rate: u32,
    pub stream: bool,
    pub audio_scope: CloudAudioScope,
}

impl std::fmt::Debug for CloudSttRequest {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("CloudSttRequest")
            .field("endpoint", &self.endpoint)
            .field("model", &self.model)
            .field("authorization", &"<redacted>")
            .field("sample_rate", &self.sample_rate)
            .field("stream", &self.stream)
            .field("audio_scope", &self.audio_scope)
            .finish()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CloudSttChunk {
    Delta(String),
    Final(String),
}

#[async_trait]
pub trait CloudSttTransport: Send + Sync {
    async fn stream(
        &self,
        request: CloudSttRequest,
        audio: mpsc::Receiver<AudioFrame>,
        chunks: mpsc::Sender<CloudSttChunk>,
        cancel: CancellationToken,
    ) -> Result<(), ProviderError>;
}

#[derive(Debug, Clone)]
pub struct CloudSttConfig {
    pub endpoint: String,
    pub model: String,
    pub credential: VaultCredentialRef,
    pub timeout: Duration,
    pub cost_per_audio_second_usd_micro: u64,
}

pub struct CloudSttProvider {
    config: CloudSttConfig,
    privacy: VoicePrivacyPolicy,
    secrets: Arc<dyn VoiceSecretResolver>,
    transport: Arc<dyn CloudSttTransport>,
}

impl CloudSttProvider {
    pub fn new(
        config: CloudSttConfig,
        privacy: VoicePrivacyPolicy,
        secrets: Arc<dyn VoiceSecretResolver>,
        transport: Arc<dyn CloudSttTransport>,
    ) -> Self {
        Self {
            config,
            privacy,
            secrets,
            transport,
        }
    }

    fn validate_readiness(&self) -> Result<(), ProviderError> {
        self.privacy.ensure_cloud_allowed()?;
        if !self.config.endpoint.starts_with("https://") {
            return Err(ProviderError::InvalidInput {
                message: "cloud STT endpoint must use HTTPS".into(),
            });
        }
        if self.config.model.trim().is_empty() {
            return Err(ProviderError::InvalidInput {
                message: "cloud STT model is empty".into(),
            });
        }
        Ok(())
    }

    async fn run(
        &self,
        mut audio: mpsc::Receiver<AudioFrame>,
        events: mpsc::Sender<SttEvent>,
        cancel: CancellationToken,
    ) -> Result<(), ProviderError> {
        let secret = tokio::select! {
            biased;
            _ = cancel.cancelled() => return Err(ProviderError::Cancelled),
            result = self.secrets.resolve(&self.config.credential) => result?,
        };
        if secret.is_empty() {
            return Err(ProviderError::InvalidInput {
                message: "cloud STT Vault credential resolved to an empty secret".into(),
            });
        }
        let request = CloudSttRequest {
            endpoint: self.config.endpoint.clone(),
            model: self.config.model.clone(),
            authorization: format!("Bearer {secret}"),
            sample_rate: TARGET_SAMPLE_RATE,
            stream: true,
            audio_scope: CloudAudioScope::PostWakeOnly,
        };
        let (upload_tx, upload_rx) = mpsc::channel(8);
        let (chunk_tx, mut chunk_rx) = mpsc::channel(8);
        let transport = self.transport.clone();
        let transport_cancel = cancel.child_token();
        let task_cancel = transport_cancel.clone();
        let mut task = AbortOnDrop(tokio::spawn(async move {
            transport
                .stream(request, upload_rx, chunk_tx, task_cancel)
                .await
        }));
        let mut upload = Some(upload_tx);
        let mut uploaded_samples = 0_u64;

        loop {
            tokio::select! {
                biased;
                _ = cancel.cancelled() => {
                    transport_cancel.cancel();
                    return Err(ProviderError::Cancelled);
                }
                result = &mut task.0 => {
                    upload.take();
                    let result = result.map_err(|error| ProviderError::Other(format!("cloud STT transport task failed: {error}")))?;
                    while let Some(chunk) = chunk_rx.recv().await {
                        send_chunk(&events, chunk, &cancel).await?;
                    }
                    return result;
                }
                chunk = chunk_rx.recv() => {
                    if let Some(chunk) = chunk {
                        send_chunk(&events, chunk, &cancel).await?;
                    }
                }
                frame = audio.recv(), if upload.is_some() => {
                    let Some(frame) = frame else {
                        upload.take();
                        continue;
                    };
                    let frame_samples = u64::try_from(frame.samples.len()).unwrap_or(u64::MAX);
                    let projected = uploaded_samples.saturating_add(frame_samples);
                    self.privacy.check_cloud_usage(
                        projected,
                        TARGET_SAMPLE_RATE,
                        self.config.cost_per_audio_second_usd_micro,
                    )?;
                    let sender = upload.as_ref().expect("upload sender guarded above");
                    tokio::select! {
                        biased;
                        _ = cancel.cancelled() => {
                            transport_cancel.cancel();
                            return Err(ProviderError::Cancelled);
                        }
                        result = sender.send(frame) => {
                            result.map_err(|_| ProviderError::Other("cloud STT transport closed audio upload".into()))?;
                        }
                    }
                    uploaded_samples = projected;
                }
            }
        }
    }
}

#[async_trait]
impl SttProvider for CloudSttProvider {
    fn readiness(&self) -> Result<(), ProviderError> {
        self.validate_readiness()
    }

    async fn transcribe(
        &self,
        audio: mpsc::Receiver<AudioFrame>,
        events: mpsc::Sender<SttEvent>,
        cancel: CancellationToken,
    ) -> Result<(), ProviderError> {
        if cancel.is_cancelled() {
            return Err(ProviderError::Cancelled);
        }
        self.validate_readiness()?;
        let operation_cancel = cancel.child_token();
        let operation = self.run(audio, events, operation_cancel.clone());
        tokio::select! {
            biased;
            _ = cancel.cancelled() => {
                operation_cancel.cancel();
                Err(ProviderError::Cancelled)
            }
            result = tokio::time::timeout(self.config.timeout, operation) => {
                match result {
                    Ok(result) => result,
                    Err(_) => {
                        operation_cancel.cancel();
                        Err(ProviderError::Timeout {
                            timeout_ms: self.config.timeout.as_millis().min(u128::from(u64::MAX)) as u64,
                        })
                    }
                }
            }
        }
    }
}

struct AbortOnDrop<T>(tokio::task::JoinHandle<T>);

impl<T> Drop for AbortOnDrop<T> {
    fn drop(&mut self) {
        self.0.abort();
    }
}

async fn send_chunk(
    events: &mpsc::Sender<SttEvent>,
    chunk: CloudSttChunk,
    cancel: &CancellationToken,
) -> Result<(), ProviderError> {
    let event = match chunk {
        CloudSttChunk::Delta(text) => SttEvent::Partial(text),
        CloudSttChunk::Final(text) => SttEvent::Final(text),
    };
    tokio::select! {
        biased;
        _ = cancel.cancelled() => Err(ProviderError::Cancelled),
        result = events.send(event) => result.map_err(|_| ProviderError::Other("STT event receiver closed".into())),
    }
}
