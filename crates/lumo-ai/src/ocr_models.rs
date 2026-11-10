//! Local OCR model catalog, ModelScope download helpers, and lightweight
//! external-worker execution for downloaded OCR/VLM models.

use bytes::Bytes;
use lumo_core::ai_hook::AiCallUsage;
use lumo_core::error::StepError;
use serde::Serialize;
use std::{
    collections::{HashMap, HashSet},
    hash::{Hash, Hasher},
    path::{Component, Path, PathBuf},
    process::Stdio,
    sync::{Mutex, OnceLock},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use tokio::process::Command;

const MODELSCOPE_PREFIX: &str = "modelscope/";
const DEFAULT_TIMEOUT_SECS: u64 = 600;

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OcrModelPreset {
    pub id: &'static str,
    pub label: &'static str,
    pub repo: &'static str,
    pub engine: &'static str,
    pub size_hint: &'static str,
    pub description: &'static str,
    pub runnable: bool,
    pub recommended: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OcrModelStatus {
    pub preset: OcrModelPreset,
    pub cache_dir: String,
    pub downloaded: bool,
    pub download_state: &'static str,
    pub download_reason: Option<String>,
    pub runtime_state: &'static str,
    pub runtime_reason: Option<String>,
    pub inference_state: &'static str,
    pub inference_reason: Option<String>,
}

#[derive(Clone, Default)]
struct ModelDiagnostics {
    fingerprint: u64,
    runtime_checked: bool,
    runtime_error: Option<String>,
    inference_checked: bool,
    inference_error: Option<String>,
}

static DIAGNOSTICS: OnceLock<Mutex<HashMap<PathBuf, ModelDiagnostics>>> = OnceLock::new();
static DOWNLOAD_LOCK: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();

struct StagingCleanup(PathBuf);
impl Drop for StagingCleanup {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

struct TemporaryImage(PathBuf);
impl Drop for TemporaryImage {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OcrModelDownload {
    pub model: OcrModelStatus,
    pub stdout_tail: String,
    pub stderr_tail: String,
}

pub fn ocr_model_presets() -> &'static [OcrModelPreset] {
    &[
        OcrModelPreset {
            id: "modelscope/PaddlePaddle/PaddleOCR-VL-1.6",
            label: "PaddleOCR-VL-1.6",
            repo: "PaddlePaddle/PaddleOCR-VL-1.6",
            engine: "paddleocr-vl",
            size_hint: "VLM",
            description: "PaddleOCR-VL latest document OCR/VL model from ModelScope.",
            runnable: true,
            recommended: true,
        },
        OcrModelPreset {
            id: "modelscope/ZhipuAI/GLM-OCR",
            label: "GLM-OCR",
            repo: "ZhipuAI/GLM-OCR",
            engine: "glm-ocr",
            size_hint: "VLM",
            description: "ZhipuAI OCR vision-language model from ModelScope.",
            runnable: true,
            recommended: true,
        },
        OcrModelPreset {
            id: "modelscope/PaddlePaddle/PaddleOCR-VL-1.5",
            label: "PaddleOCR-VL-1.5",
            repo: "PaddlePaddle/PaddleOCR-VL-1.5",
            engine: "paddleocr-vl",
            size_hint: "VLM",
            description: "Previous PaddleOCR-VL model version for compatibility testing.",
            runnable: true,
            recommended: false,
        },
        OcrModelPreset {
            id: "modelscope/deepseek-ai/DeepSeek-OCR-2",
            label: "DeepSeek-OCR-2",
            repo: "deepseek-ai/DeepSeek-OCR-2",
            engine: "deepseek-ocr",
            size_hint: "VLM",
            description: "DeepSeek OCR model from ModelScope; requires its Python runtime deps.",
            runnable: true,
            recommended: false,
        },
    ]
}

/// Catalog of the preset OCR models with download status. `home` is the
/// LumoRPA data dir the cache lives under (P2-2: hosts pass it explicitly
/// instead of mutating `LUMO_HOME` at runtime — `std::env::set_var` in an
/// async command races every concurrent `getenv` on multi-threaded tokio).
pub fn ocr_model_catalog(home: &Path) -> Vec<OcrModelStatus> {
    ocr_model_presets()
        .iter()
        .copied()
        .map(|preset| status_for_preset(Some(home), preset))
        .collect()
}

pub fn is_local_ocr_model(model: &str) -> bool {
    let model = model.trim();
    model.starts_with(MODELSCOPE_PREFIX) || resolve_preset(model).is_some()
}

pub fn normalize_model_ref(model: &str) -> String {
    let model = model.trim();
    if let Some(preset) = resolve_preset(model) {
        preset.id.to_string()
    } else if model.contains('/') && !model.starts_with(MODELSCOPE_PREFIX) {
        format!("{MODELSCOPE_PREFIX}{model}")
    } else {
        model.to_string()
    }
}

pub fn resolve_preset(model: &str) -> Option<OcrModelPreset> {
    let trimmed = model.trim();
    let repo = trimmed.strip_prefix(MODELSCOPE_PREFIX).unwrap_or(trimmed);
    ocr_model_presets()
        .iter()
        .copied()
        .find(|preset| preset.id == trimmed || preset.repo == repo || preset.label == trimmed)
}

pub fn status_for_model(model: &str) -> Option<OcrModelStatus> {
    resolve_preset(model).map(|preset| status_for_preset(None, preset))
}

/// Download a preset model into the cache under `home` (see
/// [`ocr_model_catalog`] for why `home` is an explicit parameter).
pub async fn download_modelscope_model(
    home: &Path,
    model: &str,
) -> Result<OcrModelDownload, StepError> {
    let bin = std::env::var("LUMO_MODELSCOPE_BIN").unwrap_or_else(|_| "modelscope".into());
    download_with_binary(home, model, &bin).await
}

async fn download_with_binary(
    home: &Path,
    model: &str,
    bin: &str,
) -> Result<OcrModelDownload, StepError> {
    let preset = resolve_preset(model)
        .ok_or_else(|| StepError::msg(format!("unknown OCR model preset `{model}`")))?;
    let _guard = DOWNLOAD_LOCK.get_or_init(Default::default).lock().await;
    let dest = cache_dir_for_repo(Some(home), preset.repo);
    if model_dir_downloaded(&dest) {
        return Ok(OcrModelDownload {
            model: status_for_preset(Some(home), preset),
            stdout_tail: "Model files are already complete".into(),
            stderr_tail: String::new(),
        });
    }
    let parent = dest
        .parent()
        .ok_or_else(|| StepError::msg("invalid OCR cache path"))?;
    tokio::fs::create_dir_all(parent)
        .await
        .map_err(|e| StepError::msg(format!("create OCR model cache {}: {e}", dest.display())))?;
    let staging = parent.join(format!(
        ".{}-download-{}",
        preset.repo.replace('/', "__"),
        unique_suffix()
    ));
    tokio::fs::create_dir(&staging)
        .await
        .map_err(|e| StepError::msg(format!("create OCR staging directory: {e}")))?;
    let _staging_cleanup = StagingCleanup(staging.clone());

    let output = Command::new(bin)
        .arg("download")
        .arg("--model")
        .arg(preset.repo)
        .arg("--local_dir")
        .arg(&staging)
        .kill_on_drop(true)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output();
    let timeout_secs = std::env::var("LUMO_OCR_DOWNLOAD_TIMEOUT_SECS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(3600);
    let output = tokio::time::timeout(Duration::from_secs(timeout_secs), output)
        .await
        .map_err(|_| {
            StepError::msg(format!(
                "OCR download timed out after {timeout_secs}s; previous cache preserved"
            ))
        })?
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                StepError::msg(
                    "modelscope CLI not found. Install it with `pip install modelscope`, \
                     or set LUMO_MODELSCOPE_BIN to the executable path."
                        .to_string(),
                )
            } else {
                StepError::msg(format!("run modelscope download: {e}"))
            }
        })?;

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    if !output.status.success() {
        let _ = tokio::fs::remove_dir_all(&staging).await;
        return Err(StepError::msg(format!(
            "modelscope download `{}` failed with status {}; stderr: {}",
            preset.repo,
            output.status,
            tail(&stderr, 1800)
        )));
    }

    if let Err(reason) = validate_model_files(&staging) {
        let _ = tokio::fs::remove_dir_all(&staging).await;
        return Err(StepError::msg(format!(
            "OCR download is incomplete: {reason}; previous cache preserved"
        )));
    }
    write_download_marker(&staging, preset)?;
    publish_download(&staging, &dest)?;
    Ok(OcrModelDownload {
        model: status_for_preset(Some(home), preset),
        stdout_tail: tail(&stdout, 1800),
        stderr_tail: tail(&stderr, 1800),
    })
}

pub async fn run_modelscope_ocr(
    image: Bytes,
    media_type: &str,
    prompt: &str,
    model: &str,
) -> Result<(String, AiCallUsage), StepError> {
    run_modelscope_ocr_at(None, image, media_type, prompt, model).await
}

pub(crate) async fn run_modelscope_ocr_at(
    home: Option<&Path>,
    image: Bytes,
    media_type: &str,
    prompt: &str,
    model: &str,
) -> Result<(String, AiCallUsage), StepError> {
    let python = std::env::var("LUMO_OCR_PYTHON").unwrap_or_else(|_| "python3".into());
    run_modelscope_ocr_inner(home, &python, image, media_type, prompt, model).await
}

async fn run_modelscope_ocr_inner(
    home: Option<&Path>,
    python: &str,
    image: Bytes,
    media_type: &str,
    prompt: &str,
    model: &str,
) -> Result<(String, AiCallUsage), StepError> {
    let model_ref = normalize_model_ref(model);
    let preset = resolve_preset(&model_ref)
        .ok_or_else(|| StepError::msg(format!("unknown local OCR model `{model}`")))?;
    let status = status_for_preset(home, preset);
    if !status.downloaded {
        return Err(StepError::msg(format!(
            "OCR model `{}` is not downloaded. Download it from the Models page first.",
            preset.label
        )));
    }

    let t0 = Instant::now();
    let image_path = write_temp_image(&image, media_type).await?;
    let result = run_python_worker_with_python(
        python,
        preset,
        Path::new(&status.cache_dir),
        &image_path.0,
        prompt,
    )
    .await;
    drop(image_path);
    if let Ok(mut diagnostics) = DIAGNOSTICS.get_or_init(Default::default).lock() {
        let entry = diagnostics
            .entry(PathBuf::from(&status.cache_dir))
            .or_default();
        entry.fingerprint = validate_model_files(Path::new(&status.cache_dir)).unwrap_or(0);
        entry.inference_checked = true;
        entry.inference_error = result.as_ref().err().map(ToString::to_string);
        if result.is_ok() {
            entry.runtime_checked = true;
            entry.runtime_error = None;
        }
    }
    let text = result?;
    Ok((
        text,
        AiCallUsage {
            helper: "ocr_image".into(),
            provider: "local-ocr".into(),
            model: model_ref,
            input_tokens: 0,
            output_tokens: 0,
            latency_ms: t0.elapsed().as_millis() as i64,
            cost_usd_micro: 0,
        },
    ))
}

fn status_for_preset(home: Option<&Path>, preset: OcrModelPreset) -> OcrModelStatus {
    let cache_dir = cache_dir_for_repo(home, preset.repo);
    status_for_dir(preset, cache_dir)
}

fn status_for_dir(preset: OcrModelPreset, cache_dir: PathBuf) -> OcrModelStatus {
    let files = validate_model_files(&cache_dir);
    let diagnostics = DIAGNOSTICS
        .get_or_init(Default::default)
        .lock()
        .ok()
        .and_then(|values| values.get(&cache_dir).cloned())
        .filter(|entry| files.as_ref().copied().unwrap_or(0) == entry.fingerprint);
    let runtime_state = match diagnostics.as_ref() {
        Some(entry) if entry.runtime_checked && entry.runtime_error.is_none() => "ready",
        Some(entry) if entry.runtime_checked => "missing",
        _ => "unchecked",
    };
    let inference_state = match diagnostics.as_ref() {
        Some(entry) if entry.inference_checked && entry.inference_error.is_none() => "ready",
        Some(entry) if entry.inference_checked => "failed",
        _ => "unchecked",
    };
    OcrModelStatus {
        preset,
        downloaded: files.is_ok(),
        download_state: if files.is_ok() {
            "complete"
        } else if cache_dir.exists() {
            "partial"
        } else {
            "missing"
        },
        download_reason: files.err(),
        runtime_state,
        runtime_reason: diagnostics
            .as_ref()
            .and_then(|entry| entry.runtime_error.clone()),
        inference_state,
        inference_reason: diagnostics.and_then(|entry| entry.inference_error),
        cache_dir: cache_dir.display().to_string(),
    }
}

/// Cache-root precedence: `LUMO_OCR_MODELS_DIR` (dedicated override) → the
/// explicit `home` argument (P2-2) → `LUMO_HOME` env → `~/.lumorpa`. The env
/// fallbacks stay for call sites that have no home threaded through
/// ([`run_modelscope_ocr`] / [`status_for_model`]); they only read the
/// environment — the desktop no longer writes it at runtime.
fn cache_root(home: Option<&Path>) -> PathBuf {
    if let Ok(p) = std::env::var("LUMO_OCR_MODELS_DIR") {
        return PathBuf::from(p);
    }
    if let Some(home) = home {
        return home.join("models").join("ocr");
    }
    if let Ok(p) = std::env::var("LUMO_HOME") {
        return PathBuf::from(p).join("models").join("ocr");
    }
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".lumorpa")
        .join("models")
        .join("ocr")
}

fn cache_dir_for_repo(home: Option<&Path>, repo: &str) -> PathBuf {
    cache_root(home).join(repo.replace(['/', ':'], "__"))
}

fn model_dir_downloaded(path: &Path) -> bool {
    validate_model_files(path).is_ok()
}

/// Validate the files Transformers will read, including *all* indexed shards.
/// This is a structural completeness check; inference is a separate explicit
/// operation. Catalog reads never import Python or load model weights.
fn validate_model_files(root: &Path) -> Result<u64, String> {
    let mut fingerprint = std::collections::hash_map::DefaultHasher::new();
    let canonical_root = root
        .canonicalize()
        .map_err(|e| format!("model directory: {e}"))?;
    let mut check_file = |name: &str| -> Result<PathBuf, String> {
        let relative = Path::new(name);
        if relative
            .components()
            .any(|part| !matches!(part, Component::Normal(_)))
        {
            return Err(format!("unsafe model asset path: {name}"));
        }
        let path = root.join(relative);
        let canonical = path
            .canonicalize()
            .map_err(|_| format!("missing model asset: {name}"))?;
        if !canonical.starts_with(&canonical_root) {
            return Err(format!("model asset leaves its directory: {name}"));
        }
        let metadata = path.metadata().map_err(|e| format!("{name}: {e}"))?;
        if !metadata.is_file() || metadata.len() == 0 {
            return Err(format!("empty or invalid model asset: {name}"));
        }
        name.hash(&mut fingerprint);
        metadata.len().hash(&mut fingerprint);
        metadata.modified().ok().hash(&mut fingerprint);
        Ok(path)
    };
    for name in ["config.json", "tokenizer_config.json"] {
        read_json_object(&check_file(name)?)?;
    }
    let processor = ["preprocessor_config.json", "processor_config.json"]
        .into_iter()
        .find(|name| root.join(name).is_file())
        .ok_or("missing processor configuration")?;
    read_json_object(&check_file(processor)?)?;
    let tokenizer = [
        "tokenizer.json",
        "tokenizer.model",
        "spiece.model",
        "vocab.json",
    ]
    .into_iter()
    .find(|name| root.join(name).is_file())
    .ok_or("missing tokenizer vocabulary")?;
    let tokenizer_path = check_file(tokenizer)?;
    if tokenizer.ends_with(".json") {
        read_json_object(&tokenizer_path)?;
    }

    let indexes: Vec<_> = [
        "model.safetensors.index.json",
        "pytorch_model.bin.index.json",
    ]
    .into_iter()
    .filter(|name| root.join(name).exists())
    .collect();
    if indexes.is_empty() {
        let weights = ["model.safetensors", "pytorch_model.bin"]
            .into_iter()
            .find(|name| root.join(name).is_file())
            .ok_or("missing model weights or shard index")?;
        check_file(weights)?;
    } else {
        for name in indexes {
            let index = read_json_object(&check_file(name)?)?;
            let map = index
                .get("weight_map")
                .and_then(serde_json::Value::as_object)
                .filter(|map| !map.is_empty())
                .ok_or_else(|| format!("invalid weight_map in {name}"))?;
            let mut checked = HashSet::new();
            for shard in map.values() {
                let shard = shard
                    .as_str()
                    .filter(|value| !value.is_empty())
                    .ok_or("invalid weight shard name")?;
                if checked.insert(shard) {
                    check_file(shard)?;
                }
            }
        }
    }
    Ok(fingerprint.finish())
}

fn read_json_object(path: &Path) -> Result<serde_json::Value, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let value: serde_json::Value =
        serde_json::from_slice(&bytes).map_err(|e| format!("invalid {}: {e}", path.display()))?;
    if !value.is_object() {
        return Err(format!("{} must contain a JSON object", path.display()));
    }
    Ok(value)
}

fn publish_download(staging: &Path, dest: &Path) -> Result<(), StepError> {
    // Recheck at publication, so partial downloads can never replace the cache.
    validate_model_files(staging).map_err(StepError::msg)?;
    let backup = dest.with_extension(format!("previous-{}", unique_suffix()));
    let previous = dest.exists();
    if previous {
        std::fs::rename(dest, &backup)
            .map_err(|e| StepError::msg(format!("preserve previous OCR cache: {e}")))?;
    }
    if let Err(error) = std::fs::rename(staging, dest) {
        if previous {
            let _ = std::fs::rename(&backup, dest);
        }
        return Err(StepError::msg(format!("publish OCR download: {error}")));
    }
    if previous {
        let _ = std::fs::remove_dir_all(backup);
    }
    if let Ok(mut entries) = DIAGNOSTICS.get_or_init(Default::default).lock() {
        entries.remove(dest);
    }
    Ok(())
}

fn unique_suffix() -> String {
    format!(
        "{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    )
}

/// Explicit diagnostics. A plain catalog refresh never runs this check. The
/// runtime probe imports dependencies; inference additionally loads the model
/// and recognizes a tiny local fixture with every HF network fallback disabled.
pub async fn check_ocr_model(
    home: &Path,
    model: &str,
    inference: bool,
) -> Result<OcrModelStatus, StepError> {
    let preset = resolve_preset(model)
        .ok_or_else(|| StepError::msg(format!("unknown OCR model preset `{model}`")))?;
    let dir = cache_dir_for_repo(Some(home), preset.repo);
    let files = validate_model_files(&dir);
    let python = std::env::var("LUMO_OCR_PYTHON").unwrap_or_else(|_| "python3".into());
    let fingerprint = files.as_ref().copied().unwrap_or(0);
    let mut diagnostics = DIAGNOSTICS
        .get_or_init(Default::default)
        .lock()
        .ok()
        .and_then(|entries| entries.get(&dir).cloned())
        .filter(|entry| entry.fingerprint == fingerprint)
        .unwrap_or_default();
    diagnostics.fingerprint = fingerprint;
    diagnostics.runtime_checked = true;
    diagnostics.runtime_error = probe_runtime(&python, preset.engine)
        .await
        .err()
        .map(|error| error.to_string());
    if diagnostics.runtime_error.is_some() {
        diagnostics.inference_checked = false;
        diagnostics.inference_error = None;
    }
    if inference {
        diagnostics.inference_checked = true;
        diagnostics.inference_error = if let Err(reason) = files {
            Some(format!("model files incomplete: {reason}"))
        } else if diagnostics.runtime_error.is_some() {
            Some("Python runtime dependencies are unavailable; run the runtime check first".into())
        } else {
            use base64::Engine as _;
            // 64x64 RGB PNG. The check exercises decoding, model loading and
            // generation; an empty recognition is valid for this blank image.
            let bytes = base64::engine::general_purpose::STANDARD.decode("iVBORw0KGgoAAAANSUhEUgAAAEAAAABACAIAAAAlC+aJAAAAXklEQVR4nO3PMQ0AMAzAsPInvYLYYVWKESTzjhsd8KsBrQGtAa0BrQGtAa0BrQGtAa0BrQGtAa0BrQGtAa0BrQGtAa0BrQGtAa0BrQGtAa0BrQGtAa0BrQGtAa0BrQGtAa0BrQGtAa0BrQGtAa0BbQHKU9LC7/CP1AAAAABJRU5ErkJggg==")
                .map_err(|error| StepError::msg(format!("OCR diagnostic fixture: {error}")))?;
            let image_path = write_temp_image(&bytes, "image/png").await?;
            let result = run_python_worker(
                preset,
                &dir,
                &image_path.0,
                "Extract readable text. Return plain text only.",
            )
            .await;
            drop(image_path);
            result.err().map(|error| error.to_string())
        };
    }
    DIAGNOSTICS
        .get_or_init(Default::default)
        .lock()
        .map_err(|_| StepError::msg("OCR diagnostics unavailable"))?
        .insert(dir.clone(), diagnostics);
    Ok(status_for_dir(preset, dir))
}

async fn probe_runtime(python: &str, engine: &str) -> Result<(), StepError> {
    let output = Command::new(python)
        .arg("-c")
        .arg(PYTHON_RUNTIME_PROBE)
        .arg(engine)
        .env("HF_HUB_OFFLINE", "1")
        .env("TRANSFORMERS_OFFLINE", "1")
        .kill_on_drop(true)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output();
    let output = tokio::time::timeout(Duration::from_secs(45), output)
        .await
        .map_err(|_| StepError::msg("OCR Python dependency check timed out after 45s"))?
        .map_err(|error| StepError::msg(format!("OCR Python `{python}` unavailable: {error}")))?;
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).map_err(|_| {
        StepError::msg(format!(
            "OCR runtime check failed: {}",
            tail(&String::from_utf8_lossy(&output.stderr), 1800)
        ))
    })?;
    if !output.status.success()
        || report.get("ok").and_then(serde_json::Value::as_bool) != Some(true)
    {
        return Err(StepError::msg(format!(
            "OCR runtime dependencies unavailable: {}",
            report
                .get("error")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("invalid runtime probe result")
        )));
    }
    Ok(())
}

const PYTHON_RUNTIME_PROBE: &str = r#"
import contextlib, importlib, json, sys
try:
    with contextlib.redirect_stdout(sys.stderr):
        for name in ("torch", "transformers", "PIL", "accelerate"):
            importlib.import_module(name)
        if sys.argv[1] in ("glm-ocr", "paddleocr-vl"):
            importlib.import_module("torchvision")
        import transformers
        required = ("AutoTokenizer", "AutoModel") if sys.argv[1] == "deepseek-ocr" else ("AutoProcessor", "GlmOcrForConditionalGeneration" if sys.argv[1] == "glm-ocr" else "AutoModelForImageTextToText")
        for name in required:
            getattr(transformers, name)
    print(json.dumps({"ok": True}))
except Exception as error:
    print(json.dumps({"ok": False, "error": str(error)}))
    sys.exit(1)
"#;

fn write_download_marker(path: &Path, preset: OcrModelPreset) -> Result<(), StepError> {
    let marker = serde_json::json!({
        "id": preset.id,
        "repo": preset.repo,
        "engine": preset.engine,
        "downloaded_at": chrono_like_timestamp(),
    });
    std::fs::write(path.join(".lumorpa-ocr-model.json"), marker.to_string())
        .map_err(|e| StepError::msg(format!("write OCR model marker: {e}")))
}

async fn write_temp_image(image: &[u8], media_type: &str) -> Result<TemporaryImage, StepError> {
    let ext = match media_type {
        "image/jpeg" | "image/jpg" => "jpg",
        "image/bmp" => "bmp",
        "image/webp" => "webp",
        _ => "png",
    };
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let path =
        std::env::temp_dir().join(format!("lumorpa-ocr-{}-{nanos}.{ext}", std::process::id()));
    let mut options = tokio::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    options.mode(0o600);
    let mut file = options
        .open(&path)
        .await
        .map_err(|error| StepError::msg(format!("create OCR temp image: {error}")))?;
    let image_path = TemporaryImage(path);
    use tokio::io::AsyncWriteExt;
    file.write_all(image)
        .await
        .map_err(|error| StepError::msg(format!("write OCR temp image: {error}")))?;
    Ok(image_path)
}

async fn run_python_worker(
    preset: OcrModelPreset,
    model_dir: &Path,
    image_path: &Path,
    prompt: &str,
) -> Result<String, StepError> {
    let python = std::env::var("LUMO_OCR_PYTHON").unwrap_or_else(|_| "python3".into());
    run_python_worker_with_python(&python, preset, model_dir, image_path, prompt).await
}

async fn run_python_worker_with_python(
    python: &str,
    preset: OcrModelPreset,
    model_dir: &Path,
    image_path: &Path,
    prompt: &str,
) -> Result<String, StepError> {
    let timeout_secs = std::env::var("LUMO_OCR_TIMEOUT_SECS")
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(DEFAULT_TIMEOUT_SECS);
    let output = python_worker_command(python, preset, model_dir, image_path, prompt).output();

    let output = tokio::time::timeout(Duration::from_secs(timeout_secs), output)
        .await
        .map_err(|_| {
            StepError::msg(format!(
                "local OCR model `{}` timed out after {timeout_secs}s",
                preset.label
            ))
        })?
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                StepError::msg(
                    "python3 not found. Install Python with transformers/modelscope runtime deps, \
                     or set LUMO_OCR_PYTHON to the interpreter path."
                        .to_string(),
                )
            } else {
                StepError::msg(format!("run local OCR worker: {e}"))
            }
        })?;

    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    if !output.status.success() {
        return Err(StepError::msg(format!(
            "local OCR worker for `{}` failed with status {}; stderr: {}",
            preset.label,
            output.status,
            tail(&stderr, 1800)
        )));
    }
    let result: serde_json::Value = serde_json::from_slice(&output.stdout).map_err(|error| {
        StepError::msg(format!(
            "invalid local OCR worker response: {error}; stderr: {}",
            tail(&stderr, 1800)
        ))
    })?;
    result
        .get("text")
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| StepError::msg("local OCR worker response has no text field"))
}

fn python_worker_command(
    python: &str,
    preset: OcrModelPreset,
    model_dir: &Path,
    image_path: &Path,
    prompt: &str,
) -> Command {
    let mut command = Command::new(python);
    command
        .arg("-c")
        .arg(PYTHON_WORKER)
        .arg(preset.engine)
        .arg(model_dir)
        .arg(image_path)
        .arg(if prompt.trim().is_empty() {
            "Extract all readable text from this image. Preserve line breaks. Return plain text only."
        } else {
            prompt
        })
        .env("HF_HUB_OFFLINE", "1")
        .env("TRANSFORMERS_OFFLINE", "1")
        .env("HF_HUB_DISABLE_TELEMETRY", "1")
        .kill_on_drop(true)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    command
}

fn tail(s: &str, max_chars: usize) -> String {
    let len = s.chars().count();
    if len <= max_chars {
        return s.to_string();
    }
    s.chars().skip(len - max_chars).collect()
}

fn chrono_like_timestamp() -> String {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs().to_string())
        .unwrap_or_else(|_| "0".into())
}

const PYTHON_WORKER: &str = r#"
import contextlib
import inspect
import json
import os
import sys
import tempfile

engine, model_dir, image_path, prompt = sys.argv[1:5]

def as_text(value):
    if value is None:
        raise RuntimeError("OCR backend returned no recognition result")
    if isinstance(value, (list, tuple)):
        return "\n".join(str(x) for x in value if x is not None)
    return str(value)

def run_chat_template(model_cls_name=None):
    import torch
    from transformers import AutoProcessor
    if model_cls_name == "glm":
        from transformers import GlmOcrForConditionalGeneration
        model = GlmOcrForConditionalGeneration.from_pretrained(
            model_dir, trust_remote_code=True, local_files_only=True, torch_dtype="auto", device_map="auto"
        )
    else:
        from transformers import AutoModelForImageTextToText
        model = AutoModelForImageTextToText.from_pretrained(
            model_dir, trust_remote_code=True, local_files_only=True, torch_dtype="auto", device_map="auto"
        )
    processor = AutoProcessor.from_pretrained(model_dir, trust_remote_code=True, local_files_only=True)
    messages = [{
        "role": "user",
        "content": [
            {"type": "image", "url": image_path},
            {"type": "text", "text": prompt},
        ],
    }]
    inputs = processor.apply_chat_template(
        messages,
        tokenize=True,
        add_generation_prompt=True,
        return_dict=True,
        return_tensors="pt",
    )
    inputs = inputs.to(model.device)
    with torch.no_grad():
        generated = model.generate(**inputs, max_new_tokens=int(os.environ.get("LUMO_OCR_MAX_TOKENS", "2048")))
    generated = generated[:, inputs["input_ids"].shape[-1]:]
    text = processor.batch_decode(generated, skip_special_tokens=True)[0]
    return text.strip()

def run_deepseek():
    from transformers import AutoModel, AutoTokenizer
    import torch
    tokenizer = AutoTokenizer.from_pretrained(model_dir, trust_remote_code=True, local_files_only=True)
    model = AutoModel.from_pretrained(
        model_dir, trust_remote_code=True, local_files_only=True, torch_dtype="auto", device_map="auto"
    )
    if hasattr(model, "eval"):
        model = model.eval()
    if not hasattr(model, "infer"):
        raise RuntimeError("DeepSeek OCR model object has no infer() method")
    kwargs = {"eval_mode": True} if "eval_mode" in inspect.signature(model.infer).parameters else {}
    with tempfile.TemporaryDirectory(prefix="lumorpa-ocr-output-") as output_path:
        result = model.infer(
            tokenizer,
            prompt=prompt,
            image_file=image_path,
            output_path=output_path,
            base_size=int(os.environ.get("LUMO_OCR_BASE_SIZE", "1024")),
            image_size=int(os.environ.get("LUMO_OCR_IMAGE_SIZE", "640")),
            crop_mode=os.environ.get("LUMO_OCR_CROP_MODE", "1") != "0",
            save_results=False,
            **kwargs,
        )
    return as_text(result)

# Model libraries often print progress. Keep stdout a machine-readable result
# and leave diagnostics on stderr so library logs cannot become document text.
with contextlib.redirect_stdout(sys.stderr):
    if engine == "glm-ocr":
        text = run_chat_template("glm")
    elif engine == "paddleocr-vl":
        text = run_chat_template(None)
    elif engine == "deepseek-ocr":
        text = run_deepseek()
    else:
        raise RuntimeError(f"unsupported OCR engine: {engine}")
print(json.dumps({"text": text}, ensure_ascii=False))
"#;

#[cfg(test)]
mod tests {
    use super::*;

    fn complete_model(dir: &Path) {
        for name in [
            "config.json",
            "tokenizer_config.json",
            "preprocessor_config.json",
            "tokenizer.json",
        ] {
            std::fs::write(dir.join(name), b"{}").unwrap();
        }
        std::fs::write(dir.join("model.safetensors"), b"weights").unwrap();
    }

    #[test]
    fn a_download_marker_or_partial_directory_does_not_mean_complete() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(".lumorpa-ocr-model.json"), b"{}").unwrap();
        assert!(!model_dir_downloaded(dir.path()));
        std::fs::write(dir.path().join("config.json"), b"{}").unwrap();
        assert!(!model_dir_downloaded(dir.path()));
        complete_model(dir.path());
        assert!(model_dir_downloaded(dir.path()));
        std::fs::write(dir.path().join("model.safetensors"), b"").unwrap();
        assert!(!model_dir_downloaded(dir.path()));
    }

    #[test]
    fn every_indexed_weight_shard_must_exist_and_be_nonempty() {
        let dir = tempfile::tempdir().unwrap();
        complete_model(dir.path());
        std::fs::remove_file(dir.path().join("model.safetensors")).unwrap();
        std::fs::write(
            dir.path().join("model.safetensors.index.json"),
            br#"{"weight_map":{"a":"model-1.safetensors","b":"model-2.safetensors"}}"#,
        )
        .unwrap();
        std::fs::write(dir.path().join("model-1.safetensors"), b"weights").unwrap();
        assert!(!model_dir_downloaded(dir.path()));
        std::fs::write(dir.path().join("model-2.safetensors"), b"weights").unwrap();
        assert!(model_dir_downloaded(dir.path()));
        std::fs::write(
            dir.path().join("model.safetensors.index.json"),
            br#"{"weight_map":{"a":"../outside.safetensors"}}"#,
        )
        .unwrap();
        assert!(!model_dir_downloaded(dir.path()));
    }

    #[test]
    fn invalid_configs_and_missing_processor_or_tokenizer_are_incomplete() {
        let dir = tempfile::tempdir().unwrap();
        complete_model(dir.path());
        std::fs::write(dir.path().join("config.json"), b"{broken").unwrap();
        assert!(!model_dir_downloaded(dir.path()));
        complete_model(dir.path());
        std::fs::remove_file(dir.path().join("preprocessor_config.json")).unwrap();
        assert!(!model_dir_downloaded(dir.path()));
        complete_model(dir.path());
        std::fs::remove_file(dir.path().join("tokenizer.json")).unwrap();
        assert!(!model_dir_downloaded(dir.path()));
    }

    #[test]
    fn catalog_separates_files_runtime_and_inference_and_invalidates_stale_probes() {
        let dir = tempfile::tempdir().unwrap();
        complete_model(dir.path());
        let preset = ocr_model_presets()[0];
        let status = status_for_dir(preset, dir.path().into());
        assert!(status.downloaded);
        assert_eq!(status.download_state, "complete");
        assert_eq!(status.runtime_state, "unchecked");
        assert_eq!(status.inference_state, "unchecked");
        DIAGNOSTICS
            .get_or_init(Default::default)
            .lock()
            .unwrap()
            .insert(
                dir.path().into(),
                ModelDiagnostics {
                    fingerprint: validate_model_files(dir.path()).unwrap(),
                    runtime_checked: true,
                    runtime_error: Some("missing torch".into()),
                    inference_checked: true,
                    inference_error: Some("runtime unavailable".into()),
                },
            );
        let status = status_for_dir(preset, dir.path().into());
        assert_eq!(status.runtime_state, "missing");
        assert_eq!(status.runtime_reason.as_deref(), Some("missing torch"));
        assert_eq!(status.inference_state, "failed");
        std::fs::write(
            dir.path().join("model.safetensors"),
            b"different model weights",
        )
        .unwrap();
        assert_eq!(
            status_for_dir(preset, dir.path().into()).inference_state,
            "unchecked"
        );
    }

    #[test]
    fn publishing_partial_download_preserves_the_previous_cache() {
        let parent = tempfile::tempdir().unwrap();
        let dest = parent.path().join("current");
        let staging = parent.path().join("staging");
        std::fs::create_dir(&dest).unwrap();
        std::fs::create_dir(&staging).unwrap();
        complete_model(&dest);
        std::fs::write(dest.join("preserved"), b"previous").unwrap();
        assert!(publish_download(&staging, &dest).is_err());
        assert_eq!(std::fs::read(dest.join("preserved")).unwrap(), b"previous");
        complete_model(&staging);
        publish_download(&staging, &dest).unwrap();
        assert!(model_dir_downloaded(&dest));
        assert!(!dest.join("preserved").exists());
    }

    #[cfg(unix)]
    fn executable(dir: &Path, name: &str, source: &str) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let path = dir.join(name);
        std::fs::write(&path, source).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
        path
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn successful_cli_exit_without_complete_files_is_rejected() {
        if std::env::var_os("LUMO_OCR_MODELS_DIR").is_some() {
            return;
        }
        let home = tempfile::tempdir().unwrap();
        let bin = executable(
            home.path(),
            "download",
            "#!/bin/sh\nprintf '{}' > \"$5/config.json\"\n",
        );
        let preset = ocr_model_presets()[0];
        let dest = cache_dir_for_repo(Some(home.path()), preset.repo);
        std::fs::create_dir_all(&dest).unwrap();
        std::fs::write(dest.join("previous"), b"preserved").unwrap();
        let error = download_with_binary(home.path(), preset.id, bin.to_str().unwrap())
            .await
            .unwrap_err();
        assert!(error.to_string().contains("incomplete"), "{error}");
        assert_eq!(std::fs::read(dest.join("previous")).unwrap(), b"preserved");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn downloaded_catalog_and_inference_share_the_explicit_desktop_home() {
        if std::env::var_os("LUMO_OCR_MODELS_DIR").is_some() {
            return;
        }
        let home = tempfile::tempdir().unwrap();
        let downloader = executable(
            home.path(),
            "download",
            r#"#!/bin/sh
for name in config.json tokenizer_config.json preprocessor_config.json tokenizer.json; do
  printf '{}' > "$5/$name"
done
printf weights > "$5/model.safetensors"
"#,
        );
        let python = executable(
            home.path(),
            "python",
            r#"#!/bin/sh
printf '%s' "$4" > "$0.model-dir"
printf '%s' '{"text":"same desktop cache"}'
"#,
        );
        let preset = ocr_model_presets()[0];
        let downloaded = download_with_binary(home.path(), preset.id, downloader.to_str().unwrap())
            .await
            .unwrap();
        assert!(downloaded.model.downloaded);
        let catalog = ocr_model_catalog(home.path());
        assert_eq!(catalog[0].cache_dir, downloaded.model.cache_dir);
        let (text, usage) = run_modelscope_ocr_inner(
            Some(home.path()),
            python.to_str().unwrap(),
            Bytes::from_static(b"image"),
            "image/png",
            "read",
            preset.id,
        )
        .await
        .unwrap();
        assert_eq!(text, "same desktop cache");
        assert_eq!(usage.provider, "local-ocr");
        assert_eq!(
            std::fs::read_to_string(python.with_file_name("python.model-dir")).unwrap(),
            downloaded.model.cache_dir
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn runtime_check_reports_missing_python_and_dependency_failures() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("not-a-python");
        assert!(probe_runtime(missing.to_str().unwrap(), "glm-ocr")
            .await
            .unwrap_err()
            .to_string()
            .contains("unavailable"));
        let bin = executable(
            dir.path(),
            "python",
            "#!/bin/sh\nprintf '%s' '{\"ok\":false,\"error\":\"No module named torch\"}'\nexit 1\n",
        );
        let error = probe_runtime(bin.to_str().unwrap(), "glm-ocr")
            .await
            .unwrap_err();
        assert!(
            error.to_string().contains("No module named torch"),
            "{error}"
        );
    }

    async fn runtime_probe_with_fake_modules(
        dir: &Path,
        engine: &str,
    ) -> (bool, serde_json::Value) {
        let output = Command::new("python3")
            // Exclude installed site packages so a developer's torchvision
            // cannot hide a missing dependency in this controlled fixture.
            .arg("-S")
            .arg("-c")
            .arg(PYTHON_RUNTIME_PROBE)
            .arg(engine)
            .env("PYTHONPATH", dir)
            .env("PYTHONDONTWRITEBYTECODE", "1")
            .current_dir(dir)
            .output()
            .await
            .expect("Python 3 is required for the OCR runtime probe contract test");
        let report = serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
            panic!(
                "invalid runtime report: {error}; stderr: {}",
                String::from_utf8_lossy(&output.stderr)
            )
        });
        (output.status.success(), report)
    }

    fn fake_runtime_modules(dir: &Path) {
        for name in ["torch", "PIL", "accelerate"] {
            std::fs::write(dir.join(format!("{name}.py")), "").unwrap();
        }
        std::fs::write(
            dir.join("transformers.py"),
            "AutoTokenizer = AutoModel = AutoProcessor = GlmOcrForConditionalGeneration = AutoModelForImageTextToText = object\n",
        )
        .unwrap();
    }

    #[tokio::test]
    async fn multimodal_runtime_probe_rejects_missing_or_broken_torchvision() {
        let dir = tempfile::tempdir().unwrap();
        fake_runtime_modules(dir.path());
        for engine in ["glm-ocr", "paddleocr-vl"] {
            let (success, report) = runtime_probe_with_fake_modules(dir.path(), engine).await;
            assert!(!success, "{engine} must reject missing torchvision");
            assert_eq!(report["ok"], false);
            assert!(
                report["error"].as_str().unwrap().contains("torchvision"),
                "{engine}: {report}"
            );
        }
        std::fs::write(
            dir.path().join("torchvision.py"),
            "raise RuntimeError('torchvision compiled operators unavailable')\n",
        )
        .unwrap();
        for engine in ["glm-ocr", "paddleocr-vl"] {
            let (success, report) = runtime_probe_with_fake_modules(dir.path(), engine).await;
            assert!(!success, "{engine} must reject a broken torchvision import");
            assert_eq!(report["ok"], false);
            assert_eq!(
                report["error"],
                "torchvision compiled operators unavailable"
            );
        }
        std::fs::write(dir.path().join("torchvision.py"), "").unwrap();
        for engine in ["glm-ocr", "paddleocr-vl"] {
            let (success, report) = runtime_probe_with_fake_modules(dir.path(), engine).await;
            assert!(success, "{engine}: {report}");
            assert_eq!(report["ok"], true);
        }
    }

    #[tokio::test]
    async fn deepseek_runtime_probe_does_not_require_unrelated_processor_dependencies() {
        let dir = tempfile::tempdir().unwrap();
        fake_runtime_modules(dir.path());
        std::fs::write(
            dir.path().join("torchvision.py"),
            "raise RuntimeError('this engine must not import torchvision')\n",
        )
        .unwrap();
        let (success, report) = runtime_probe_with_fake_modules(dir.path(), "deepseek-ocr").await;
        assert!(success, "{report}");
        assert_eq!(report["ok"], true);
    }

    #[tokio::test]
    async fn python_worker_uses_only_local_files_and_separates_library_logs_from_text() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("torch.py"),
            "class no_grad:\n def __enter__(self): return self\n def __exit__(self, *args): pass\n",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("transformers.py"),
            r#"
import os
class AutoTokenizer:
    @classmethod
    def from_pretrained(cls, path, **kwargs):
        assert kwargs.get("local_files_only") is True
        assert os.environ.get("HF_HUB_OFFLINE") == "1"
        assert os.environ.get("TRANSFORMERS_OFFLINE") == "1"
        print("library progress log")
        return cls()
class AutoModel(AutoTokenizer):
    device = "cpu"
    def eval(self): return self
    def infer(self, tokenizer, **kwargs):
        return "本地识别结果"
    def generate(self, **kwargs): return Tensor()
class Tensor:
    shape = (1, 2)
    def __getitem__(self, key): return self
class Inputs(dict):
    def __init__(self): self["input_ids"] = Tensor()
    def to(self, device): return self
class AutoProcessor(AutoTokenizer):
    def apply_chat_template(self, messages, **kwargs): return Inputs()
    def batch_decode(self, generated, **kwargs): return ["本地识别结果"]
AutoModelForImageTextToText = AutoModel
GlmOcrForConditionalGeneration = AutoModel
"#,
        )
        .unwrap();
        for preset in ocr_model_presets() {
            let output = python_worker_command(
                "python3",
                *preset,
                dir.path(),
                &dir.path().join("input.png"),
                "read text",
            )
            .env("PYTHONPATH", dir.path())
            .output()
            .await
            .expect("Python 3 is required for the OCR worker contract test");
            assert!(
                output.status.success(),
                "{}: {}",
                preset.label,
                String::from_utf8_lossy(&output.stderr)
            );
            let result: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
            assert_eq!(result["text"], "本地识别结果");
            assert!(String::from_utf8_lossy(&output.stderr).contains("library progress log"));
        }
    }

    #[test]
    fn resolves_screenshot_presets() {
        assert!(resolve_preset("PaddlePaddle/PaddleOCR-VL-1.6").is_some());
        assert!(resolve_preset("modelscope/ZhipuAI/GLM-OCR").is_some());
        assert!(resolve_preset("deepseek-ai/DeepSeek-OCR-2").is_some());
    }

    #[test]
    fn local_ocr_detector_accepts_modelscope_refs() {
        assert!(is_local_ocr_model("modelscope/ZhipuAI/GLM-OCR"));
        assert!(is_local_ocr_model("ZhipuAI/GLM-OCR"));
        assert!(!is_local_ocr_model("gpt-4o"));
    }

    /// P2-2: the catalog roots every cache dir under the *explicit* home,
    /// so hosts never need to mutate `LUMO_HOME` at runtime.
    #[test]
    fn catalog_roots_cache_dirs_under_explicit_home() {
        if std::env::var_os("LUMO_OCR_MODELS_DIR").is_some() {
            // The dedicated override wins over any home by design; nothing
            // to assert about home-rooting in that environment.
            return;
        }
        let home = std::env::temp_dir().join("lumo-ai-ocr-explicit-home");
        let catalog = ocr_model_catalog(&home);
        assert!(!catalog.is_empty());
        for status in catalog {
            assert!(
                Path::new(&status.cache_dir).starts_with(home.join("models").join("ocr")),
                "cache dir `{}` must live under the explicit home",
                status.cache_dir
            );
        }
    }
}
