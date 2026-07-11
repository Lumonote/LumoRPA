use super::{app_home, DesktopState};
use async_trait::async_trait;
use lumo_voice::model_installer::{
    install_voice_model_with_downloader, FileModelDownloader, ModelDownloader, VoiceModelError,
    VoiceModelKind, VoiceModelManifest,
};
use serde::Serialize;
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use tauri::{State, Wry};
use tokio::io::AsyncWriteExt;
use tokio_util::sync::CancellationToken;

type AppHandle = tauri::AppHandle<Wry>;

#[derive(Default)]
pub(super) struct VoiceModelRuntime {
    installs: HashMap<String, CancellationToken>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct VoiceModelDto {
    id: String,
    kind: VoiceModelKind,
    status: String,
    active_version: Option<String>,
}

struct DesktopModelDownloader {
    client: reqwest::Client,
}

impl Default for DesktopModelDownloader {
    fn default() -> Self {
        Self {
            client: reqwest::Client::new(),
        }
    }
}

#[async_trait]
impl ModelDownloader for DesktopModelDownloader {
    async fn download(
        &self,
        url: &str,
        destination: &Path,
        cancel: CancellationToken,
    ) -> Result<(), VoiceModelError> {
        if !(url.starts_with("http://") || url.starts_with("https://")) {
            return FileModelDownloader.download(url, destination, cancel).await;
        }
        let parsed = reqwest::Url::parse(url).map_err(|_| VoiceModelError::Task {
            message: "invalid HTTP model URL".into(),
        })?;
        if !parsed.username().is_empty() || parsed.password().is_some() {
            return Err(VoiceModelError::Task {
                message: "HTTP model URL must not contain credentials".into(),
            });
        }
        let mut response = tokio::select! {
            _ = cancel.cancelled() => return Err(VoiceModelError::Cancelled),
            response = self.client.get(parsed).send() => response.map_err(|_| VoiceModelError::Task {
                message: "HTTP model download failed".into(),
            })?,
        };
        if !response.status().is_success() {
            return Err(VoiceModelError::Task {
                message: format!("HTTP model download returned status {}", response.status()),
            });
        }
        let mut output = tokio::fs::File::create(destination).await?;
        loop {
            let chunk = tokio::select! {
                _ = cancel.cancelled() => return Err(VoiceModelError::Cancelled),
                chunk = response.chunk() => chunk.map_err(|_| VoiceModelError::Task {
                    message: "HTTP model download stream failed".into(),
                })?,
            };
            let Some(chunk) = chunk else { break };
            tokio::select! {
                _ = cancel.cancelled() => return Err(VoiceModelError::Cancelled),
                result = output.write_all(&chunk) => result?,
            }
        }
        output.flush().await?;
        output.sync_all().await?;
        Ok(())
    }
}

#[tauri::command(rename_all = "camelCase")]
pub(super) async fn voice_model_install(
    app: AppHandle,
    state: State<'_, DesktopState>,
    model_id: String,
    manifest: Option<VoiceModelManifest>,
) -> Result<VoiceModelDto, String> {
    validate_model_id(&model_id)?;
    let home = app_home(&app)?;
    let manifests = load_manifests(&home)?;
    let manifest = manifest
        .or_else(|| manifests.iter().find(|item| item.id == model_id).cloned())
        .ok_or_else(|| {
            format!(
                "voice model manifest `{model_id}` not found; configure LUMO_VOICE_MODEL_MANIFESTS"
            )
        })?;
    if manifest.id != model_id {
        return Err("voice model manifest id does not match modelId".into());
    }
    let cancel = CancellationToken::new();
    {
        let mut runtime = state
            .voice_models
            .lock()
            .map_err(|_| "voice model runtime is unavailable".to_string())?;
        if runtime.installs.contains_key(&model_id) {
            return Err(format!("voice model `{model_id}` is already installing"));
        }
        runtime.installs.insert(model_id.clone(), cancel.clone());
    }
    let target = models_root(&home).join(&model_id);
    let result = install_voice_model_with_downloader(
        &manifest,
        &target,
        cancel,
        &DesktopModelDownloader::default(),
    )
    .await
    .map_err(|error| error.to_string());
    state
        .voice_models
        .lock()
        .map_err(|_| "voice model runtime is unavailable".to_string())?
        .installs
        .remove(&model_id);
    result?;
    model_dto(&target, &manifest, false)
}

#[tauri::command(rename_all = "camelCase")]
pub(super) async fn voice_model_remove(
    app: AppHandle,
    state: State<'_, DesktopState>,
    model_id: String,
) -> Result<bool, String> {
    validate_model_id(&model_id)?;
    let target = models_root(&app_home(&app)?).join(&model_id);
    let cancel = state
        .voice_models
        .lock()
        .map_err(|_| "voice model runtime is unavailable".to_string())?
        .installs
        .remove(&model_id);
    if let Some(cancel) = cancel {
        cancel.cancel();
    }
    if !target.exists() {
        return Ok(false);
    }
    tokio::fs::remove_dir_all(target)
        .await
        .map_err(|error| error.to_string())?;
    Ok(true)
}

#[tauri::command]
pub(super) fn voice_model_list(
    app: AppHandle,
    state: State<'_, DesktopState>,
) -> Result<Vec<VoiceModelDto>, String> {
    let home = app_home(&app)?;
    let manifests = load_manifests(&home)?;
    let installing = state
        .voice_models
        .lock()
        .map_err(|_| "voice model runtime is unavailable".to_string())?
        .installs
        .keys()
        .cloned()
        .collect::<Vec<_>>();
    let mut listed = list_models_at(&models_root(&home), &manifests)?;
    for model in &mut listed {
        if installing.contains(&model.id) {
            model.status = "installing".into();
        }
    }
    Ok(listed)
}

fn load_manifests(home: &Path) -> Result<Vec<VoiceModelManifest>, String> {
    let path = std::env::var_os("LUMO_VOICE_MODEL_MANIFESTS")
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join("voice-model-manifests.json"));
    load_manifests_from_path(&path)
}

fn load_manifests_from_path(path: &Path) -> Result<Vec<VoiceModelManifest>, String> {
    if !path.exists() {
        return Ok(Vec::new());
    }
    let bytes = std::fs::read(path).map_err(|error| error.to_string())?;
    serde_json::from_slice(&bytes).map_err(|error| error.to_string())
}

fn list_models_at(
    root: &Path,
    manifests: &[VoiceModelManifest],
) -> Result<Vec<VoiceModelDto>, String> {
    let mut by_id = manifests
        .iter()
        .map(|manifest| (manifest.id.clone(), manifest.clone()))
        .collect::<BTreeMap<_, _>>();
    if let Ok(entries) = std::fs::read_dir(root) {
        for entry in entries.flatten() {
            let Some(id) = entry.file_name().to_str().map(str::to_string) else {
                continue;
            };
            by_id.entry(id.clone()).or_insert(VoiceModelManifest {
                id,
                kind: VoiceModelKind::SpeechToText,
                url: String::new(),
                sha256: String::new(),
                unpacked_bytes: 0,
            });
        }
    }
    by_id
        .values()
        .map(|manifest| model_dto(&root.join(&manifest.id), manifest, false))
        .collect()
}

fn model_dto(
    target: &Path,
    manifest: &VoiceModelManifest,
    installing: bool,
) -> Result<VoiceModelDto, String> {
    let active_version = std::fs::read_to_string(target.join("active"))
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty() && target.join("versions").join(value).is_dir());
    Ok(VoiceModelDto {
        id: manifest.id.clone(),
        kind: manifest.kind,
        status: if installing {
            "installing"
        } else if active_version.is_some() {
            "installed"
        } else {
            "missing"
        }
        .into(),
        active_version,
    })
}

#[cfg(test)]
async fn remove_model_at(
    runtime: &mut VoiceModelRuntime,
    model_id: &str,
    target: &Path,
) -> Result<bool, String> {
    if let Some(cancel) = runtime.installs.remove(model_id) {
        cancel.cancel();
    }
    if !target.exists() {
        return Ok(false);
    }
    tokio::fs::remove_dir_all(target)
        .await
        .map_err(|error| error.to_string())?;
    Ok(true)
}

fn models_root(home: &Path) -> PathBuf {
    home.join("voice-models")
}

fn validate_model_id(model_id: &str) -> Result<(), String> {
    if model_id.is_empty()
        || !model_id
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_'))
    {
        return Err("voice model id contains invalid characters".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use lumo_voice::model_installer::{VoiceModelKind, VoiceModelManifest};

    #[test]
    fn manifest_file_and_active_pointer_are_reflected_in_listing() {
        let temp = tempfile::tempdir().unwrap();
        let manifest_path = temp.path().join("manifests.json");
        std::fs::write(
            &manifest_path,
            serde_json::to_vec(&vec![VoiceModelManifest {
                id: "sherpa-stt-en-v1".into(),
                kind: VoiceModelKind::SpeechToText,
                url: "file:///tmp/model.tar".into(),
                sha256: "00".repeat(32),
                unpacked_bytes: 1,
            }])
            .unwrap(),
        )
        .unwrap();
        let target = temp.path().join("models/sherpa-stt-en-v1");
        std::fs::create_dir_all(target.join("versions/version-1")).unwrap();
        std::fs::write(target.join("active"), "version-1\n").unwrap();

        let manifests = load_manifests_from_path(&manifest_path).unwrap();
        let listed = list_models_at(&temp.path().join("models"), &manifests).unwrap();
        let model = listed
            .iter()
            .find(|model| model.id == "sherpa-stt-en-v1")
            .unwrap();
        assert_eq!(model.status, "installed");
        assert_eq!(model.active_version.as_deref(), Some("version-1"));
    }

    #[tokio::test]
    async fn removal_cancels_inflight_install_and_deletes_model_directory() {
        let temp = tempfile::tempdir().unwrap();
        let target = temp.path().join("models/demo");
        std::fs::create_dir_all(&target).unwrap();
        std::fs::write(target.join(".install.part"), b"partial").unwrap();
        let mut runtime = VoiceModelRuntime::default();
        let cancel = CancellationToken::new();
        runtime.installs.insert("demo".into(), cancel.clone());

        remove_model_at(&mut runtime, "demo", &target)
            .await
            .unwrap();
        assert!(cancel.is_cancelled());
        assert!(!target.exists());
    }
}
