//! Feature-gated native Sherpa runtime boundary.
//!
//! The runtime is loaded dynamically so the default desktop build never
//! claims native support when the library is absent. A small stable C ABI
//! adapter (`lumo_sherpa_*`) isolates this crate from sherpa-onnx C struct
//! layout changes.

use crate::audio::AudioFrame;
#[cfg(feature = "sherpa-native")]
use crate::audio::TARGET_SAMPLE_RATE;
use crate::model_installer::sha256_hex;
use crate::provider::{ProviderError, SttEvent};
use crate::sherpa::{send_event, SherpaBackend};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

pub const NATIVE_MANIFEST_FILE: &str = "sherpa-native.json";
const WAKE_MODEL_ID: &str = "sherpa-kws-en-v1";
const STT_MODEL_ID: &str = "sherpa-stt-en-v1";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NativeAssetManifest {
    pub path: PathBuf,
    pub sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NativeModelManifest {
    pub id: String,
    pub version: String,
    pub minimum_runtime_version: String,
    pub files: Vec<NativeAssetManifest>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NativeRuntimeManifest {
    pub schema_version: u32,
    pub runtime_version: String,
    pub library: NativeAssetManifest,
    pub models: Vec<NativeModelManifest>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedNativeModel {
    pub id: String,
    pub version: String,
    pub directory: PathBuf,
    pub files: Vec<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedSherpaAssets {
    pub runtime_version: String,
    pub library_path: PathBuf,
    pub wake: ResolvedNativeModel,
    pub stt: ResolvedNativeModel,
}

pub fn read_sherpa_manifest(root: &Path) -> Result<NativeRuntimeManifest, ProviderError> {
    let path = root.join(NATIVE_MANIFEST_FILE);
    let bytes = std::fs::read(&path).map_err(|_| ProviderError::ModelMissing {
        asset_ids: vec![NATIVE_MANIFEST_FILE.into()],
    })?;
    serde_json::from_slice(&bytes).map_err(|error| ProviderError::ModelInvalid {
        asset_id: NATIVE_MANIFEST_FILE.into(),
        reason: error.to_string(),
    })
}

pub fn resolve_sherpa_assets(
    root: &Path,
    manifest: &NativeRuntimeManifest,
) -> Result<ResolvedSherpaAssets, ProviderError> {
    if manifest.schema_version != 1 {
        return Err(invalid_manifest(format!(
            "unsupported schema version {}",
            manifest.schema_version
        )));
    }
    parse_version(&manifest.runtime_version).map_err(invalid_manifest)?;
    let library_path = validate_asset(root, &manifest.library, "sherpa-onnx-runtime")?;

    let mut ids = HashSet::new();
    for model in &manifest.models {
        if !ids.insert(model.id.as_str()) {
            return Err(invalid_manifest(format!(
                "duplicate model id `{}`",
                model.id
            )));
        }
    }
    let missing = [WAKE_MODEL_ID, STT_MODEL_ID]
        .into_iter()
        .filter(|id| !ids.contains(id))
        .map(str::to_string)
        .collect::<Vec<_>>();
    if !missing.is_empty() {
        return Err(ProviderError::ModelMissing { asset_ids: missing });
    }

    let wake = resolve_model(
        root,
        &manifest.runtime_version,
        model_by_id(manifest, WAKE_MODEL_ID),
    )?;
    let stt = resolve_model(
        root,
        &manifest.runtime_version,
        model_by_id(manifest, STT_MODEL_ID),
    )?;
    Ok(ResolvedSherpaAssets {
        runtime_version: manifest.runtime_version.clone(),
        library_path,
        wake,
        stt,
    })
}

fn model_by_id<'a>(manifest: &'a NativeRuntimeManifest, id: &str) -> &'a NativeModelManifest {
    manifest
        .models
        .iter()
        .find(|model| model.id == id)
        .expect("required model ids checked above")
}

fn resolve_model(
    root: &Path,
    runtime_version: &str,
    model: &NativeModelManifest,
) -> Result<ResolvedNativeModel, ProviderError> {
    if model.version.trim().is_empty() {
        return Err(ProviderError::ModelInvalid {
            asset_id: model.id.clone(),
            reason: "model version is empty".into(),
        });
    }
    let runtime = parse_version(runtime_version).map_err(invalid_manifest)?;
    let minimum = parse_version(&model.minimum_runtime_version).map_err(|reason| {
        ProviderError::ModelInvalid {
            asset_id: model.id.clone(),
            reason,
        }
    })?;
    if compare_versions(&runtime, &minimum).is_lt() {
        return Err(ProviderError::ModelInvalid {
            asset_id: model.id.clone(),
            reason: format!(
                "requires runtime {}, installed runtime is {runtime_version}",
                model.minimum_runtime_version
            ),
        });
    }
    if model.files.is_empty() {
        return Err(ProviderError::ModelMissing {
            asset_ids: vec![model.id.clone()],
        });
    }
    let mut files = Vec::with_capacity(model.files.len());
    for asset in &model.files {
        files.push(validate_asset(root, asset, &model.id)?);
    }
    let directory = files
        .first()
        .and_then(|path| path.parent())
        .unwrap_or(root)
        .to_path_buf();
    Ok(ResolvedNativeModel {
        id: model.id.clone(),
        version: model.version.clone(),
        directory,
        files,
    })
}

fn validate_asset(
    root: &Path,
    asset: &NativeAssetManifest,
    asset_id: &str,
) -> Result<PathBuf, ProviderError> {
    if !safe_relative_path(&asset.path) {
        return Err(ProviderError::ModelInvalid {
            asset_id: asset_id.into(),
            reason: format!("unsafe asset path `{}`", asset.path.display()),
        });
    }
    if asset.sha256.len() != 64 || !asset.sha256.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(ProviderError::ModelInvalid {
            asset_id: asset_id.into(),
            reason: "checksum must be 64 hexadecimal characters".into(),
        });
    }
    let path = root.join(&asset.path);
    let bytes = std::fs::read(&path).map_err(|_| ProviderError::ModelMissing {
        asset_ids: vec![asset_id.into()],
    })?;
    let actual = sha256_hex(&bytes);
    if !actual.eq_ignore_ascii_case(&asset.sha256) {
        return Err(ProviderError::ModelInvalid {
            asset_id: asset_id.into(),
            reason: format!(
                "checksum mismatch for `{}`: expected {}, got {actual}",
                asset.path.display(),
                asset.sha256.to_ascii_lowercase()
            ),
        });
    }
    Ok(path)
}

fn safe_relative_path(path: &Path) -> bool {
    !path.as_os_str().is_empty()
        && !path.is_absolute()
        && path
            .components()
            .all(|component| matches!(component, Component::Normal(_)))
}

fn invalid_manifest(reason: String) -> ProviderError {
    ProviderError::ModelInvalid {
        asset_id: NATIVE_MANIFEST_FILE.into(),
        reason,
    }
}

fn parse_version(version: &str) -> Result<Vec<u64>, String> {
    let core = version.split_once('-').map_or(version, |(core, _)| core);
    if core.is_empty() {
        return Err("runtime version is empty".into());
    }
    core.split('.')
        .map(|part| {
            if part.is_empty() {
                Err(format!("invalid version `{version}`"))
            } else {
                part.parse::<u64>()
                    .map_err(|_| format!("invalid version `{version}`"))
            }
        })
        .collect()
}

fn compare_versions(left: &[u64], right: &[u64]) -> std::cmp::Ordering {
    let length = left.len().max(right.len());
    (0..length)
        .map(|index| {
            left.get(index)
                .copied()
                .unwrap_or(0)
                .cmp(&right.get(index).copied().unwrap_or(0))
        })
        .find(|ordering| !ordering.is_eq())
        .unwrap_or(std::cmp::Ordering::Equal)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NativeSttResult {
    Partial(String),
    Final(String),
}

pub trait WakeSession: Send {
    fn accept(&mut self, samples: &[f32]) -> Result<bool, ProviderError>;
}

pub trait SttSession: Send {
    fn accept(&mut self, samples: &[f32]) -> Result<Option<NativeSttResult>, ProviderError>;
    fn finish(&mut self) -> Result<Option<String>, ProviderError>;
}

#[async_trait]
pub trait SherpaNativeEngine: Send + Sync {
    async fn wake_session(
        &self,
        model: &ResolvedNativeModel,
    ) -> Result<Box<dyn WakeSession>, ProviderError>;

    async fn stt_session(
        &self,
        model: &ResolvedNativeModel,
    ) -> Result<Box<dyn SttSession>, ProviderError>;
}

pub struct SherpaNativeBackend {
    assets: ResolvedSherpaAssets,
    engine: Arc<dyn SherpaNativeEngine>,
}

impl SherpaNativeBackend {
    pub fn from_engine(assets: ResolvedSherpaAssets, engine: Arc<dyn SherpaNativeEngine>) -> Self {
        Self { assets, engine }
    }

    #[cfg(feature = "sherpa-native")]
    pub fn load(root: &Path) -> Result<Self, ProviderError> {
        let manifest = read_sherpa_manifest(root)?;
        let assets = resolve_sherpa_assets(root, &manifest)?;
        let engine = Arc::new(dynamic::DynamicSherpaEngine::load(&assets)?);
        Ok(Self::from_engine(assets, engine))
    }

    #[cfg(not(feature = "sherpa-native"))]
    pub fn load(_root: &Path) -> Result<Self, ProviderError> {
        Err(native_unavailable())
    }
}

#[async_trait]
impl SherpaBackend for SherpaNativeBackend {
    async fn wait_for_wake(
        &self,
        mut audio: mpsc::Receiver<AudioFrame>,
        cancel: CancellationToken,
    ) -> Result<(), ProviderError> {
        if cancel.is_cancelled() {
            return Err(ProviderError::Cancelled);
        }
        let mut session = self.engine.wake_session(&self.assets.wake).await?;
        loop {
            let frame = tokio::select! {
                biased;
                _ = cancel.cancelled() => return Err(ProviderError::Cancelled),
                frame = audio.recv() => frame,
            };
            let Some(frame) = frame else {
                return Err(ProviderError::NoWakeDetected);
            };
            let samples = normalized_samples(&frame);
            if session.accept(&samples)? {
                return Ok(());
            }
        }
    }

    async fn transcribe(
        &self,
        mut audio: mpsc::Receiver<AudioFrame>,
        events: mpsc::Sender<SttEvent>,
        cancel: CancellationToken,
    ) -> Result<(), ProviderError> {
        if cancel.is_cancelled() {
            return Err(ProviderError::Cancelled);
        }
        let mut session = self.engine.stt_session(&self.assets.stt).await?;
        let mut final_sent = false;
        loop {
            let frame = tokio::select! {
                biased;
                _ = cancel.cancelled() => return Err(ProviderError::Cancelled),
                frame = audio.recv() => frame,
            };
            let Some(frame) = frame else { break };
            let samples = normalized_samples(&frame);
            if let Some(result) = session.accept(&samples)? {
                let event = match result {
                    NativeSttResult::Partial(text) => SttEvent::Partial(text),
                    NativeSttResult::Final(text) => {
                        final_sent = true;
                        SttEvent::Final(text)
                    }
                };
                send_event(&events, event, &cancel).await?;
            }
        }
        if !final_sent {
            if let Some(text) = session.finish()? {
                send_event(&events, SttEvent::Final(text), &cancel).await?;
            }
        }
        Ok(())
    }
}

fn normalized_samples(frame: &AudioFrame) -> Vec<f32> {
    frame
        .samples
        .iter()
        .map(|sample| f32::from(*sample) / f32::from(i16::MAX))
        .collect()
}

pub fn native_sherpa_backend_at(root: &Path) -> Result<Arc<dyn SherpaBackend>, ProviderError> {
    SherpaNativeBackend::load(root).map(|backend| Arc::new(backend) as Arc<dyn SherpaBackend>)
}

pub fn native_sherpa_backend_from_environment() -> Result<Arc<dyn SherpaBackend>, ProviderError> {
    let root = std::env::var_os("LUMO_SHERPA_HOME").ok_or_else(native_unavailable)?;
    native_sherpa_backend_at(Path::new(&root))
}

fn native_unavailable() -> ProviderError {
    ProviderError::NativeUnavailable {
        backend: "sherpa-onnx".into(),
    }
}

#[cfg(feature = "sherpa-native")]
mod dynamic {
    use super::*;
    use libloading::Library;
    use std::ffi::{c_char, c_void, CStr, CString};

    const TEXT_CAPACITY: usize = 8192;
    type RuntimeVersionFn = unsafe extern "C" fn() -> *const c_char;
    type CreateSessionFn = unsafe extern "C" fn(*const c_char, u32) -> *mut c_void;
    type AcceptFn = unsafe extern "C" fn(*mut c_void, *const f32, usize, *mut c_char, usize) -> i32;
    type FinishFn = unsafe extern "C" fn(*mut c_void, *mut c_char, usize) -> i32;
    type DestroyFn = unsafe extern "C" fn(*mut c_void);

    struct Api {
        _library: Library,
        create_wake: CreateSessionFn,
        wake_accept: AcceptFn,
        destroy_wake: DestroyFn,
        create_stt: CreateSessionFn,
        stt_accept: AcceptFn,
        stt_finish: FinishFn,
        destroy_stt: DestroyFn,
    }

    // Function pointers only target the library retained by `_library`.
    unsafe impl Send for Api {}
    unsafe impl Sync for Api {}

    pub(super) struct DynamicSherpaEngine {
        api: Arc<Api>,
    }

    impl DynamicSherpaEngine {
        pub(super) fn load(assets: &ResolvedSherpaAssets) -> Result<Self, ProviderError> {
            let library = unsafe { Library::new(&assets.library_path) }.map_err(|error| {
                ProviderError::NativeUnavailable {
                    backend: format!("sherpa-onnx: {error}"),
                }
            })?;
            unsafe {
                let runtime_version = *library
                    .get::<RuntimeVersionFn>(b"lumo_sherpa_runtime_version\0")
                    .map_err(symbol_error)?;
                let pointer = runtime_version();
                if pointer.is_null() {
                    return Err(symbol_error("runtime version returned null"));
                }
                let actual = CStr::from_ptr(pointer).to_string_lossy();
                if actual != assets.runtime_version {
                    return Err(ProviderError::NativeUnavailable {
                        backend: format!(
                            "sherpa-onnx runtime version mismatch: manifest {}, library {actual}",
                            assets.runtime_version
                        ),
                    });
                }
                let api = Api {
                    create_wake: *library
                        .get(b"lumo_sherpa_create_wake_session\0")
                        .map_err(symbol_error)?,
                    wake_accept: *library
                        .get(b"lumo_sherpa_wake_accept\0")
                        .map_err(symbol_error)?,
                    destroy_wake: *library
                        .get(b"lumo_sherpa_destroy_wake_session\0")
                        .map_err(symbol_error)?,
                    create_stt: *library
                        .get(b"lumo_sherpa_create_stt_session\0")
                        .map_err(symbol_error)?,
                    stt_accept: *library
                        .get(b"lumo_sherpa_stt_accept\0")
                        .map_err(symbol_error)?,
                    stt_finish: *library
                        .get(b"lumo_sherpa_stt_finish\0")
                        .map_err(symbol_error)?,
                    destroy_stt: *library
                        .get(b"lumo_sherpa_destroy_stt_session\0")
                        .map_err(symbol_error)?,
                    _library: library,
                };
                Ok(Self { api: Arc::new(api) })
            }
        }
    }

    fn symbol_error(error: impl std::fmt::Display) -> ProviderError {
        ProviderError::NativeUnavailable {
            backend: format!("sherpa-onnx ABI: {error}"),
        }
    }

    struct DynamicWakeSession {
        handle: *mut c_void,
        api: Arc<Api>,
    }

    unsafe impl Send for DynamicWakeSession {}

    impl WakeSession for DynamicWakeSession {
        fn accept(&mut self, samples: &[f32]) -> Result<bool, ProviderError> {
            let result = unsafe {
                (self.api.wake_accept)(
                    self.handle,
                    samples.as_ptr(),
                    samples.len(),
                    std::ptr::null_mut(),
                    0,
                )
            };
            match result {
                0 => Ok(false),
                1 => Ok(true),
                code => Err(native_call_error("wake accept", code)),
            }
        }
    }

    impl Drop for DynamicWakeSession {
        fn drop(&mut self) {
            unsafe { (self.api.destroy_wake)(self.handle) }
        }
    }

    struct DynamicSttSession {
        handle: *mut c_void,
        api: Arc<Api>,
    }

    unsafe impl Send for DynamicSttSession {}

    impl SttSession for DynamicSttSession {
        fn accept(&mut self, samples: &[f32]) -> Result<Option<NativeSttResult>, ProviderError> {
            let mut output = vec![0_i8; TEXT_CAPACITY];
            let result = unsafe {
                (self.api.stt_accept)(
                    self.handle,
                    samples.as_ptr(),
                    samples.len(),
                    output.as_mut_ptr(),
                    output.len(),
                )
            };
            output[TEXT_CAPACITY - 1] = 0;
            match result {
                0 => Ok(None),
                1 => Ok(Some(NativeSttResult::Partial(output_text(&output)?))),
                2 => Ok(Some(NativeSttResult::Final(output_text(&output)?))),
                code => Err(native_call_error("STT accept", code)),
            }
        }

        fn finish(&mut self) -> Result<Option<String>, ProviderError> {
            let mut output = vec![0_i8; TEXT_CAPACITY];
            let result =
                unsafe { (self.api.stt_finish)(self.handle, output.as_mut_ptr(), output.len()) };
            output[TEXT_CAPACITY - 1] = 0;
            match result {
                0 => Ok(None),
                2 => Ok(Some(output_text(&output)?)),
                code => Err(native_call_error("STT finish", code)),
            }
        }
    }

    impl Drop for DynamicSttSession {
        fn drop(&mut self) {
            unsafe { (self.api.destroy_stt)(self.handle) }
        }
    }

    fn output_text(output: &[c_char]) -> Result<String, ProviderError> {
        let text = unsafe { CStr::from_ptr(output.as_ptr()) }
            .to_str()
            .map_err(|error| ProviderError::Other(format!("invalid native STT UTF-8: {error}")))?;
        Ok(text.to_owned())
    }

    fn native_call_error(operation: &str, code: i32) -> ProviderError {
        ProviderError::Other(format!("native Sherpa {operation} failed with code {code}"))
    }

    fn create_session(
        create: CreateSessionFn,
        model: &ResolvedNativeModel,
    ) -> Result<*mut c_void, ProviderError> {
        let directory =
            CString::new(model.directory.to_string_lossy().as_bytes()).map_err(|_| {
                ProviderError::ModelInvalid {
                    asset_id: model.id.clone(),
                    reason: "model directory contains a NUL byte".into(),
                }
            })?;
        let handle = unsafe { create(directory.as_ptr(), TARGET_SAMPLE_RATE) };
        if handle.is_null() {
            Err(ProviderError::ModelInvalid {
                asset_id: model.id.clone(),
                reason: "native runtime rejected the model".into(),
            })
        } else {
            Ok(handle)
        }
    }

    #[async_trait]
    impl SherpaNativeEngine for DynamicSherpaEngine {
        async fn wake_session(
            &self,
            model: &ResolvedNativeModel,
        ) -> Result<Box<dyn WakeSession>, ProviderError> {
            let handle = create_session(self.api.create_wake, model)?;
            Ok(Box::new(DynamicWakeSession {
                handle,
                api: self.api.clone(),
            }))
        }

        async fn stt_session(
            &self,
            model: &ResolvedNativeModel,
        ) -> Result<Box<dyn SttSession>, ProviderError> {
            let handle = create_session(self.api.create_stt, model)?;
            Ok(Box::new(DynamicSttSession {
                handle,
                api: self.api.clone(),
            }))
        }
    }
}
