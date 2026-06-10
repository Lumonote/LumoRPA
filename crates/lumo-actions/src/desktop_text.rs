//! 指令集缺口 P1:`desktop.click_text` —— OCR 文本定位点击,仅在 feature `desktop`
//! 下编译。F-1「坐标 + 图像」路线的文本驱动补件:不依赖模板小图,直接对屏幕文字下手。
//!
//! 流程:截屏(整屏或 `region`/`display` 限定,复用 desktop.screenshot 的 xcap 路径,
//! 全程内存 PNG 不落盘)→ 喂 AI provider 的 `ocr_image`(与 `image.ocr` 同一通路:
//! provider 路由 / `llm` 闸门 / 预算 / usage 台账全部共享),提示词要求返回
//! `[{"text", "bbox":[x,y,w,h]}]` 形状的 JSON(bbox 为所供图像的像素坐标)→ 本地按
//! `match`(contains/exact)过滤、`index` 取第 N 个命中 → bbox 中心换算**屏幕坐标**
//! → 复用 desktop.click 的 rdev 触发路径点击。
//!
//! 坐标换算(Retina 关键):xcap 的 `capture_image` 是**物理像素**,而 `Monitor` 的
//! x/y/width/height 与 rdev 鼠标事件同处一个坐标系(macOS 上均为逻辑点;Windows/X11
//! 上均为物理像素)。故缩放因子不取 `scale_factor()` 而是**现算** `整屏图宽 / 显示器宽`
//! —— macOS Retina 下 =2,Windows/X11 下 =1,两类平台同一公式自洽:
//! `screen_x = monitor.x + (region.x + bbox中心x) / scale`。换算是纯函数,单测钉死。
//!
//! 能力闸门:`desktop: ["screen", "mouse"]` + `llm`(OCR 走 LLM 通路,与 image.ocr
//! 一致);`dry_run: true` 只定位不点击,免 `mouse`。找不到目标文本(或 `index` 越界)
//! 报 `StepError::SelectorNotFound` —— 既有 `retry.on: [selector_not_found]` 直接可用,
//! 是「等文字渲染出来再点」最自然的重试语义。
//!
//! OCR 后端注:`ocr_image` 返回纯文本字符串,带框输出靠提示词约定 —— 云端视觉模型
//! (GLM-4V / Qwen-VL 等)可按要求吐 JSON;本地 ModelScope OCR 预设若只回纯文本,
//! 解析失败时给出指向性报错(换 vision 模型)而不是静默乱点。
//!
//! CI/headless 无显示器:闸门 / 入参 / provider 检查全部先于 xcap 截屏,纯函数
//! (解析 / 匹配 / 换算)单测不触屏;真截屏 + 真点击路径不进测试(需授权 + 会动鼠标)。

use crate::desktop::{send, ClickButton};
use crate::desktop_screen::{select_monitor, Region};
use async_trait::async_trait;
use lumo_core::error::StepError;
use lumo_core::{Action, ActionRegistry, ActionResult, StepCtx};
use once_cell::sync::Lazy;
use rdev::EventType;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{json, Value};

pub fn register(r: &mut ActionRegistry) {
    r.register(ClickTextAction);
}

// ---------------------------------------------------------------------------
// desktop.click_text
// ---------------------------------------------------------------------------

pub struct ClickTextAction;

/// 文本匹配语义:`contains`(默认,忽略大小写子串,对齐 window.activate 的
/// `title_contains` 先例)/ `exact`(候选 trim 后整串相等,大小写敏感)。
#[derive(Deserialize, JsonSchema, Default, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "lowercase")]
enum MatchMode {
    #[default]
    Contains,
    Exact,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ClickTextIn {
    /// 要点击的目标文本(按 `match` 语义与 OCR 结果比对)。
    text: String,
    /// 匹配语义:`contains`(默认,忽略大小写子串)/ `exact`(trim 后整串相等)。
    #[serde(rename = "match", default)]
    match_mode: MatchMode,
    /// 多命中时取第几个(0 起,按 OCR 返回顺序);默认 0。输出回报 `matches` 总数。
    #[serde(default)]
    index: usize,
    /// 可选截屏区域(所选显示器图像的像素坐标,同 desktop.screenshot);缩小范围
    /// 既省 OCR token 也避开同名文本干扰。
    #[serde(default)]
    region: Option<Region>,
    /// 显示器索引,0(默认)=主屏,同 desktop.screenshot。
    #[serde(default)]
    display: usize,
    /// 鼠标键:`left`(默认)/ `right` / `middle`,同 desktop.click。
    #[serde(default)]
    button: ClickButton,
    /// 是否双击,同 desktop.click。
    #[serde(default)]
    double: bool,
    /// true = 只定位不点击(免 `mouse` 闸门),返回 bbox 与换算后屏幕坐标,便于调试。
    #[serde(default)]
    dry_run: bool,
    /// 可选模型覆盖,同 image.ocr。留空用 provider 的 `ocr_model`/`vision_model`/默认。
    #[serde(default)]
    model: Option<String>,
}

/// 一条 OCR 文本项:文本 + 所供图像像素坐标系下的 bbox (x, y, w, h)。
#[derive(Clone, Debug, PartialEq)]
struct OcrItem {
    text: String,
    bbox: (f64, f64, f64, f64),
}

/// 截屏阶段产物(spawn_blocking 内一次收齐,避免跨 await 持有 xcap 句柄)。
struct Capture {
    /// 显示器原点 / 尺寸(与 rdev 同坐标系:macOS 逻辑点,Windows/X11 物理像素)。
    mon_x: i32,
    mon_y: i32,
    mon_w: u32,
    mon_h: u32,
    /// 整屏图像尺寸(物理像素,裁剪前)—— 与显示器尺寸之比即缩放因子。
    full_w: u32,
    full_h: u32,
    /// 喂 OCR 的 PNG(region 裁剪后)。
    png: Vec<u8>,
}

#[async_trait]
impl Action for ClickTextAction {
    fn id(&self) -> &'static str {
        "desktop.click_text"
    }
    fn summary(&self) -> &'static str {
        "Click on-screen text located via OCR (screenshot -> OCR -> coordinate click)"
    }
    fn schema(&self) -> &'static serde_json::Value {
        static SCHEMA: Lazy<Value> = Lazy::new(crate::schema::derive::<ClickTextIn>);
        &SCHEMA
    }
    async fn execute(&self, ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let ClickTextIn {
            text,
            match_mode,
            index,
            region,
            display,
            button,
            double,
            dry_run,
            model,
        } = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("desktop.click_text input invalid: {e}")))?;
        // 闸门先行,未授权时在触 xcap/显示器前快速失败(CI 无显示器可测):
        // 截屏要 screen;真点击另要 mouse(dry_run 只定位,免);OCR 走 LLM 要 llm。
        ctx.ensure_desktop("screen")?;
        if !dry_run {
            ctx.ensure_desktop("mouse")?;
        }
        ctx.ensure_llm(model.as_deref().unwrap_or(""))?;
        let provider = ctx.ai_provider().cloned().ok_or_else(|| {
            StepError::msg("desktop.click_text requires AI provider configuration")
        })?;
        if text.trim().is_empty() {
            return Err(StepError::msg(
                "desktop.click_text: text must not be empty".to_string(),
            ));
        }
        if let Some(r) = region {
            if r.width == 0 || r.height == 0 {
                return Err(StepError::msg(
                    "desktop.click_text: region width/height must be > 0".to_string(),
                ));
            }
        }

        // 1) 截屏(同 desktop.screenshot 的 xcap 阻塞路径;PNG 留内存不落盘)。
        let Capture {
            mon_x,
            mon_y,
            mon_w,
            mon_h,
            full_w,
            full_h,
            png,
        } = tokio::task::spawn_blocking(move || capture(display, region))
            .await
            .map_err(|e| StepError::msg(format!("desktop.click_text join: {e}")))??;

        // 2) OCR:与 image.ocr 同一 provider 通路(预算 / usage 台账共享)。
        let (crop_w, crop_h) = match region {
            Some(r) => (r.width, r.height),
            None => (full_w, full_h),
        };
        let raw = provider
            .ocr_image(
                png.into(),
                "image/png",
                &ocr_prompt(crop_w, crop_h),
                model.as_deref(),
            )
            .await?;

        // 3) 本地匹配 + 换算(纯函数,单测覆盖)。
        let items = parse_ocr_items(&raw)?;
        let (hit, matches) = pick_match(&items, &text, match_mode, index)?;
        let (cx, cy) = bbox_center(hit.bbox);
        let (sx, sy) = image_to_screen(
            mon_x,
            mon_y,
            mon_w,
            mon_h,
            full_w,
            full_h,
            region.map(|r| (r.x, r.y)).unwrap_or((0, 0)),
            cx,
            cy,
        )?;

        // 4) 点击(dry_run 跳过):复用 desktop.click 的 move + press/release 序列。
        if !dry_run {
            tokio::task::spawn_blocking(move || {
                send(EventType::MouseMove { x: sx, y: sy })?;
                let clicks = if double { 2 } else { 1 };
                for _ in 0..clicks {
                    send(EventType::ButtonPress(button.to_rdev()))?;
                    send(EventType::ButtonRelease(button.to_rdev()))?;
                }
                Ok::<(), StepError>(())
            })
            .await
            .map_err(|e| StepError::msg(format!("desktop.click_text join: {e}")))??;
        }
        Ok(ActionResult::from(json!({
            "clicked": !dry_run,
            "x": sx,
            "y": sy,
            "matched_text": hit.text,
            "matches": matches,
            // bbox 为(裁剪后)截图像素坐标 —— dry_run 调试时与截图直接对得上。
            "bbox": {
                "x": hit.bbox.0,
                "y": hit.bbox.1,
                "width": hit.bbox.2,
                "height": hit.bbox.3,
            },
        })))
    }
}

/// 截屏 + 收显示器几何(spawn_blocking 内执行;xcap 同步阻塞)。
fn capture(display: usize, region: Option<Region>) -> Result<Capture, StepError> {
    let monitor = select_monitor("desktop.click_text", display)?;
    let geo =
        |e: xcap::XCapError| StepError::msg(format!("desktop.click_text: monitor geometry: {e}"));
    let mon_x = monitor.x().map_err(geo)?;
    let mon_y = monitor.y().map_err(geo)?;
    let mon_w = monitor.width().map_err(geo)?;
    let mon_h = monitor.height().map_err(geo)?;
    let img = monitor
        .capture_image()
        .map_err(|e| StepError::msg(format!("desktop.click_text: capture failed: {e}")))?;
    let (full_w, full_h) = img.dimensions();
    let img = match region {
        Some(r) => {
            if r.x.saturating_add(r.width) > full_w || r.y.saturating_add(r.height) > full_h {
                return Err(StepError::msg(format!(
                    "desktop.click_text: region exceeds display bounds ({full_w}x{full_h})"
                )));
            }
            image::imageops::crop_imm(&img, r.x, r.y, r.width, r.height).to_image()
        }
        None => img,
    };
    let mut png = Vec::new();
    img.write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
        .map_err(|e| StepError::msg(format!("desktop.click_text encode: {e}")))?;
    Ok(Capture {
        mon_x,
        mon_y,
        mon_w,
        mon_h,
        full_w,
        full_h,
        png,
    })
}

/// 带框 OCR 提示词:严格 JSON 数组、像素坐标、报图像尺寸帮模型对齐坐标系。
fn ocr_prompt(w: u32, h: u32) -> String {
    format!(
        "You are an OCR engine. The image is {w}x{h} pixels. Detect every visible \
         text element and return ONLY a JSON array (no markdown fences, no commentary) \
         where each element is {{\"text\": string, \"bbox\": [x, y, width, height]}} \
         with bbox in pixel coordinates of this image, origin at the top-left corner."
    )
}

/// 剥掉模型常见的 ``` / ```json 围栏(尽管提示词禁了,稳妥起见仍兼容)。
fn strip_code_fence(raw: &str) -> &str {
    let s = raw.trim();
    let Some(s) = s.strip_prefix("```") else {
        return s;
    };
    // 去掉围栏行的语言标注(如 ```json)。
    let s = s.split_once('\n').map(|(_, rest)| rest).unwrap_or(s);
    s.trim().strip_suffix("```").unwrap_or(s).trim()
}

/// 解析 OCR 输出为文本项列表。接受裸数组或 `{"items": [...]}` 包裹;单项缺
/// text/bbox 或 bbox 形状不对则跳过(模型偶发噪声不应整步失败);整体非 JSON
/// → 指向性报错(多半是模型不支持带框输出)。
fn parse_ocr_items(raw: &str) -> Result<Vec<OcrItem>, StepError> {
    let s = strip_code_fence(raw);
    let value: Value = serde_json::from_str(s).map_err(|_| {
        StepError::msg(format!(
            "desktop.click_text: OCR output is not valid JSON (the configured model may \
             not support bounding-box output; use a vision model): {}",
            truncate(s, 200)
        ))
    })?;
    let arr = match &value {
        Value::Array(a) => a.as_slice(),
        Value::Object(o) => o
            .get("items")
            .and_then(|v| v.as_array())
            .map(|a| a.as_slice())
            .unwrap_or(&[]),
        _ => &[],
    };
    Ok(arr
        .iter()
        .filter_map(|item| {
            let text = item.get("text")?.as_str()?.to_string();
            let b = item.get("bbox")?.as_array()?;
            if b.len() != 4 {
                return None;
            }
            let n: Vec<f64> = b.iter().filter_map(|v| v.as_f64()).collect();
            if n.len() != 4 {
                return None;
            }
            Some(OcrItem {
                text,
                bbox: (n[0], n[1], n[2], n[3]),
            })
        })
        .collect())
}

fn truncate(s: &str, max: usize) -> &str {
    match s.char_indices().nth(max) {
        Some((i, _)) => &s[..i],
        None => s,
    }
}

/// 匹配语义:contains = 忽略大小写子串;exact = 候选 trim 后整串相等(OCR 常带
/// 首尾空白,trim 让 exact 可用;大小写保持敏感 —— 需要宽松就用 contains)。
fn text_matches(candidate: &str, target: &str, mode: MatchMode) -> bool {
    match mode {
        MatchMode::Contains => candidate
            .to_lowercase()
            .contains(&target.trim().to_lowercase()),
        MatchMode::Exact => candidate.trim() == target.trim(),
    }
}

/// 过滤 + 取第 `index` 个命中。零命中 / 越界都报 `SelectorNotFound` ——
/// `retry.on: [selector_not_found]` 下「等文字出现再点」开箱即用。
fn pick_match(
    items: &[OcrItem],
    target: &str,
    mode: MatchMode,
    index: usize,
) -> Result<(OcrItem, usize), StepError> {
    let hits: Vec<&OcrItem> = items
        .iter()
        .filter(|i| text_matches(&i.text, target, mode))
        .collect();
    let matches = hits.len();
    if matches == 0 {
        return Err(StepError::SelectorNotFound(format!(
            "desktop.click_text: no on-screen text matched `{target}` \
             ({} OCR item(s) scanned)",
            items.len()
        )));
    }
    let hit = hits.into_iter().nth(index).ok_or_else(|| {
        StepError::SelectorNotFound(format!(
            "desktop.click_text: index {index} out of range \
             ({matches} match(es) for `{target}`)"
        ))
    })?;
    Ok((hit.clone(), matches))
}

/// bbox (x, y, w, h) 的中心点。
fn bbox_center(bbox: (f64, f64, f64, f64)) -> (f64, f64) {
    (bbox.0 + bbox.2 / 2.0, bbox.1 + bbox.3 / 2.0)
}

/// (裁剪后)截图像素坐标 → 屏幕坐标(rdev 鼠标事件的坐标系)。
///
/// 缩放因子现算 `整屏图宽 / 显示器宽`:xcap 截图恒为物理像素,而 Monitor 几何与
/// rdev 同坐标系(macOS 逻辑点 → 因子=Retina 缩放;Windows/X11 物理像素 → 因子=1),
/// 同一公式两类平台自洽。region 偏移与 bbox 同为物理像素,先相加再除因子。
#[allow(clippy::too_many_arguments)]
fn image_to_screen(
    mon_x: i32,
    mon_y: i32,
    mon_w: u32,
    mon_h: u32,
    full_w: u32,
    full_h: u32,
    region_off: (u32, u32),
    px: f64,
    py: f64,
) -> Result<(f64, f64), StepError> {
    if mon_w == 0 || mon_h == 0 || full_w == 0 || full_h == 0 {
        return Err(StepError::msg(
            "desktop.click_text: zero-sized display geometry".to_string(),
        ));
    }
    let scale_x = full_w as f64 / mon_w as f64;
    let scale_y = full_h as f64 / mon_h as f64;
    let sx = mon_x as f64 + (region_off.0 as f64 + px) / scale_x;
    let sy = mon_y as f64 + (region_off.1 as f64 + py) / scale_y;
    Ok((sx, sy))
}

// ---------------------------------------------------------------------------
// 纯函数单测(CI 安全:不触 xcap / rdev / provider)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn item(text: &str, x: f64, y: f64, w: f64, h: f64) -> OcrItem {
        OcrItem {
            text: text.into(),
            bbox: (x, y, w, h),
        }
    }

    // ── parse_ocr_items ──────────────────────────────────────────────────────

    #[test]
    fn parse_accepts_bare_array() {
        let items = parse_ocr_items(r#"[{"text":"登录","bbox":[10,20,60,24]}]"#).expect("parse ok");
        assert_eq!(items, vec![item("登录", 10.0, 20.0, 60.0, 24.0)]);
    }

    #[test]
    fn parse_accepts_items_wrapper_and_code_fence() {
        let raw = "```json\n{\"items\":[{\"text\":\"OK\",\"bbox\":[1,2,3,4]}]}\n```";
        let items = parse_ocr_items(raw).expect("parse ok");
        assert_eq!(items, vec![item("OK", 1.0, 2.0, 3.0, 4.0)]);
    }

    #[test]
    fn parse_skips_malformed_entries() {
        let raw = r#"[
            {"text":"good","bbox":[1,2,3,4]},
            {"text":"no-bbox"},
            {"bbox":[1,2,3,4]},
            {"text":"short-bbox","bbox":[1,2]},
            {"text":"non-numeric","bbox":[1,2,3,"x"]}
        ]"#;
        let items = parse_ocr_items(raw).expect("parse ok");
        assert_eq!(items, vec![item("good", 1.0, 2.0, 3.0, 4.0)]);
    }

    #[test]
    fn parse_rejects_non_json_with_pointer_to_model_choice() {
        let err = parse_ocr_items("登录 注册 设置").unwrap_err().to_string();
        assert!(err.contains("not valid JSON"), "got: {err}");
        assert!(err.contains("vision model"), "got: {err}");
    }

    // ── 匹配语义 ─────────────────────────────────────────────────────────────

    #[test]
    fn contains_is_case_insensitive_substring() {
        assert!(text_matches("Sign In Now", "sign in", MatchMode::Contains));
        assert!(text_matches("登录账号", "登录", MatchMode::Contains));
        assert!(!text_matches("Sign Out", "sign in", MatchMode::Contains));
    }

    #[test]
    fn exact_trims_but_keeps_case() {
        assert!(text_matches("  登录  ", "登录", MatchMode::Exact));
        assert!(!text_matches("OK", "ok", MatchMode::Exact));
        assert!(!text_matches("确认登录", "登录", MatchMode::Exact));
    }

    // ── bbox 选取 / index / matches ─────────────────────────────────────────

    #[test]
    fn pick_reports_match_count_and_honors_index() {
        let items = vec![
            item("打开文件", 0.0, 0.0, 10.0, 10.0),
            item("保存", 0.0, 20.0, 10.0, 10.0),
            item("打开终端", 0.0, 40.0, 10.0, 10.0),
        ];
        let (hit, matches) = pick_match(&items, "打开", MatchMode::Contains, 1).expect("hit");
        assert_eq!(matches, 2);
        assert_eq!(hit.text, "打开终端");
    }

    #[test]
    fn pick_no_match_is_selector_not_found() {
        let items = vec![item("保存", 0.0, 0.0, 10.0, 10.0)];
        let err = pick_match(&items, "登录", MatchMode::Contains, 0).unwrap_err();
        assert!(
            matches!(err, StepError::SelectorNotFound(_)),
            "got: {err:?}"
        );
    }

    #[test]
    fn pick_index_out_of_range_is_selector_not_found() {
        let items = vec![item("登录", 0.0, 0.0, 10.0, 10.0)];
        let err = pick_match(&items, "登录", MatchMode::Contains, 3).unwrap_err();
        assert!(
            matches!(err, StepError::SelectorNotFound(ref m) if m.contains("index 3")),
            "got: {err:?}"
        );
    }

    // ── 坐标换算(Retina 缩放 / 显示器偏移 / region 偏移) ────────────────────

    #[test]
    fn center_of_bbox() {
        assert_eq!(bbox_center((10.0, 20.0, 60.0, 24.0)), (40.0, 32.0));
    }

    #[test]
    fn screen_coords_identity_when_scale_is_one() {
        // Windows/X11 常态:显示器尺寸与截图同为物理像素 → 因子 1,仅平移原点。
        let (x, y) = image_to_screen(0, 0, 1920, 1080, 1920, 1080, (0, 0), 960.0, 540.0)
            .expect("convert ok");
        assert_eq!((x, y), (960.0, 540.0));
    }

    #[test]
    fn screen_coords_halve_pixels_on_retina() {
        // macOS Retina:显示器 1440x900 逻辑点,截图 2880x1800 物理像素 → 因子 2。
        let (x, y) = image_to_screen(0, 0, 1440, 900, 2880, 1800, (0, 0), 2880.0, 1800.0)
            .expect("convert ok");
        assert_eq!((x, y), (1440.0, 900.0));
    }

    #[test]
    fn screen_coords_add_monitor_origin_and_region_offset() {
        // 副屏在主屏左侧(原点 -1440, 100),Retina 2x,region 从 (200, 80) 起裁。
        // bbox 中心 (100, 50) 在裁剪图内 → 物理 (300, 130) → 逻辑 (150, 65) → 平移。
        let (x, y) = image_to_screen(-1440, 100, 1440, 900, 2880, 1800, (200, 80), 100.0, 50.0)
            .expect("convert ok");
        assert_eq!((x, y), (-1440.0 + 150.0, 100.0 + 65.0));
    }

    #[test]
    fn screen_coords_reject_zero_geometry() {
        let err = image_to_screen(0, 0, 0, 900, 2880, 1800, (0, 0), 1.0, 1.0).unwrap_err();
        assert!(err.to_string().contains("zero-sized"), "got: {err}");
    }

    // ── 围栏剥离 ─────────────────────────────────────────────────────────────

    #[test]
    fn fence_stripping_variants() {
        assert_eq!(strip_code_fence("[1]"), "[1]");
        assert_eq!(strip_code_fence("```json\n[1]\n```"), "[1]");
        assert_eq!(strip_code_fence("```\n[1]\n```"), "[1]");
    }
}
