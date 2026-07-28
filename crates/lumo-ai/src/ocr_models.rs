//! Local OCR model catalog, ModelScope download helpers, and lightweight
//! external-worker execution for downloaded OCR/VLM models.

use bytes::Bytes;
use lumo_core::ai_hook::AiCallUsage;
use lumo_core::error::StepError;
use serde::Serialize;
use std::{
    path::{Path, PathBuf},
    process::Stdio,
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
    let preset = resolve_preset(model)
        .ok_or_else(|| StepError::msg(format!("unknown OCR model preset `{model}`")))?;
    let dest = cache_dir_for_repo(Some(home), preset.repo);
    tokio::fs::create_dir_all(&dest)
        .await
        .map_err(|e| StepError::msg(format!("create OCR model cache {}: {e}", dest.display())))?;

    let bin = std::env::var("LUMO_MODELSCOPE_BIN").unwrap_or_else(|_| "modelscope".into());
    let output = Command::new(&bin)
        .arg("download")
        .arg("--model")
        .arg(preset.repo)
        .arg("--local_dir")
        .arg(&dest)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await
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
        return Err(StepError::msg(format!(
            "modelscope download `{}` failed with status {}; stderr: {}",
            preset.repo,
            output.status,
            tail(&stderr, 1800)
        )));
    }

    write_download_marker(&dest, preset)?;
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
    let model_ref = normalize_model_ref(model);
    let preset = resolve_preset(&model_ref)
        .ok_or_else(|| StepError::msg(format!("unknown local OCR model `{model}`")))?;
    // No explicit home here: this runs deep inside the flow-run AI helpers
    // where no data dir is threaded. `cache_root(None)` only *reads* the
    // environment, which is race-free now that hosts no longer write it.
    let status = status_for_preset(None, preset);
    if !status.downloaded {
        return Err(StepError::msg(format!(
            "OCR model `{}` is not downloaded. Download it from the Models page first.",
            preset.label
        )));
    }

    let t0 = Instant::now();
    let image_path = write_temp_image(&image, media_type).await?;
    let result = run_python_worker(preset, Path::new(&status.cache_dir), &image_path, prompt).await;
    let _ = tokio::fs::remove_file(&image_path).await;
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
    OcrModelStatus {
        preset,
        downloaded: model_dir_downloaded(&cache_dir),
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
    if path.join(".lumorpa-ocr-model.json").exists() {
        return true;
    }
    std::fs::read_dir(path)
        .ok()
        .and_then(|mut entries| entries.next())
        .transpose()
        .ok()
        .flatten()
        .is_some()
}

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

async fn write_temp_image(image: &[u8], media_type: &str) -> Result<PathBuf, StepError> {
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
    tokio::fs::write(&path, image)
        .await
        .map_err(|e| StepError::msg(format!("write OCR temp image {}: {e}", path.display())))?;
    Ok(path)
}

async fn run_python_worker(
    preset: OcrModelPreset,
    model_dir: &Path,
    image_path: &Path,
    prompt: &str,
) -> Result<String, StepError> {
    let python = std::env::var("LUMO_OCR_PYTHON").unwrap_or_else(|_| "python3".into());
    let timeout_secs = std::env::var("LUMO_OCR_TIMEOUT_SECS")
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(DEFAULT_TIMEOUT_SECS);
    let output = Command::new(&python)
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
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output();

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

    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    if !output.status.success() {
        return Err(StepError::msg(format!(
            "local OCR worker for `{}` failed with status {}; stderr: {}",
            preset.label,
            output.status,
            tail(&stderr, 1800)
        )));
    }
    Ok(stdout)
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
import os
import sys

engine, model_dir, image_path, prompt = sys.argv[1:5]

def print_text(value):
    if value is None:
        return
    if isinstance(value, (list, tuple)):
        print("\n".join(str(x) for x in value if x is not None))
    else:
        print(str(value))

def run_chat_template(model_cls_name=None):
    import torch
    from transformers import AutoProcessor
    if model_cls_name == "glm":
        from transformers import GlmOcrForConditionalGeneration
        model = GlmOcrForConditionalGeneration.from_pretrained(
            model_dir, trust_remote_code=True, torch_dtype="auto", device_map="auto"
        )
    else:
        from transformers import AutoModelForImageTextToText
        model = AutoModelForImageTextToText.from_pretrained(
            model_dir, trust_remote_code=True, torch_dtype="auto", device_map="auto"
        )
    processor = AutoProcessor.from_pretrained(model_dir, trust_remote_code=True)
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
    print(text.strip())

def run_deepseek():
    from transformers import AutoModel, AutoTokenizer
    import torch
    tokenizer = AutoTokenizer.from_pretrained(model_dir, trust_remote_code=True)
    model = AutoModel.from_pretrained(
        model_dir, trust_remote_code=True, torch_dtype="auto", device_map="auto"
    )
    if hasattr(model, "eval"):
        model = model.eval()
    if not hasattr(model, "infer"):
        raise RuntimeError("DeepSeek OCR model object has no infer() method")
    result = model.infer(
        tokenizer,
        prompt=prompt,
        image_file=image_path,
        output_path=os.environ.get("LUMO_OCR_OUTPUT_DIR", os.getcwd()),
        base_size=int(os.environ.get("LUMO_OCR_BASE_SIZE", "1024")),
        image_size=int(os.environ.get("LUMO_OCR_IMAGE_SIZE", "640")),
        crop_mode=os.environ.get("LUMO_OCR_CROP_MODE", "1") != "0",
        save_results=False,
    )
    print_text(result)

if engine == "glm-ocr":
    run_chat_template("glm")
elif engine == "paddleocr-vl":
    run_chat_template(None)
elif engine == "deepseek-ocr":
    run_deepseek()
else:
    raise RuntimeError(f"unsupported OCR engine: {engine}")
"#;

#[cfg(test)]
mod tests {
    use super::*;

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
