use crate::audio::AudioFrame;
use crate::provider::{ProviderError, SttEvent, SttProvider};
use async_trait::async_trait;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SttPreference {
    LocalFirst,
    Cloud,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SttRouterConfig {
    pub preference: SttPreference,
    pub cloud_allowed: bool,
    pub timeout: Duration,
}

pub struct SttRouter {
    local: Option<Arc<dyn SttProvider>>,
    cloud: Option<Arc<dyn SttProvider>>,
    config: SttRouterConfig,
}

impl SttRouter {
    pub fn new(
        local: Option<Arc<dyn SttProvider>>,
        cloud: Option<Arc<dyn SttProvider>>,
        config: SttRouterConfig,
    ) -> Self {
        Self {
            local,
            cloud,
            config,
        }
    }

    fn ready(provider: &Option<Arc<dyn SttProvider>>) -> Option<Arc<dyn SttProvider>> {
        provider
            .as_ref()
            .filter(|provider| provider.readiness().is_ok())
            .cloned()
    }

    fn select_provider(&self) -> Result<Arc<dyn SttProvider>, ProviderError> {
        match self.config.preference {
            SttPreference::LocalFirst => {
                if let Some(local) = Self::ready(&self.local) {
                    return Ok(local);
                }
                if self.config.cloud_allowed {
                    if let Some(cloud) = Self::ready(&self.cloud) {
                        return Ok(cloud);
                    }
                }
                if let Some(local) = &self.local {
                    return local.readiness().map(|()| local.clone());
                }
                Err(ProviderError::Unavailable)
            }
            SttPreference::Cloud if self.config.cloud_allowed => {
                let cloud = self.cloud.as_ref().ok_or(ProviderError::Unavailable)?;
                cloud.readiness()?;
                Ok(cloud.clone())
            }
            SttPreference::Cloud => {
                if let Some(local) = Self::ready(&self.local) {
                    Ok(local)
                } else {
                    Err(ProviderError::PrivacyDenied)
                }
            }
        }
    }
}

#[async_trait]
impl SttProvider for SttRouter {
    fn readiness(&self) -> Result<(), ProviderError> {
        self.select_provider().map(|_| ())
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
        let provider = self.select_provider()?;
        let operation_cancel = cancel.child_token();
        let operation = provider.transcribe(audio, events, operation_cancel.clone());
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
