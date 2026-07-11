use async_trait::async_trait;
use lumo_voice::model_installer::sha256_hex;
use lumo_voice::provider::{ProviderError, SttEvent};
use lumo_voice::sherpa::SherpaBackend;
use lumo_voice::sherpa_native::{
    resolve_sherpa_assets, NativeAssetManifest, NativeModelManifest, NativeRuntimeManifest,
    NativeSttResult, SherpaNativeBackend, SherpaNativeEngine, SttSession, WakeSession,
};
use lumo_voice::AudioFrame;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
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
        let path = std::env::temp_dir().join(format!(
            "lumo-sherpa-native-{label}-{}-{nonce}",
            std::process::id()
        ));
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn write_asset(root: &Path, relative: &str, bytes: &[u8]) -> NativeAssetManifest {
    let path = root.join(relative);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, bytes).unwrap();
    NativeAssetManifest {
        path: relative.into(),
        sha256: sha256_hex(bytes),
    }
}

fn valid_manifest(root: &Path) -> NativeRuntimeManifest {
    let library = write_asset(root, "runtime/libsherpa-lumo.dylib", b"runtime");
    let wake_files = vec![
        write_asset(root, "models/wake/encoder.onnx", b"wake-encoder"),
        write_asset(root, "models/wake/decoder.onnx", b"wake-decoder"),
        write_asset(root, "models/wake/joiner.onnx", b"wake-joiner"),
        write_asset(root, "models/wake/tokens.txt", b"tokens"),
        write_asset(root, "models/wake/keywords.txt", b"hey lumo"),
    ];
    let stt_files = vec![
        write_asset(root, "models/stt/encoder.onnx", b"stt-encoder"),
        write_asset(root, "models/stt/decoder.onnx", b"stt-decoder"),
        write_asset(root, "models/stt/joiner.onnx", b"stt-joiner"),
        write_asset(root, "models/stt/tokens.txt", b"tokens"),
    ];
    NativeRuntimeManifest {
        schema_version: 1,
        runtime_version: "1.10.0".into(),
        library,
        models: vec![
            NativeModelManifest {
                id: "sherpa-kws-en-v1".into(),
                version: "1.0.0".into(),
                minimum_runtime_version: "1.9.0".into(),
                files: wake_files,
            },
            NativeModelManifest {
                id: "sherpa-stt-en-v1".into(),
                version: "1.0.0".into(),
                minimum_runtime_version: "1.10.0".into(),
                files: stt_files,
            },
        ],
    }
}

#[test]
fn manifest_maps_runtime_wake_and_stt_assets() {
    let root = TestDir::new("mapping");
    let manifest = valid_manifest(root.path());
    let resolved = resolve_sherpa_assets(root.path(), &manifest).expect("valid assets");

    assert_eq!(resolved.runtime_version, "1.10.0");
    assert_eq!(
        resolved.library_path,
        root.path().join(&manifest.library.path)
    );
    assert_eq!(resolved.wake.id, "sherpa-kws-en-v1");
    assert_eq!(resolved.stt.id, "sherpa-stt-en-v1");
    assert_eq!(resolved.wake.files.len(), 5);
    assert_eq!(resolved.stt.files.len(), 4);
}

#[test]
fn missing_model_and_runtime_or_model_checksum_failures_are_typed() {
    let root = TestDir::new("invalid-assets");
    let mut manifest = valid_manifest(root.path());
    manifest
        .models
        .retain(|model| model.id != "sherpa-stt-en-v1");
    let err = resolve_sherpa_assets(root.path(), &manifest).unwrap_err();
    assert!(matches!(
        err,
        ProviderError::ModelMissing { asset_ids } if asset_ids == vec!["sherpa-stt-en-v1"]
    ));

    let mut manifest = valid_manifest(root.path());
    manifest.library.sha256 = "00".repeat(32);
    let err = resolve_sherpa_assets(root.path(), &manifest).unwrap_err();
    assert!(matches!(
        err,
        ProviderError::ModelInvalid { asset_id, reason }
            if asset_id == "sherpa-onnx-runtime" && reason.contains("checksum")
    ));

    let mut manifest = valid_manifest(root.path());
    manifest.models[0].files[0].sha256 = "00".repeat(32);
    let err = resolve_sherpa_assets(root.path(), &manifest).unwrap_err();
    assert!(matches!(
        err,
        ProviderError::ModelInvalid { asset_id, reason }
            if asset_id == "sherpa-kws-en-v1" && reason.contains("checksum")
    ));
}

#[test]
fn incompatible_runtime_version_is_rejected_before_sessions() {
    let root = TestDir::new("runtime-version");
    let mut manifest = valid_manifest(root.path());
    manifest.runtime_version = "1.8.0".into();
    let err = resolve_sherpa_assets(root.path(), &manifest).unwrap_err();
    assert!(matches!(
        err,
        ProviderError::ModelInvalid { asset_id, reason }
            if asset_id == "sherpa-kws-en-v1" && reason.contains("requires runtime")
    ));
}

#[cfg(feature = "sherpa-native")]
#[test]
fn invalid_dynamic_library_is_native_unavailable_not_success() {
    let root = TestDir::new("missing-library");
    let manifest = valid_manifest(root.path());
    fs::write(
        root.path()
            .join(lumo_voice::sherpa_native::NATIVE_MANIFEST_FILE),
        serde_json::to_vec_pretty(&manifest).unwrap(),
    )
    .unwrap();

    let err = match SherpaNativeBackend::load(root.path()) {
        Ok(_) => panic!("invalid dynamic library must never report native readiness"),
        Err(error) => error,
    };
    assert!(matches!(
        err,
        ProviderError::NativeUnavailable { backend } if backend.contains("sherpa-onnx")
    ));
}

struct FakeWakeSession {
    calls: Arc<Mutex<usize>>,
}

impl WakeSession for FakeWakeSession {
    fn accept(&mut self, samples: &[f32]) -> Result<bool, ProviderError> {
        *self.calls.lock().unwrap() += 1;
        Ok(samples.iter().any(|sample| *sample > 0.5))
    }
}

struct FakeSttSession {
    calls: usize,
}

impl SttSession for FakeSttSession {
    fn accept(&mut self, _samples: &[f32]) -> Result<Option<NativeSttResult>, ProviderError> {
        self.calls += 1;
        Ok(match self.calls {
            1 => Some(NativeSttResult::Partial("你好".into())),
            2 => Some(NativeSttResult::Final("你好 Lumo".into())),
            _ => None,
        })
    }

    fn finish(&mut self) -> Result<Option<String>, ProviderError> {
        Ok(None)
    }
}

struct FakeEngine {
    wake_calls: Arc<Mutex<usize>>,
}

#[async_trait]
impl SherpaNativeEngine for FakeEngine {
    async fn wake_session(
        &self,
        _model: &lumo_voice::sherpa_native::ResolvedNativeModel,
    ) -> Result<Box<dyn WakeSession>, ProviderError> {
        Ok(Box::new(FakeWakeSession {
            calls: self.wake_calls.clone(),
        }))
    }

    async fn stt_session(
        &self,
        _model: &lumo_voice::sherpa_native::ResolvedNativeModel,
    ) -> Result<Box<dyn SttSession>, ProviderError> {
        Ok(Box::new(FakeSttSession { calls: 0 }))
    }
}

fn backend(root: &Path, wake_calls: Arc<Mutex<usize>>) -> SherpaNativeBackend {
    let assets = resolve_sherpa_assets(root, &valid_manifest(root)).unwrap();
    SherpaNativeBackend::from_engine(assets, Arc::new(FakeEngine { wake_calls }))
}

#[tokio::test]
async fn native_sessions_emit_wake_partial_final_and_honor_cancellation() {
    let root = TestDir::new("sessions");
    let wake_calls = Arc::new(Mutex::new(0));
    let backend = backend(root.path(), wake_calls.clone());
    let (audio_tx, audio_rx) = mpsc::channel(2);
    audio_tx
        .send(AudioFrame {
            samples: vec![0, i16::MAX],
        })
        .await
        .unwrap();
    drop(audio_tx);
    backend
        .wait_for_wake(audio_rx, CancellationToken::new())
        .await
        .expect("wake hit");
    assert_eq!(*wake_calls.lock().unwrap(), 1);

    let (audio_tx, audio_rx) = mpsc::channel(3);
    let (event_tx, mut event_rx) = mpsc::channel(3);
    audio_tx
        .send(AudioFrame { samples: vec![1] })
        .await
        .unwrap();
    audio_tx
        .send(AudioFrame { samples: vec![2] })
        .await
        .unwrap();
    drop(audio_tx);
    backend
        .transcribe(audio_rx, event_tx, CancellationToken::new())
        .await
        .expect("native transcription");
    assert_eq!(
        event_rx.recv().await,
        Some(SttEvent::Partial("你好".into()))
    );
    assert_eq!(
        event_rx.recv().await,
        Some(SttEvent::Final("你好 Lumo".into()))
    );

    let (_audio_tx, audio_rx) = mpsc::channel(1);
    let (event_tx, _event_rx) = mpsc::channel(1);
    let cancel = CancellationToken::new();
    cancel.cancel();
    assert!(matches!(
        backend.transcribe(audio_rx, event_tx, cancel).await,
        Err(ProviderError::Cancelled)
    ));
}
