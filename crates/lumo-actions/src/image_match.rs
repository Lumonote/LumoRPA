//! F-2 图像 / 模板匹配动作。
//!
//! 纯 Rust 的 `image` + `imageproc`(均 MIT),无 C 依赖、可交叉编译 / 信创友好。
//! OCR 文本识别走可配置的多模态模型,避免引入平台相关的本地 C/C++ OCR 依赖。
//!
//! - `image.locate` —— 在大图(haystack)中用归一化互相关定位模板小图,返回最佳
//!   匹配的左上角 + 中心坐标 + 分数(中心坐标可供 F-1 坐标驱动点击)。
//! - `image.compare` —— 两张**同尺寸**图的整体相似度(归一化互相关,1≈完全一致)。
//! - `image.ocr` —— 调用 AI hook provider 对本地图片做 OCR/结构化识别。
//!
//! 模板匹配动作均 LOCAL(不触网),走 `fs.read` 能力闸门(先校验能力、再读盘);
//! `image.ocr` 额外需要 `llm` 能力和已配置 AI provider。解码 + 匹配是 CPU
//! 密集且同步,挪到 `spawn_blocking` 不阻塞 async 执行器。
//!
//! 性能注:`match_template` 复杂度约 O(W·H·w·h),超大 haystack(如 4K 整屏)会偏慢,
//! 后续可换并行版 / 先降采样;当前求正确优先。

use async_trait::async_trait;
use image::GrayImage;
use imageproc::template_matching::{find_extremes, match_template, MatchTemplateMethod};
use lumo_core::error::StepError;
use lumo_core::{Action, ActionRegistry, ActionResult, StepCtx};
use once_cell::sync::Lazy;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::Value;
use std::path::{Path, PathBuf};

pub fn register(r: &mut ActionRegistry) {
    r.register(LocateAction);
    r.register(CompareAction);
    r.register(OcrAction);
}

/// 读图并转灰度(模板匹配在 luma 上进行)。
fn load_gray(path: &Path) -> Result<GrayImage, StepError> {
    let img = image::open(path)
        .map_err(|e| StepError::msg(format!("open image {}: {e}", path.display())))?;
    Ok(img.to_luma8())
}

/// 把非有限值(NaN/Inf,如对零方差区域做归一化互相关)归一为 0.0 ——
/// 便于 JSON 序列化(serde_json 会把 NaN 序列化成 null)且对调用方友好。
fn finite(x: f32) -> f32 {
    if x.is_finite() {
        x
    } else {
        0.0
    }
}

fn infer_media_type(path: &Path, explicit: Option<String>) -> Result<String, StepError> {
    if let Some(mt) = explicit {
        let mt = mt.trim();
        if mt.is_empty() {
            return Err(StepError::msg(
                "image.ocr: media_type must not be empty".to_string(),
            ));
        }
        if !mt.starts_with("image/") {
            return Err(StepError::msg(format!(
                "image.ocr: media_type must start with image/, got {mt}"
            )));
        }
        return Ok(mt.to_string());
    }

    let ext = path
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    match ext.as_str() {
        "png" => Ok("image/png".to_string()),
        "jpg" | "jpeg" => Ok("image/jpeg".to_string()),
        "webp" => Ok("image/webp".to_string()),
        "gif" => Ok("image/gif".to_string()),
        "bmp" => Ok("image/bmp".to_string()),
        "tif" | "tiff" => Ok("image/tiff".to_string()),
        _ => Err(StepError::msg(format!(
            "image.ocr: cannot infer media_type from {}; pass media_type explicitly",
            path.display()
        ))),
    }
}

// ---------------------------------------------------------------------------
// image.locate
// ---------------------------------------------------------------------------

pub struct LocateAction;

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct LocateIn {
    /// 被搜索的大图路径(如 `browser.screenshot` 的产物)。
    image: PathBuf,
    /// 模板小图路径(须不大于大图)。
    template: PathBuf,
    /// 命中阈值:归一化互相关分数 ≥ 阈值才算找到。默认 0.9。
    #[serde(default)]
    threshold: Option<f32>,
}

#[async_trait]
impl Action for LocateAction {
    fn id(&self) -> &'static str {
        "image.locate"
    }
    fn summary(&self) -> &'static str {
        "Locate a template image inside a larger image (normalized cross-correlation)"
    }
    fn schema(&self) -> &'static serde_json::Value {
        static SCHEMA: Lazy<Value> = Lazy::new(crate::schema::derive::<LocateIn>);
        &SCHEMA
    }
    async fn execute(&self, ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let LocateIn {
            image,
            template,
            threshold,
        } = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("image.locate input invalid: {e}")))?;
        // 先校验能力,再碰文件 —— 两个路径都需 fs.read。
        ctx.ensure_fs_read(&image)?;
        ctx.ensure_fs_read(&template)?;

        let (img_p, tpl_p) = (image.clone(), template.clone());
        let m = tokio::task::spawn_blocking(move || locate(&img_p, &tpl_p))
            .await
            .map_err(|e| StepError::msg(format!("image.locate join: {e}")))??;

        let thr = threshold.unwrap_or(0.9);
        Ok(ActionResult::from(serde_json::json!({
            "found": m.score >= thr,
            "score": m.score,
            "threshold": thr,
            "x": m.x,
            "y": m.y,
            "width": m.w,
            "height": m.h,
            "center_x": m.x + m.w / 2,
            "center_y": m.y + m.h / 2,
        })))
    }
}

/// 最佳匹配:左上角 (x,y) + 模板尺寸 (w,h) + 归一化互相关分数。
struct Located {
    score: f32,
    x: u32,
    y: u32,
    w: u32,
    h: u32,
}

fn locate(image_path: &Path, template_path: &Path) -> Result<Located, StepError> {
    let haystack = load_gray(image_path)?;
    let template = load_gray(template_path)?;
    let (hw, hh) = haystack.dimensions();
    let (tw, th) = template.dimensions();
    if tw == 0 || th == 0 {
        return Err(StepError::msg(
            "image.locate: template image is empty".to_string(),
        ));
    }
    if tw > hw || th > hh {
        return Err(StepError::msg(format!(
            "image.locate: template ({tw}x{th}) is larger than image ({hw}x{hh})"
        )));
    }
    let result = match_template(
        &haystack,
        &template,
        MatchTemplateMethod::CrossCorrelationNormalized,
    );
    let ex = find_extremes(&result);
    // 归一化互相关:分数越大越匹配 → 取最大值位置。
    Ok(Located {
        score: finite(ex.max_value),
        x: ex.max_value_location.0,
        y: ex.max_value_location.1,
        w: tw,
        h: th,
    })
}

// ---------------------------------------------------------------------------
// image.compare
// ---------------------------------------------------------------------------

pub struct CompareAction;

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct CompareIn {
    /// 第一张图路径。
    a: PathBuf,
    /// 第二张图路径(须与 `a` 同尺寸)。
    b: PathBuf,
}

#[async_trait]
impl Action for CompareAction {
    fn id(&self) -> &'static str {
        "image.compare"
    }
    fn summary(&self) -> &'static str {
        "Compare two equally-sized images, returning a normalized similarity score"
    }
    fn schema(&self) -> &'static serde_json::Value {
        static SCHEMA: Lazy<Value> = Lazy::new(crate::schema::derive::<CompareIn>);
        &SCHEMA
    }
    async fn execute(&self, ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let CompareIn { a, b } = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("image.compare input invalid: {e}")))?;
        ctx.ensure_fs_read(&a)?;
        ctx.ensure_fs_read(&b)?;

        let (ap, bp) = (a.clone(), b.clone());
        let score = tokio::task::spawn_blocking(move || compare(&ap, &bp))
            .await
            .map_err(|e| StepError::msg(format!("image.compare join: {e}")))??;

        Ok(ActionResult::from(serde_json::json!({
            "score": score,
            "identical": score >= 0.9999,
        })))
    }
}

fn compare(a: &Path, b: &Path) -> Result<f32, StepError> {
    let ga = load_gray(a)?;
    let gb = load_gray(b)?;
    if ga.dimensions() != gb.dimensions() {
        return Err(StepError::msg(format!(
            "image.compare: dimension mismatch {:?} vs {:?}",
            ga.dimensions(),
            gb.dimensions()
        )));
    }
    // 同尺寸 → match_template 结果为 1×1,取该归一化互相关分数。
    let result = match_template(&ga, &gb, MatchTemplateMethod::CrossCorrelationNormalized);
    let ex = find_extremes(&result);
    Ok(finite(ex.max_value))
}

// ---------------------------------------------------------------------------
// image.ocr
// ---------------------------------------------------------------------------

pub struct OcrAction;

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct OcrIn {
    /// 本地图片路径。
    image: PathBuf,
    /// 可选提示词。留空时默认提取全部可读文字并保留换行。
    #[serde(default)]
    prompt: Option<String>,
    /// 可选模型覆盖。留空时使用 provider 的 `ocr_model` / `vision_model` / 默认模型。
    #[serde(default)]
    model: Option<String>,
    /// 可选 MIME 类型。留空时按扩展名推断。
    #[serde(default)]
    media_type: Option<String>,
}

#[async_trait]
impl Action for OcrAction {
    fn id(&self) -> &'static str {
        "image.ocr"
    }
    fn summary(&self) -> &'static str {
        "Extract text or structured data from a local image using the configured OCR/vision model"
    }
    fn schema(&self) -> &'static serde_json::Value {
        static SCHEMA: Lazy<Value> = Lazy::new(crate::schema::derive::<OcrIn>);
        &SCHEMA
    }
    async fn execute(&self, ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let OcrIn {
            image,
            prompt,
            model,
            media_type,
        } = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("image.ocr input invalid: {e}")))?;
        ctx.ensure_fs_read(&image)?;
        ctx.ensure_llm(model.as_deref().unwrap_or(""))?;
        let provider = ctx
            .ai_provider()
            .cloned()
            .ok_or_else(|| StepError::msg("image.ocr requires AI provider configuration"))?;
        let media_type = infer_media_type(&image, media_type)?;
        let bytes = tokio::fs::read(&image)
            .await
            .map_err(|e| StepError::msg(format!("image.ocr read {}: {e}", image.display())))?;
        let prompt = prompt.unwrap_or_default();
        let text = provider
            .ocr_image(bytes.into(), &media_type, &prompt, model.as_deref())
            .await?;
        Ok(ActionResult::from(serde_json::json!({
            "text": text,
            "media_type": media_type,
        })))
    }
}
