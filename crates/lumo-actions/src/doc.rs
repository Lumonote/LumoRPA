//! `doc.*` —— 文档理解动作族(设计见 `.omc/plans/doc-parsing-skill.md`)。
//!
//! 与既有 `pdf.*` / `docx.*` / `excel.*` 的分工:那三族是**格式操作**(写 PDF、
//! 替换占位符、读写单元格);本族是**文档理解** —— 输入任意文档,输出统一的
//! DocIR(结构化块 + 表格 + 图片 + 溯源)。两者正交并存,互不取代。
//!
//! 门禁与契约:
//! * 先校验能力再碰文件(同 `pdf.rs` 既有约定),未授权路径即使不存在也先失败。
//! * 解析是 CPU 密集且同步 → `spawn_blocking` + `with_timeout`。
//! * `limit` 有界并回报 `truncated`;大文档走 `output: file|artifact`,避免把
//!   数十 MB 的 DocIR 灌进模板上下文与运行存储。

use async_trait::async_trait;
use lumo_core::error::StepError;
use lumo_core::{Action, ActionRegistry, ActionResult, StepCtx};
use lumo_doc::{DocError, ParseOptions};
use once_cell::sync::Lazy;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::Value;
use std::path::PathBuf;

pub fn register(r: &mut ActionRegistry) {
    r.register(ParseAction);
    r.register(ToMarkdownAction);
    r.register(OcrAction);
}

/// 跑 OCR 并把结果并回 DocIR。返回写入的块数。
///
/// 只对**整页扫描图**做识别 —— 插图、logo 不值得也不该被当成正文来源。
async fn run_ocr(
    ctx: &mut StepCtx,
    options: &crate::ocr::OcrOptions,
    document: &mut lumo_doc::Document,
    media: &[(String, Vec<u8>)],
) -> Result<usize, StepError> {
    use crate::ocr::{recognize, resolve_engine, OcrRequest, Resolved};

    let targets: Vec<(usize, &str, &[u8])> = document
        .images
        .iter()
        .filter(|image| image.is_page_scan)
        .filter_map(|image| {
            media
                .iter()
                .find(|(id, _)| id == &image.id)
                .map(|(_, bytes)| (image.page, image.format.as_str(), bytes.as_slice()))
        })
        .collect();
    if targets.is_empty() {
        return Ok(0);
    }

    let resolved = resolve_engine(
        options,
        crate::ocr::native::is_available(),
        ctx.ai_provider().is_some(),
    );
    if let Resolved::Unavailable(reason) = &resolved {
        // 需要 OCR 但没有可用引擎:如实告警而不是静默出空结果
        document.diagnostics.warn(format!(
            "{} 页需要 OCR 但引擎不可用:{reason}",
            targets.len()
        ));
        return Ok(0);
    }

    let mut results = Vec::new();
    for (page, format, bytes) in targets {
        let media_type = format!("image/{format}");
        let request = OcrRequest {
            image: bytes,
            media_type: &media_type,
            page,
            languages: &options.lang,
        };
        match recognize(ctx, &resolved, options, request).await {
            Ok(result) => results.push(result),
            Err(e) => {
                // 单页失败不该毁掉整份文档,但必须留痕
                document
                    .diagnostics
                    .warn(format!("第 {} 页 OCR 失败:{e}", page + 1));
            }
        }
    }

    let low_confidence: Vec<usize> = results
        .iter()
        .filter(|r| {
            r.lines
                .iter()
                .any(|line| line.confidence < options.min_confidence)
        })
        .map(|r| r.page + 1)
        .collect();
    let written = lumo_doc::ocr::apply_ocr(document, &results, 0.05);
    if !low_confidence.is_empty() {
        document.diagnostics.warn(format!(
            "第 {} 页的 OCR 置信度低于 {},建议人工复核",
            low_confidence
                .iter()
                .map(|p| p.to_string())
                .collect::<Vec<_>>()
                .join("、"),
            options.min_confidence
        ));
    }
    Ok(written)
}

/// 引擎错误 → 步骤错误。分类对齐 retry kind:IO/格式问题不该被重试。
fn map_error(action: &str, error: DocError) -> StepError {
    match error {
        DocError::Io { .. } => StepError::io(format!("{action}: {error}")),
        other => StepError::msg(format!("{action}: {other}")),
    }
}

// ---------------------------------------------------------------------------
// doc.parse
// ---------------------------------------------------------------------------

#[derive(Deserialize, JsonSchema, Default)]
#[serde(deny_unknown_fields)]
struct ImageOptions {
    /// 抽取内嵌图片并落盘。需要 `fs.write` 授权 `dir`。
    #[serde(default)]
    extract: bool,
    #[serde(default)]
    dir: Option<PathBuf>,
}

#[derive(Deserialize, JsonSchema, Default)]
#[serde(rename_all = "snake_case")]
enum OutputMode {
    /// 完整 DocIR 进 step result(受 `limit` 约束)。
    #[default]
    Inline,
    /// 写 JSON 到 `out_path`,result 只回 `{path, bytes, count, truncated}`。
    File,
    /// 归档为运行级 artifact,result 回 `{artifact_id, ...}`。
    Artifact,
}

pub struct ParseAction;

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ParseIn {
    path: PathBuf,
    #[serde(default = "crate::contracts::default_collection_limit")]
    limit: usize,
    #[serde(default)]
    images: ImageOptions,
    #[serde(default)]
    output: OutputMode,
    /// OCR 配置。默认 `mode: auto` + `allow_cloud: false` —— 有本地模型才用,
    /// 内容不出网。
    #[serde(default)]
    ocr: crate::ocr::OcrOptions,
    #[serde(default)]
    out_path: Option<PathBuf>,
    /// 计算源文件 sha256 便于溯源;超大文件可关。
    #[serde(default = "default_true")]
    hash_source: bool,
    #[serde(default = "crate::contracts::default_action_timeout_ms")]
    timeout_ms: u64,
}

fn default_true() -> bool {
    true
}

#[async_trait]
impl Action for ParseAction {
    fn id(&self) -> &'static str {
        "doc.parse"
    }
    fn summary(&self) -> &'static str {
        "Parse a document (DOCX/XLSX) into structured DocIR: blocks, tables, images, outline"
    }
    fn schema(&self) -> &'static serde_json::Value {
        static SCHEMA: Lazy<Value> = Lazy::new(crate::schema::derive::<ParseIn>);
        &SCHEMA
    }

    async fn execute(&self, ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let ParseIn {
            path,
            limit,
            images,
            output,
            ocr,
            out_path,
            hash_source,
            timeout_ms,
        } = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("doc.parse input invalid: {e}")))?;
        let limit = crate::contracts::checked_limit("doc.parse", limit)?;

        // 先校验能力,再碰文件 —— 未授权路径即使不存在也要先失败。
        ctx.ensure_fs_read(&path)?;
        let image_dir = if images.extract {
            let dir = images.dir.clone().ok_or_else(|| {
                StepError::msg("doc.parse: `images.extract` 需要同时给出 `images.dir`")
            })?;
            ctx.ensure_fs_write(&dir)?;
            Some(dir)
        } else {
            None
        };
        if matches!(output, OutputMode::File) {
            let target = out_path.clone().ok_or_else(|| {
                StepError::msg("doc.parse: `output: file` 需要同时给出 `out_path`")
            })?;
            ctx.ensure_fs_write(&target)?;
        }

        let opts = ParseOptions {
            collect_images: images.extract,
            limit,
            hash_source,
        };
        let path_for_blocking = path.clone();
        let parsed = crate::contracts::with_timeout(timeout_ms, async move {
            tokio::task::spawn_blocking(move || {
                let bytes = std::fs::read(&path_for_blocking).map_err(|e| {
                    StepError::io(format!(
                        "doc.parse read {}: {e}",
                        path_for_blocking.display()
                    ))
                })?;
                lumo_doc::parse_bytes(&bytes, &path_for_blocking.to_string_lossy(), &opts)
                    .map_err(|e| map_error("doc.parse", e))
            })
            .await
            .map_err(|e| StepError::io(format!("doc.parse join: {e}")))?
        })
        .await?;

        let mut document = parsed.document;

        // 图片落盘:能力已在上面校验过,这里只做写入。写失败要连坐 —— 调用方
        // 明确要了图片,静默少几张比失败更糟。
        if let Some(dir) = image_dir {
            std::fs::create_dir_all(&dir)
                .map_err(|e| StepError::io(format!("doc.parse mkdir {}: {e}", dir.display())))?;
            for (image_id, bytes) in &parsed.media {
                let format = document
                    .images
                    .iter()
                    .find(|i| &i.id == image_id)
                    .map(|i| i.format.clone())
                    .unwrap_or_else(|| "bin".to_string());
                let target = dir.join(format!("{image_id}.{format}"));
                std::fs::write(&target, bytes).map_err(|e| {
                    StepError::io(format!("doc.parse write {}: {e}", target.display()))
                })?;
                if let Some(image) = document.images.iter_mut().find(|i| &i.id == image_id) {
                    image.path = Some(target.to_string_lossy().into_owned());
                }
            }
        }

        // OCR:只在有整页扫描图时才真正跑。引擎选择、出网授权都在 run_ocr 内部
        // 逐层把关(能力门禁 + allow_cloud 双重)。
        if !matches!(ocr.mode, crate::ocr::Mode::Off) {
            run_ocr(ctx, &ocr, &mut document, &parsed.media).await?;
        }

        let counts = serde_json::json!({
            "blocks": document.blocks.len(),
            "tables": document.tables.len(),
            "images": document.images.len(),
            "pages": document.pages.len(),
        });
        let truncated = document.diagnostics.truncated;

        match output {
            OutputMode::Inline => Ok(ActionResult::from(
                serde_json::to_value(&document)
                    .map_err(|e| StepError::msg(format!("doc.parse serialize: {e}")))?,
            )),
            OutputMode::File => {
                let target = out_path.expect("checked above");
                let json = serde_json::to_vec_pretty(&document)
                    .map_err(|e| StepError::msg(format!("doc.parse serialize: {e}")))?;
                std::fs::write(&target, &json).map_err(|e| {
                    StepError::io(format!("doc.parse write {}: {e}", target.display()))
                })?;
                Ok(ActionResult::from(serde_json::json!({
                    "path": target.to_string_lossy(),
                    "bytes": json.len(),
                    "counts": counts,
                    "truncated": truncated,
                    "diagnostics": document.diagnostics,
                })))
            }
            OutputMode::Artifact => {
                let json = serde_json::to_vec(&document)
                    .map_err(|e| StepError::msg(format!("doc.parse serialize: {e}")))?;
                let artifact_id = crate::attach_artifact_lenient(
                    ctx,
                    "doc.parse",
                    "docir",
                    "application/json",
                    &json,
                );
                Ok(ActionResult::from(serde_json::json!({
                    "artifact_id": artifact_id,
                    "bytes": json.len(),
                    "counts": counts,
                    "truncated": truncated,
                    "diagnostics": document.diagnostics,
                })))
            }
        }
    }
}

// ---------------------------------------------------------------------------
// doc.ocr
// ---------------------------------------------------------------------------

pub struct OcrAction;

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct OcrIn {
    /// 待识别的图片路径。
    image: PathBuf,
    #[serde(default)]
    ocr: crate::ocr::OcrOptions,
}

#[async_trait]
impl Action for OcrAction {
    fn id(&self) -> &'static str {
        "doc.ocr"
    }
    fn summary(&self) -> &'static str {
        "Recognize text in an image via the local model, a remote OCR endpoint, or a vision model"
    }
    fn schema(&self) -> &'static serde_json::Value {
        static SCHEMA: Lazy<Value> = Lazy::new(crate::schema::derive::<OcrIn>);
        &SCHEMA
    }

    async fn execute(&self, ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        use crate::ocr::{recognize, resolve_engine, OcrRequest, Resolved};

        let OcrIn { image, ocr } = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("doc.ocr input invalid: {e}")))?;
        // 先校验能力,再碰文件
        ctx.ensure_fs_read(&image)?;

        let resolved = resolve_engine(
            &ocr,
            crate::ocr::native::is_available(),
            ctx.ai_provider().is_some(),
        );
        if let Resolved::Unavailable(reason) = &resolved {
            // 说清楚为什么不可用,而不是返回空文本
            return Err(StepError::msg(format!("doc.ocr 不可用:{reason}")));
        }

        let bytes = tokio::fs::read(&image)
            .await
            .map_err(|e| StepError::io(format!("doc.ocr read {}: {e}", image.display())))?;
        let media_type = match image
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| e.to_ascii_lowercase())
            .as_deref()
        {
            Some("png") => "image/png",
            Some("jpg") | Some("jpeg") => "image/jpeg",
            Some("bmp") => "image/bmp",
            Some("tif") | Some("tiff") => "image/tiff",
            other => {
                return Err(StepError::msg(format!(
                    "doc.ocr: 不支持的图片扩展名 {other:?}(支持 png/jpg/jpeg/bmp/tif/tiff)"
                )))
            }
        };

        let result = recognize(
            ctx,
            &resolved,
            &ocr,
            OcrRequest {
                image: &bytes,
                media_type,
                page: 0,
                languages: &ocr.lang,
            },
        )
        .await?;

        let low_confidence = result
            .lines
            .iter()
            .filter(|line| line.confidence < ocr.min_confidence)
            .count();
        Ok(ActionResult::from(serde_json::json!({
            "text": result.text(),
            "lines": result.lines.iter().map(|line| serde_json::json!({
                "text": line.text,
                "confidence": line.confidence,
            })).collect::<Vec<_>>(),
            // 如实标注是哪档引擎出的结果,以及是否出过网
            "engine": format!("{:?}", result.source).to_lowercase(),
            "left_machine": result.source.leaves_machine(),
            "low_confidence_lines": low_confidence,
        })))
    }
}

pub struct ToMarkdownAction;

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ToMarkdownIn {
    /// 上游 `doc.parse` 的 DocIR。与 `path` 二选一。
    #[serde(default)]
    doc: Option<Value>,
    /// 直接读文件解析后转换(需 `fs.read`)。
    #[serde(default)]
    path: Option<PathBuf>,
    #[serde(default = "crate::contracts::default_collection_limit")]
    limit: usize,
    #[serde(default = "crate::contracts::default_action_timeout_ms")]
    timeout_ms: u64,
}

#[async_trait]
impl Action for ToMarkdownAction {
    fn id(&self) -> &'static str {
        "doc.to_markdown"
    }
    fn summary(&self) -> &'static str {
        "Render a parsed document (or a file) as Markdown with headings and tables"
    }
    fn schema(&self) -> &'static serde_json::Value {
        static SCHEMA: Lazy<Value> = Lazy::new(crate::schema::derive::<ToMarkdownIn>);
        &SCHEMA
    }

    async fn execute(&self, ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let ToMarkdownIn {
            doc,
            path,
            limit,
            timeout_ms,
        } = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("doc.to_markdown input invalid: {e}")))?;
        let limit = crate::contracts::checked_limit("doc.to_markdown", limit)?;

        let document: lumo_doc::Document = match (doc, path) {
            (Some(value), None) => serde_json::from_value(value).map_err(|e| {
                StepError::msg(format!("doc.to_markdown: `doc` 不是有效的 DocIR: {e}"))
            })?,
            (None, Some(path)) => {
                // 走文件路径才需要 fs.read —— 纯数据变换不该无谓要权限。
                ctx.ensure_fs_read(&path)?;
                let opts = ParseOptions {
                    collect_images: false,
                    limit,
                    hash_source: false,
                };
                crate::contracts::with_timeout(timeout_ms, async move {
                    tokio::task::spawn_blocking(move || {
                        let bytes = std::fs::read(&path).map_err(|e| {
                            StepError::io(format!("doc.to_markdown read {}: {e}", path.display()))
                        })?;
                        lumo_doc::parse_bytes(&bytes, &path.to_string_lossy(), &opts)
                            .map(|p| p.document)
                            .map_err(|e| map_error("doc.to_markdown", e))
                    })
                    .await
                    .map_err(|e| StepError::io(format!("doc.to_markdown join: {e}")))?
                })
                .await?
            }
            (Some(_), Some(_)) => {
                return Err(StepError::msg(
                    "doc.to_markdown: `doc` 与 `path` 只能给一个",
                ))
            }
            (None, None) => {
                return Err(StepError::msg(
                    "doc.to_markdown: 需要 `doc`(上游 DocIR)或 `path`(文件)",
                ))
            }
        };

        let markdown = lumo_doc::to_markdown(&document);
        Ok(ActionResult::from(serde_json::json!({
            "markdown": markdown,
            "blocks": document.blocks.len(),
            "tables": document.tables.len(),
        })))
    }
}
