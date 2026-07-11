use lumo_voice::provider::{ProviderError, SttEvent, SttProvider, WakeWordProvider};
use lumo_voice::sherpa::{
    native_sherpa_backend, sherpa_model_catalog, validate_sherpa_models,
    DeterministicSherpaBackend, SherpaSttProvider, SherpaWakeWordProvider,
};
use lumo_voice::AudioFrame;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

struct TestDir(PathBuf);

impl TestDir {
    fn new(label: &str) -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path =
            std::env::temp_dir().join(format!("lumo-voice-{label}-{}-{nonce}", std::process::id()));
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

#[test]
fn unavailable_native_sherpa_backend_is_typed() {
    let err = match native_sherpa_backend() {
        Ok(_) => panic!("native Sherpa is not linked in this build"),
        Err(err) => err,
    };
    assert!(matches!(
        err,
        ProviderError::NativeUnavailable { ref backend } if backend == "sherpa-onnx"
    ));
}

impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn catalog_lists_wake_and_stt_assets_and_validation_reports_all_missing_ids() {
    let catalog = sherpa_model_catalog();
    assert!(catalog.iter().any(|model| model.id == "sherpa-kws-en-v1"));
    assert!(catalog.iter().any(|model| model.id == "sherpa-stt-en-v1"));
    assert!(catalog.iter().all(|model| !model.required_files.is_empty()));

    let root = TestDir::new("missing-models");
    let err = validate_sherpa_models(root.path(), &catalog).unwrap_err();
    match err {
        ProviderError::ModelMissing { asset_ids } => {
            assert_eq!(asset_ids, vec!["sherpa-kws-en-v1", "sherpa-stt-en-v1"]);
        }
        other => panic!("expected typed ModelMissing, got {other:?}"),
    }
}

#[test]
fn model_validation_accepts_complete_non_empty_asset_directories() {
    let catalog = sherpa_model_catalog();
    let root = TestDir::new("complete-models");
    for model in &catalog {
        let directory = root.path().join(&model.id);
        fs::create_dir_all(&directory).unwrap();
        for relative in &model.required_files {
            let path = directory.join(relative);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, b"fixture-model-data").unwrap();
        }
    }

    validate_sherpa_models(root.path(), &catalog).expect("complete model set");
}

#[tokio::test]
async fn deterministic_wake_backend_reports_hit_miss_and_cancellation() {
    let backend = Arc::new(DeterministicSherpaBackend::new(
        1_000,
        vec!["hel".to_string()],
        "hello".to_string(),
    ));
    let provider = SherpaWakeWordProvider::new(backend.clone());

    let (tx, rx) = mpsc::channel(4);
    tx.send(AudioFrame {
        samples: vec![10, 20, 30],
    })
    .await
    .unwrap();
    tx.send(AudioFrame {
        samples: vec![50, 1_500, 40],
    })
    .await
    .unwrap();
    drop(tx);
    provider
        .wait_for_wake(rx, CancellationToken::new())
        .await
        .expect("threshold crossing should wake");

    let (tx, rx) = mpsc::channel(2);
    tx.send(AudioFrame {
        samples: vec![1, 2, 3],
    })
    .await
    .unwrap();
    drop(tx);
    assert!(matches!(
        provider.wait_for_wake(rx, CancellationToken::new()).await,
        Err(ProviderError::NoWakeDetected)
    ));

    let (_tx, rx) = mpsc::channel(1);
    let cancel = CancellationToken::new();
    cancel.cancel();
    assert!(matches!(
        provider.wait_for_wake(rx, cancel).await,
        Err(ProviderError::Cancelled)
    ));
}

#[tokio::test]
async fn deterministic_stt_backend_emits_partial_and_final_events() {
    let backend = Arc::new(DeterministicSherpaBackend::new(
        1_000,
        vec!["hel".to_string(), "hello wor".to_string()],
        "hello world".to_string(),
    ));
    let provider = SherpaSttProvider::new(backend);
    let (audio_tx, audio_rx) = mpsc::channel(4);
    let (event_tx, mut event_rx) = mpsc::channel(4);

    let task = tokio::spawn(async move {
        provider
            .transcribe(audio_rx, event_tx, CancellationToken::new())
            .await
    });
    audio_tx
        .send(AudioFrame {
            samples: vec![1, 2],
        })
        .await
        .unwrap();
    audio_tx
        .send(AudioFrame {
            samples: vec![3, 4],
        })
        .await
        .unwrap();
    drop(audio_tx);

    assert_eq!(event_rx.recv().await, Some(SttEvent::Partial("hel".into())));
    assert_eq!(
        event_rx.recv().await,
        Some(SttEvent::Partial("hello wor".into()))
    );
    assert_eq!(
        event_rx.recv().await,
        Some(SttEvent::Final("hello world".into()))
    );
    task.await.unwrap().expect("fixture transcription");
}
