use async_trait::async_trait;
use lumo_voice::cloud_stt::{
    CloudAudioScope, CloudSttChunk, CloudSttConfig, CloudSttProvider, CloudSttRequest,
    CloudSttTransport, VaultCredentialRef, VoiceSecretResolver,
};
use lumo_voice::provider::{ProviderError, SttEvent, SttProvider};
use lumo_voice::stt_router::VoicePrivacyPolicy;
use lumo_voice::AudioFrame;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

#[derive(Default)]
struct FixtureResolver {
    calls: AtomicUsize,
    keys: Mutex<Vec<String>>,
}

#[async_trait]
impl VoiceSecretResolver for FixtureResolver {
    async fn resolve(&self, reference: &VaultCredentialRef) -> Result<String, ProviderError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.keys.lock().unwrap().push(reference.key().to_string());
        Ok("vault-secret-token".into())
    }
}

#[derive(Clone, Copy)]
enum TransportBehavior {
    Stream,
    Hang,
}

struct FixtureTransport {
    behavior: TransportBehavior,
    requests: Mutex<Vec<CloudSttRequest>>,
    frames: Mutex<Vec<AudioFrame>>,
    cancelled: AtomicUsize,
}

impl FixtureTransport {
    fn streaming() -> Arc<Self> {
        Arc::new(Self {
            behavior: TransportBehavior::Stream,
            requests: Mutex::new(Vec::new()),
            frames: Mutex::new(Vec::new()),
            cancelled: AtomicUsize::new(0),
        })
    }

    fn hanging() -> Arc<Self> {
        Arc::new(Self {
            behavior: TransportBehavior::Hang,
            requests: Mutex::new(Vec::new()),
            frames: Mutex::new(Vec::new()),
            cancelled: AtomicUsize::new(0),
        })
    }
}

#[async_trait]
impl CloudSttTransport for FixtureTransport {
    async fn stream(
        &self,
        request: CloudSttRequest,
        mut audio: mpsc::Receiver<AudioFrame>,
        chunks: mpsc::Sender<CloudSttChunk>,
        cancel: CancellationToken,
    ) -> Result<(), ProviderError> {
        self.requests.lock().unwrap().push(request);
        match self.behavior {
            TransportBehavior::Stream => {
                let mut index = 0;
                while let Some(frame) = tokio::select! {
                    biased;
                    _ = cancel.cancelled() => {
                        self.cancelled.fetch_add(1, Ordering::SeqCst);
                        return Err(ProviderError::Cancelled);
                    }
                    frame = audio.recv() => frame,
                } {
                    self.frames.lock().unwrap().push(frame);
                    index += 1;
                    if index == 1 {
                        chunks
                            .send(CloudSttChunk::Delta("hello".into()))
                            .await
                            .unwrap();
                    }
                }
                chunks
                    .send(CloudSttChunk::Final("hello world".into()))
                    .await
                    .unwrap();
                Ok(())
            }
            TransportBehavior::Hang => {
                cancel.cancelled().await;
                self.cancelled.fetch_add(1, Ordering::SeqCst);
                Err(ProviderError::Cancelled)
            }
        }
    }
}

fn policy() -> VoicePrivacyPolicy {
    VoicePrivacyPolicy {
        cloud_allowed: true,
        retain_transcript: false,
        retain_audio: false,
        max_cloud_seconds: 30,
        max_cost_usd_micro: 1_000,
    }
}

fn config(timeout: Duration) -> CloudSttConfig {
    CloudSttConfig {
        endpoint: "https://api.example.test/v1/audio/transcriptions".into(),
        model: "whisper-1".into(),
        credential: VaultCredentialRef::parse("${{ vault.voice.openai_key }}").unwrap(),
        timeout,
        cost_per_audio_second_usd_micro: 100,
    }
}

#[test]
fn credential_boundary_rejects_literals() {
    let error = VaultCredentialRef::parse("sk-plaintext-secret").unwrap_err();
    assert!(matches!(error, ProviderError::InvalidInput { .. }));
}

fn make_provider(
    transport: Arc<FixtureTransport>,
    resolver: Arc<FixtureResolver>,
    privacy: VoicePrivacyPolicy,
    timeout: Duration,
) -> CloudSttProvider {
    CloudSttProvider::new(config(timeout), privacy, resolver, transport)
}

#[tokio::test]
async fn openai_compatible_stream_resolves_vault_ref_and_uploads_post_wake_frames() {
    let transport = FixtureTransport::streaming();
    let resolver = Arc::new(FixtureResolver::default());
    let provider = make_provider(
        transport.clone(),
        resolver.clone(),
        policy(),
        Duration::from_secs(1),
    );
    let (audio_tx, audio_rx) = mpsc::channel(2);
    let (event_tx, mut event_rx) = mpsc::channel(3);
    audio_tx
        .send(AudioFrame {
            samples: vec![1, 2, 3],
        })
        .await
        .unwrap();
    audio_tx
        .send(AudioFrame {
            samples: vec![4, 5, 6],
        })
        .await
        .unwrap();
    drop(audio_tx);

    provider
        .transcribe(audio_rx, event_tx, CancellationToken::new())
        .await
        .unwrap();

    assert_eq!(
        event_rx.recv().await,
        Some(SttEvent::Partial("hello".into()))
    );
    assert_eq!(
        event_rx.recv().await,
        Some(SttEvent::Final("hello world".into()))
    );
    assert_eq!(resolver.calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        resolver.keys.lock().unwrap().as_slice(),
        ["voice.openai_key"]
    );
    let requests = transport.requests.lock().unwrap();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].authorization, "Bearer vault-secret-token");
    assert!(!format!("{:?}", requests[0]).contains("vault-secret-token"));
    assert_eq!(requests[0].audio_scope, CloudAudioScope::PostWakeOnly);
    assert!(requests[0].stream);
    assert_eq!(transport.frames.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn enterprise_cloud_deny_happens_before_secret_resolution_or_transport() {
    let transport = FixtureTransport::streaming();
    let resolver = Arc::new(FixtureResolver::default());
    let mut privacy = policy();
    privacy.cloud_allowed = false;
    let provider = make_provider(
        transport.clone(),
        resolver.clone(),
        privacy,
        Duration::from_secs(1),
    );
    let (_audio_tx, audio_rx) = mpsc::channel(1);
    let (event_tx, _event_rx) = mpsc::channel(1);

    assert!(matches!(
        provider
            .transcribe(audio_rx, event_tx, CancellationToken::new())
            .await,
        Err(ProviderError::PrivacyDenied)
    ));
    assert_eq!(resolver.calls.load(Ordering::SeqCst), 0);
    assert!(transport.requests.lock().unwrap().is_empty());
}

#[tokio::test]
async fn duration_and_cost_budgets_reject_frames_before_upload() {
    for (privacy, expected_cost) in [
        (
            VoicePrivacyPolicy {
                max_cloud_seconds: 1,
                ..policy()
            },
            false,
        ),
        (
            VoicePrivacyPolicy {
                max_cloud_seconds: 30,
                max_cost_usd_micro: 40,
                ..policy()
            },
            true,
        ),
    ] {
        let transport = FixtureTransport::streaming();
        let resolver = Arc::new(FixtureResolver::default());
        let provider = make_provider(transport.clone(), resolver, privacy, Duration::from_secs(1));
        let (audio_tx, audio_rx) = mpsc::channel(1);
        let (event_tx, _event_rx) = mpsc::channel(1);
        let sample_count = if expected_cost { 8_000 } else { 16_001 };
        audio_tx
            .send(AudioFrame {
                samples: vec![1; sample_count],
            })
            .await
            .unwrap();
        drop(audio_tx);

        let err = provider
            .transcribe(audio_rx, event_tx, CancellationToken::new())
            .await
            .unwrap_err();
        if expected_cost {
            assert!(matches!(err, ProviderError::CostBudgetExceeded { .. }));
        } else {
            assert!(matches!(err, ProviderError::CloudDurationExceeded { .. }));
        }
        assert!(transport.frames.lock().unwrap().is_empty());
    }
}

#[tokio::test]
async fn timeout_and_external_cancellation_stop_the_transport() {
    let transport = FixtureTransport::hanging();
    let resolver = Arc::new(FixtureResolver::default());
    let provider = make_provider(
        transport.clone(),
        resolver.clone(),
        policy(),
        Duration::from_millis(30),
    );
    let (_audio_tx, audio_rx) = mpsc::channel(1);
    let (event_tx, _event_rx) = mpsc::channel(1);
    assert!(matches!(
        provider
            .transcribe(audio_rx, event_tx, CancellationToken::new())
            .await,
        Err(ProviderError::Timeout { .. })
    ));

    let provider = Arc::new(make_provider(
        transport.clone(),
        resolver,
        policy(),
        Duration::from_secs(5),
    ));
    let (_audio_tx, audio_rx) = mpsc::channel(1);
    let (event_tx, _event_rx) = mpsc::channel(1);
    let cancel = CancellationToken::new();
    let task_cancel = cancel.clone();
    let task_provider = provider.clone();
    let task = tokio::spawn(async move {
        task_provider
            .transcribe(audio_rx, event_tx, task_cancel)
            .await
    });
    tokio::task::yield_now().await;
    cancel.cancel();
    assert!(matches!(task.await.unwrap(), Err(ProviderError::Cancelled)));
    assert!(transport.cancelled.load(Ordering::SeqCst) >= 1);
}
