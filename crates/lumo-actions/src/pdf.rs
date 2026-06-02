//! PDF 动作：本地读取/抽取 + 写出。
//!
//! 全部基于纯 Rust 的 `lopdf`（MIT），无 C 依赖、可交叉编译/信创友好，
//! 读写共用一套 crate。三个动作均为 LOCAL（不触网），故走 `fs.read`/
//! `fs.write` 能力闸门 —— 与 `file.rs` 一致，并且**先校验能力再碰文件**，
//! 让未授权路径快速失败（"capability denied" + "fs.read"/"fs.write"）。

use async_trait::async_trait;
use lumo_core::error::StepError;
use lumo_core::{Action, ActionRegistry, ActionResult, StepCtx};
use once_cell::sync::Lazy;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::Value;
use std::path::PathBuf;

pub fn register(r: &mut ActionRegistry) {
    r.register(ExtractTextAction);
    r.register(InfoAction);
    r.register(WriteAction);
}

// ---------------------------------------------------------------------------
// pdf.extract_text
// ---------------------------------------------------------------------------

pub struct ExtractTextAction;
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ExtractTextIn {
    path: PathBuf,
}

#[async_trait]
impl Action for ExtractTextAction {
    fn id(&self) -> &'static str {
        "pdf.extract_text"
    }
    fn summary(&self) -> &'static str {
        "Extract text from a PDF file"
    }
    fn schema(&self) -> &'static serde_json::Value {
        static SCHEMA: Lazy<Value> = Lazy::new(crate::schema::derive::<ExtractTextIn>);
        &SCHEMA
    }
    async fn execute(&self, ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let ExtractTextIn { path } = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("pdf.extract_text input invalid: {e}")))?;
        // 先校验能力，再碰文件 —— 未授权路径即使不存在也要先失败。
        ctx.ensure_fs_read(&path)?;

        // lopdf 解析是 CPU 密集且同步，挪到阻塞线程池避免占用 async 执行器。
        let path_for_blocking = path.clone();
        let (text, pages) = tokio::task::spawn_blocking(move || extract(&path_for_blocking))
            .await
            .map_err(|e| StepError::msg(format!("pdf.extract_text join: {e}")))??;

        Ok(ActionResult::from(serde_json::json!({
            "text": text,
            "pages": pages,
        })))
    }
}

/// 加载 PDF，按页拼接文本，返回 (全文, 页数)。
fn extract(path: &std::path::Path) -> Result<(String, usize), StepError> {
    let doc = lopdf::Document::load(path)
        .map_err(|e| StepError::msg(format!("load pdf {}: {e}", path.display())))?;
    let page_numbers: Vec<u32> = doc.get_pages().keys().copied().collect();
    let pages = page_numbers.len();
    let text = doc
        .extract_text(&page_numbers)
        .map_err(|e| StepError::msg(format!("extract text {}: {e}", path.display())))?;
    Ok((text, pages))
}

// ---------------------------------------------------------------------------
// pdf.info
// ---------------------------------------------------------------------------

pub struct InfoAction;
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct InfoIn {
    path: PathBuf,
}

#[async_trait]
impl Action for InfoAction {
    fn id(&self) -> &'static str {
        "pdf.info"
    }
    fn summary(&self) -> &'static str {
        "Report PDF page count and cheap metadata"
    }
    fn schema(&self) -> &'static serde_json::Value {
        static SCHEMA: Lazy<Value> = Lazy::new(crate::schema::derive::<InfoIn>);
        &SCHEMA
    }
    async fn execute(&self, ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let InfoIn { path } = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("pdf.info input invalid: {e}")))?;
        ctx.ensure_fs_read(&path)?;

        let path_for_blocking = path.clone();
        let (pages, version) = tokio::task::spawn_blocking(move || info(&path_for_blocking))
            .await
            .map_err(|e| StepError::msg(format!("pdf.info join: {e}")))??;

        Ok(ActionResult::from(serde_json::json!({
            "pages": pages,
            "version": version,
        })))
    }
}

/// 加载 PDF，返回 (页数, PDF 版本号字符串)。
fn info(path: &std::path::Path) -> Result<(usize, String), StepError> {
    let doc = lopdf::Document::load(path)
        .map_err(|e| StepError::msg(format!("load pdf {}: {e}", path.display())))?;
    let pages = doc.get_pages().len();
    Ok((pages, doc.version.clone()))
}

// ---------------------------------------------------------------------------
// pdf.write
// ---------------------------------------------------------------------------

pub struct WriteAction;
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct WriteIn {
    path: PathBuf,
    /// 整段文本（按 `\n` 拆行）。与 `lines` 二选一，二者皆缺则报错。
    #[serde(default)]
    text: Option<String>,
    /// 行数组（已分行）。优先于 `text`。
    #[serde(default)]
    lines: Option<Vec<String>>,
    /// 字号（pt），默认 12。
    #[serde(default)]
    font_size: Option<f32>,
}

#[async_trait]
impl Action for WriteAction {
    fn id(&self) -> &'static str {
        "pdf.write"
    }
    fn summary(&self) -> &'static str {
        "Render a simple text PDF to a file"
    }
    fn schema(&self) -> &'static serde_json::Value {
        static SCHEMA: Lazy<Value> = Lazy::new(crate::schema::derive::<WriteIn>);
        &SCHEMA
    }
    async fn execute(&self, ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let WriteIn {
            path,
            text,
            lines,
            font_size,
        } = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("pdf.write input invalid: {e}")))?;
        // 先校验写能力，再创建文件（与 browser.screenshot 同样的 "gate before work"）。
        ctx.ensure_fs_write(&path)?;

        // 归一为行数组：lines 优先，否则按 \n 拆 text。
        let body: Vec<String> = match (lines, text) {
            (Some(ls), _) => ls,
            (None, Some(t)) => t.split('\n').map(|s| s.to_string()).collect(),
            (None, None) => {
                return Err(StepError::msg(
                    "pdf.write requires `text` or `lines`".to_string(),
                ))
            }
        };
        let size = font_size.unwrap_or(12.0);

        if let Some(parent) = path.parent() {
            let _ = tokio::fs::create_dir_all(parent).await;
        }

        let path_for_blocking = path.clone();
        tokio::task::spawn_blocking(move || write_pdf(&path_for_blocking, &body, size))
            .await
            .map_err(|e| StepError::msg(format!("pdf.write join: {e}")))??;

        Ok(ActionResult::from(serde_json::json!({ "path": path })))
    }
}

/// 用 lopdf 组装单页文本 PDF 并保存。每行一个 `Td` 下移（行距 = 1.2×字号），
/// 用内置 Courier（Type1，无需嵌入字体文件）。
fn write_pdf(path: &std::path::Path, lines: &[String], font_size: f32) -> Result<(), StepError> {
    use lopdf::content::{Content, Operation};
    use lopdf::{dictionary, Document, Object, Stream};

    let mut doc = Document::with_version("1.5");
    let pages_id = doc.new_object_id();

    let font_id = doc.add_object(dictionary! {
        "Type" => "Font",
        "Subtype" => "Type1",
        "BaseFont" => "Courier",
    });
    let resources_id = doc.add_object(dictionary! {
        "Font" => dictionary! { "F1" => font_id },
    });

    let leading = (font_size * 1.2).max(1.0);
    let mut operations = vec![
        Operation::new("BT", vec![]),
        Operation::new("Tf", vec!["F1".into(), font_size.into()]),
        // 行距，供 T* 使用。
        Operation::new("TL", vec![leading.into()]),
        // 起始位置：左边距 50pt，从顶部下移一个行距。
        Operation::new("Td", vec![50.into(), (792.0 - leading).into()]),
    ];
    for (i, line) in lines.iter().enumerate() {
        if i > 0 {
            // 换行到下一行起点。
            operations.push(Operation::new("T*", vec![]));
        }
        operations.push(Operation::new(
            "Tj",
            vec![Object::string_literal(line.as_bytes().to_vec())],
        ));
    }
    operations.push(Operation::new("ET", vec![]));

    let content = Content { operations };
    let content_id = doc.add_object(Stream::new(
        dictionary! {},
        content
            .encode()
            .map_err(|e| StepError::msg(format!("encode pdf content: {e}")))?,
    ));

    let page_id = doc.add_object(dictionary! {
        "Type" => "Page",
        "Parent" => pages_id,
        "Contents" => content_id,
    });
    let pages = dictionary! {
        "Type" => "Pages",
        "Kids" => vec![page_id.into()],
        "Count" => 1,
        "Resources" => resources_id,
        "MediaBox" => vec![0.into(), 0.into(), 612.into(), 792.into()],
    };
    doc.objects.insert(pages_id, Object::Dictionary(pages));

    let catalog_id = doc.add_object(dictionary! {
        "Type" => "Catalog",
        "Pages" => pages_id,
    });
    doc.trailer.set("Root", catalog_id);
    doc.compress();
    doc.save(path)
        .map_err(|e| StepError::msg(format!("save pdf {}: {e}", path.display())))?;
    Ok(())
}
