use async_trait::async_trait;
use lumo_voice::provider::{ProviderError, SttEvent, SttProvider};
use lumo_voice::stt_router::{SttPreference, SttRouter, SttRouterConfig};
use lumo_voice::AudioFrame;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

struct FixtureStt {
    label: &'static str,
    calls: AtomicUsize,
    ready: Result<(), ProviderError>,
    behavior: FixtureBehavior,
}

enum FixtureBehavior {
    Final,
    Hang,
}

impl FixtureStt {
    fn ready(label: &'static str) -> Arc<Self> {
        Arc::new(Self {
            label,
            calls: AtomicUsize::new(0),
            ready: Ok(()),
            behavior: FixtureBehavior::Final,
        })
    }

    fn hanging(label: &'static str) -> Arc<Self> {
        Arc::new(Self {
            label,
            calls: AtomicUsize::new(0),
            ready: Ok(()),
            behavior: FixtureBehavior::Hang,
        })
    }

    fn missing(label: &'static str) -> Arc<Self> {
        Arc::new(Self {
            label,
            calls: AtomicUsize::new(0),
            ready: Err(ProviderError::ModelMissing {
                asset_ids: vec!["fixture-local".into()],
            }),
            behavior: FixtureBehavior::Final,
        })
    }

    fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl SttProvider for FixtureStt {
    fn readiness(&self) -> Result<(), ProviderError> {
        self.ready.clone()
    }

    async fn transcribe(
        &self,
        mut audio: mpsc::Receiver<AudioFrame>,
        events: mpsc::Sender<SttEvent>,
        cancel: CancellationToken,
    ) -> Result<(), ProviderError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        match self.behavior {
            FixtureBehavior::Final => {
                while tokio::select! {
                    _ = cancel.cancelled() => return Err(ProviderError::Cancelled),
                    frame = audio.recv() => frame,
                }
                .is_some()
                {}
                events
                    .send(SttEvent::Final(self.label.to_string()))
                    .await
                    .map_err(|_| ProviderError::Other("event receiver closed".into()))
            }
            FixtureBehavior::Hang => {
                cancel.cancelled().await;
                Err(ProviderError::Cancelled)
            }
        }
    }
}

async fn run_router(router: &SttRouter) -> Result<SttEvent, ProviderError> {
    let (audio_tx, audio_rx) = mpsc::channel(1);
    let (event_tx, mut event_rx) = mpsc::channel(2);
    audio_tx
        .send(AudioFrame { samples: vec![1] })
        .await
        .unwrap();
    drop(audio_tx);
    router
        .transcribe(audio_rx, event_tx, CancellationToken::new())
        .await?;
    event_rx
        .recv()
        .await
        .ok_or_else(|| ProviderError::Other("missing fixture event".into()))
}

fn config(preference: SttPreference, cloud_allowed: bool) -> SttRouterConfig {
    SttRouterConfig {
        preference,
        cloud_allowed,
        timeout: Duration::from_secs(1),
    }
}

#[tokio::test]
async fn local_first_uses_ready_local_provider() {
    let local = FixtureStt::ready("local");
    let cloud = FixtureStt::ready("cloud");
    let router = SttRouter::new(
        Some(local.clone()),
        Some(cloud.clone()),
        config(SttPreference::LocalFirst, true),
    );

    assert_eq!(
        run_router(&router).await.unwrap(),
        SttEvent::Final("local".into())
    );
    assert_eq!(local.calls(), 1);
    assert_eq!(cloud.calls(), 0);
}

#[tokio::test]
async fn local_first_falls_back_to_cloud_when_local_model_is_missing() {
    let local = FixtureStt::missing("local");
    let cloud = FixtureStt::ready("cloud");
    let router = SttRouter::new(
        Some(local.clone()),
        Some(cloud.clone()),
        config(SttPreference::LocalFirst, true),
    );

    assert_eq!(
        run_router(&router).await.unwrap(),
        SttEvent::Final("cloud".into())
    );
    assert_eq!(local.calls(), 0);
    assert_eq!(cloud.calls(), 1);
}

#[tokio::test]
async fn explicit_cloud_selection_uses_cloud_when_privacy_allows() {
    let local = FixtureStt::ready("local");
    let cloud = FixtureStt::ready("cloud");
    let router = SttRouter::new(
        Some(local.clone()),
        Some(cloud.clone()),
        config(SttPreference::Cloud, true),
    );

    assert_eq!(
        run_router(&router).await.unwrap(),
        SttEvent::Final("cloud".into())
    );
    assert_eq!(local.calls(), 0);
    assert_eq!(cloud.calls(), 1);
}

#[tokio::test]
async fn privacy_denied_cloud_selection_falls_back_to_local() {
    let local = FixtureStt::ready("local");
    let cloud = FixtureStt::ready("cloud");
    let router = SttRouter::new(
        Some(local.clone()),
        Some(cloud.clone()),
        config(SttPreference::Cloud, false),
    );

    assert_eq!(
        run_router(&router).await.unwrap(),
        SttEvent::Final("local".into())
    );
    assert_eq!(local.calls(), 1);
    assert_eq!(cloud.calls(), 0);
}

#[tokio::test]
async fn provider_timeout_is_typed_and_cancels_selected_provider() {
    let local = FixtureStt::hanging("local");
    let router = SttRouter::new(
        Some(local.clone()),
        None,
        SttRouterConfig {
            preference: SttPreference::LocalFirst,
            cloud_allowed: false,
            timeout: Duration::from_millis(50),
        },
    );
    let (audio_tx, audio_rx) = mpsc::channel(1);
    let (event_tx, _event_rx) = mpsc::channel(1);
    drop(audio_tx);

    let err = router
        .transcribe(audio_rx, event_tx, CancellationToken::new())
        .await
        .unwrap_err();
    assert!(matches!(err, ProviderError::Timeout { .. }), "got {err:?}");
    assert_eq!(local.calls(), 1);
}

#[tokio::test]
async fn router_cancellation_unblocks_even_if_provider_hangs() {
    let local = FixtureStt::hanging("local");
    let router = Arc::new(SttRouter::new(
        Some(local),
        None,
        config(SttPreference::LocalFirst, false),
    ));
    let (_audio_tx, audio_rx) = mpsc::channel(1);
    let (event_tx, _event_rx) = mpsc::channel(1);
    let cancel = CancellationToken::new();
    let task_cancel = cancel.clone();
    let task_router = router.clone();
    let task = tokio::spawn(async move {
        task_router
            .transcribe(audio_rx, event_tx, task_cancel)
            .await
    });
    tokio::task::yield_now().await;
    cancel.cancel();

    assert!(matches!(task.await.unwrap(), Err(ProviderError::Cancelled)));
}
