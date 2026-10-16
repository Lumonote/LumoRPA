//! 本地小 OCR 模型引擎(可选特性 `ocr-native`)。
//!
//! **复刻 `lumo-voice` 的 `sherpa-native` 范式**:用 `libloading` 在**运行时**
//! 动态加载 ONNX Runtime,而不是编译期链接 C 库。这样默认构建仍是纯 Rust、可
//! 交叉编译,信创打包不受影响;缺库缺模型时优雅降级而不是编译失败。
//!
//! 模型:PP-OCR 系列(Apache-2.0)三件套 —— 检测 DB + 方向分类 + 识别 CRNN。
//! 模型不入 git,按清单下载并校验 SHA256(见 [`ModelManifest`]),校验失败拒绝
//! 加载 —— 模型文件是会被执行的资产,来源必须可验证。
//!
//! ## 当前状态(重要)
//!
//! 本模块已实现:可用性探测、模型清单与校验、降级路径、与路由层的对接。
//! **张量推理部分尚未接通** —— 缺少 ONNX Runtime 动态库与模型资产,无法在本机
//! 验证,因此 [`recognize`] 目前返回"未就绪"错误而非静默返回空结果。接通推理
//! 需要:`ort` 或直接 FFI 调用 + DB 后处理 + CRNN 解码,属于独立的一批工作。

use super::{OcrOptions, OcrRequest};
use lumo_core::error::StepError;
use lumo_doc::ocr::OcrPageResult;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// 模型清单 —— 与 `lumo-voice` 的语音模型安装器同构:每个资产带 SHA256,
/// 校验不过即拒绝加载。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelAsset {
    pub name: String,
    pub sha256: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelManifest {
    pub version: String,
    /// 检测 / 方向 / 识别三个模型文件。
    pub assets: Vec<ModelAsset>,
    /// 字典文件(识别结果的字符表)。
    pub dict: ModelAsset,
}

/// 模型根目录:`$LUMO_HOME/ocr-models`。这是宿主管理的路径,不是 flow 声明的
/// 用户路径,所以不过 `fs.read` 门禁 —— 与语音模型一致。
pub fn model_root() -> PathBuf {
    std::env::var_os("LUMO_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            std::env::var_os("HOME")
                .map(|home| PathBuf::from(home).join(".lumo"))
                .unwrap_or_else(|| PathBuf::from(".lumo"))
        })
        .join("ocr-models")
}

/// ONNX Runtime 动态库名(按平台)。
fn runtime_library_name() -> &'static str {
    #[cfg(target_os = "macos")]
    {
        "libonnxruntime.dylib"
    }
    #[cfg(target_os = "windows")]
    {
        "onnxruntime.dll"
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        "libonnxruntime.so"
    }
}

/// 本地 OCR 是否可用。**探测而不加载** —— 路由决策需要它,但不该有副作用。
///
/// 三个条件缺一不可:编译时开了 `ocr-native`、运行时库在位、模型清单校验通过。
pub fn is_available() -> bool {
    if !cfg!(feature = "ocr-native") {
        return false;
    }
    let root = model_root();
    runtime_path(&root).is_some() && verify_models(&root).is_ok()
}

/// 找运行时库:优先 `LUMO_ONNXRUNTIME_LIB` 显式指定,其次模型目录内。
fn runtime_path(root: &Path) -> Option<PathBuf> {
    if let Some(explicit) = std::env::var_os("LUMO_ONNXRUNTIME_LIB") {
        let path = PathBuf::from(explicit);
        return path.exists().then_some(path);
    }
    let candidate = root.join(runtime_library_name());
    candidate.exists().then_some(candidate)
}

/// 逐资产校验 SHA256。模型是会被加载执行的资产,来源必须可验证 —— 校验不过
/// 宁可不可用,也不加载来路不明的权重。
pub fn verify_models(root: &Path) -> Result<ModelManifest, String> {
    let manifest_path = root.join("manifest.json");
    let raw = std::fs::read(&manifest_path)
        .map_err(|e| format!("read {}: {e}", manifest_path.display()))?;
    let manifest: ModelManifest =
        serde_json::from_slice(&raw).map_err(|e| format!("invalid manifest: {e}"))?;

    for asset in manifest
        .assets
        .iter()
        .chain(std::iter::once(&manifest.dict))
    {
        let path = root.join(&asset.name);
        let bytes = std::fs::read(&path).map_err(|e| format!("read {}: {e}", path.display()))?;
        let actual = sha256_hex(&bytes);
        if actual != asset.sha256 {
            return Err(format!(
                "checksum mismatch for {}: expected {}, got {actual}",
                asset.name, asset.sha256
            ));
        }
    }
    Ok(manifest)
}

fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("{:x}", hasher.finalize())
}

/// 本地识别。当前未接通推理 —— 见模块头注。
pub fn recognize(
    _options: &OcrOptions,
    _request: &OcrRequest<'_>,
) -> Result<OcrPageResult, StepError> {
    // 不静默返回空结果:说清楚缺什么、怎么补
    Err(StepError::msg(format!(
        "本地 OCR 推理尚未接通(模型目录 {})。当前可用的替代路径:配置 \
         `ocr.engine: remote` 指向自建 OCR 服务,或 `ocr.engine: vlm` 走视觉模型 \
         —— 两者都需要显式 `ocr.allow_cloud: true`。",
        model_root().display()
    )))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn write(dir: &Path, name: &str, content: &[u8]) -> String {
        let mut file = std::fs::File::create(dir.join(name)).unwrap();
        file.write_all(content).unwrap();
        sha256_hex(content)
    }

    fn manifest_with(dir: &Path, det_hash: &str, dict_hash: &str) {
        let manifest = ModelManifest {
            version: "pp-ocr-v4".into(),
            assets: vec![ModelAsset {
                name: "det.onnx".into(),
                sha256: det_hash.into(),
            }],
            dict: ModelAsset {
                name: "keys.txt".into(),
                sha256: dict_hash.into(),
            },
        };
        std::fs::write(
            dir.join("manifest.json"),
            serde_json::to_vec(&manifest).unwrap(),
        )
        .unwrap();
    }

    #[test]
    fn a_matching_manifest_verifies() {
        let dir = tempfile::tempdir().unwrap();
        let det = write(dir.path(), "det.onnx", b"fake-model-weights");
        let dict = write(dir.path(), "keys.txt", b"a\nb\nc\n");
        manifest_with(dir.path(), &det, &dict);
        assert!(verify_models(dir.path()).is_ok());
    }

    #[test]
    fn a_tampered_model_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let _ = write(dir.path(), "det.onnx", b"fake-model-weights");
        let dict = write(dir.path(), "keys.txt", b"a\n");
        // 清单里写的是别的内容的哈希 → 校验必须失败
        manifest_with(dir.path(), &sha256_hex(b"something-else"), &dict);
        let err = verify_models(dir.path()).unwrap_err();
        assert!(err.contains("checksum mismatch"), "err={err}");
    }

    #[test]
    fn a_missing_asset_is_reported_rather_than_ignored() {
        let dir = tempfile::tempdir().unwrap();
        let dict = write(dir.path(), "keys.txt", b"a\n");
        manifest_with(dir.path(), &sha256_hex(b"x"), &dict);
        // det.onnx 根本不存在
        assert!(verify_models(dir.path()).is_err());
    }

    #[test]
    fn availability_is_false_without_the_feature_and_assets() {
        // 本机既没开特性也没装模型 —— 必须如实返回不可用,让路由去降级
        assert!(!is_available());
    }

    #[test]
    fn the_unavailable_error_names_the_alternatives() {
        let err = recognize(
            &OcrOptions::default(),
            &OcrRequest {
                image: b"",
                media_type: "image/jpeg",
                page: 0,
                languages: &[],
            },
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("remote"), "err={err}");
        assert!(err.contains("allow_cloud"), "err={err}");
    }
}
