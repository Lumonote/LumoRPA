use crate::audio::AudioFrame;
use crate::provider::{ProviderError, SttEvent, SttProvider};
use async_trait::async_trait;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VoicePrivacyPolicy {
    pub cloud_allowed: bool,
    pub retain_transcript: bool,
    pub retain_audio: bool,
    pub max_cloud_seconds: u64,
    pub max_cost_usd_micro: u64,
}

impl VoicePrivacyPolicy {
    pub fn local_only() -> Self {
        Self {
            cloud_allowed: false,
            retain_transcript: false,
            retain_audio: false,
            max_cloud_seconds: 0,
            max_cost_usd_micro: 0,
        }
    }

    pub fn cloud_unlimited() -> Self {
        Self {
            cloud_allowed: true,
            retain_transcript: false,
            retain_audio: false,
            max_cloud_seconds: 0,
            max_cost_usd_micro: 0,
        }
    }

    pub fn ensure_cloud_allowed(&self) -> Result<(), ProviderError> {
        if self.cloud_allowed {
            Ok(())
        } else {
            Err(ProviderError::PrivacyDenied)
        }
    }

    pub fn check_cloud_usage(
        &self,
        samples: u64,
        sample_rate: u32,
        cost_per_second_usd_micro: u64,
    ) -> Result<(), ProviderError> {
        self.ensure_cloud_allowed()?;
        let sample_rate = u64::from(sample_rate);
        if self.max_cloud_seconds > 0
            && samples > self.max_cloud_seconds.saturating_mul(sample_rate)
        {
            return Err(ProviderError::CloudDurationExceeded {
                limit_seconds: self.max_cloud_seconds,
            });
        }
        let cost = u128::from(samples)
            .saturating_mul(u128::from(cost_per_second_usd_micro))
            .saturating_add(u128::from(sample_rate.saturating_sub(1)))
            / u128::from(sample_rate);
        if self.max_cost_usd_micro > 0 && cost > u128::from(self.max_cost_usd_micro) {
            return Err(ProviderError::CostBudgetExceeded {
                limit_usd_micro: self.max_cost_usd_micro,
            });
        }
        Ok(())
    }
}

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
    privacy: VoicePrivacyPolicy,
}

impl SttRouter {
    pub fn new(
        local: Option<Arc<dyn SttProvider>>,
        cloud: Option<Arc<dyn SttProvider>>,
        config: SttRouterConfig,
    ) -> Self {
        let privacy = if config.cloud_allowed {
            VoicePrivacyPolicy::cloud_unlimited()
        } else {
            VoicePrivacyPolicy::local_only()
        };
        Self::new_with_privacy_policy(local, cloud, config, privacy)
    }

    pub fn new_with_privacy_policy(
        local: Option<Arc<dyn SttProvider>>,
        cloud: Option<Arc<dyn SttProvider>>,
        config: SttRouterConfig,
        privacy: VoicePrivacyPolicy,
    ) -> Self {
        Self {
            local,
            cloud,
            config,
            privacy,
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
                if self.privacy.cloud_allowed {
                    if let Some(cloud) = Self::ready(&self.cloud) {
                        return Ok(cloud);
                    }
                }
                if let Some(local) = &self.local {
                    return local.readiness().map(|()| local.clone());
                }
                Err(ProviderError::Unavailable)
            }
            SttPreference::Cloud if self.privacy.cloud_allowed => {
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
