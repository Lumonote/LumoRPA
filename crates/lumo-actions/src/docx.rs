//! Word `.docx` 动作族(`docx.*`)。
//!
//! `.docx` 本质是一个 zip(OOXML),正文在 `word/document.xml`。本族不引 docx-rs
//! (其传递依赖 image 重子树),而是复用本 crate 既有的纯 Rust `zip`:
//!
//! * `docx.read_text` —— 解出 `word/document.xml`,按 `<w:t>…</w:t>` 抽取文本、
//!   按段落 `</w:p>` 分段,返回段落数组 + 全文。
//! * `docx.replace_placeholders` —— 在 `word/document.xml` 上做 `{{key}}` → 值的
//!   字符串替换(套打/邮件合并),逐条目转写整个 zip 到输出路径。
//!
//! 全部为 LOCAL(不触网),走 `fs.read` / `fs.write` 能力闸门,且**先校验能力再
//! 碰文件**(与 `pdf.rs` / `file.rs` 一致),未授权路径快速失败。
//!
//! 占位符替换是字符串级:对“模板由本系统/Word 一次性输入、`{{key}}` 落在单个文本
//! run 内”的常见套打场景成立;若模板把 `{{key}}` 拆散到多个 `<w:r>`(例如手动逐字
//! 修改过),该 token 不会被识别 —— 这是 OOXML 套打的固有约束,模板侧规避即可。

use async_trait::async_trait;
use lumo_core::error::StepError;
use lumo_core::{Action, ActionRegistry, ActionResult, StepCtx};
use once_cell::sync::Lazy;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Map, Value};
use std::io::{Cursor, Read, Write};
use std::path::PathBuf;

pub fn register(r: &mut ActionRegistry) {
    r.register(ReadTextAction);
    r.register(ReplacePlaceholdersAction);
}

/// OOXML 正文文件名。
const DOCUMENT_XML: &str = "word/document.xml";

/// 把 `word/document.xml` 的文本按段落抽出来。每个 `</w:p>` 收一段,段内顺序拼接
/// 所有 `<w:t …>…</w:t>` 内容(无论嵌在表格 / 超链接 / run 的哪一层 —— 它们最终都
/// 落到 `<w:t>`)。返回非空段落,顺序保留。
fn paragraphs_from_xml(xml: &str) -> Vec<String> {
    let mut out = Vec::new();
    for para in xml.split("</w:p>") {
        let mut text = String::new();
        let mut rest = para;
        while let Some(i) = rest.find("<w:t") {
            let after = &rest[i..];
            // 跳过开标签到 '>',从其后取文本;自闭合 `<w:t/>` 无 `</w:t>` 时停止。
            let Some(gt) = after.find('>') else { break };
            let content_start = i + gt + 1;
            let Some(close) = rest[content_start..].find("</w:t>") else {
                break;
            };
            text.push_str(&rest[content_start..content_start + close]);
            rest = &rest[content_start + close + "</w:t>".len()..];
        }
        let unescaped = xml_unescape(&text);
        if !unescaped.trim().is_empty() {
            out.push(unescaped);
        }
    }
    out
}

fn xml_unescape(s: &str) -> String {
    s.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        // `&amp;` 必须最后处理,避免把实体名里的 `&` 提前还原。
        .replace("&amp;", "&")
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

// ---------------------------------------------------------------------------
// docx.read_text
// ---------------------------------------------------------------------------

pub struct ReadTextAction;
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ReadTextIn {
    path: PathBuf,
}

#[async_trait]
impl Action for ReadTextAction {
    fn id(&self) -> &'static str {
        "docx.read_text"
    }
    fn summary(&self) -> &'static str {
        "Extract paragraph text from a .docx file"
    }
    fn schema(&self) -> &'static serde_json::Value {
        static SCHEMA: Lazy<Value> = Lazy::new(crate::schema::derive::<ReadTextIn>);
        &SCHEMA
    }
    async fn execute(&self, ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let ReadTextIn { path } = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("docx.read_text input invalid: {e}")))?;
        // 先校验能力,再碰文件 —— 未授权路径即使不存在也要先失败。
        ctx.ensure_fs_read(&path)?;

        let path_for_blocking = path.clone();
        let paragraphs = tokio::task::spawn_blocking(move || read_text(&path_for_blocking))
            .await
            .map_err(|e| StepError::msg(format!("docx.read_text join: {e}")))??;
        let text = paragraphs.join("\n");
        Ok(ActionResult::from(serde_json::json!({
            "paragraphs": paragraphs,
            "text": text,
        })))
    }
}

fn read_text(path: &std::path::Path) -> Result<Vec<String>, StepError> {
    let bytes = std::fs::read(path)
        .map_err(|e| StepError::msg(format!("read {}: {e}", path.display())))?;
    let mut zip = zip::ZipArchive::new(Cursor::new(bytes))
        .map_err(|e| StepError::msg(format!("open docx {}: {e}", path.display())))?;
    let mut xml = String::new();
    zip.by_name(DOCUMENT_XML)
        .map_err(|e| StepError::msg(format!("docx missing {DOCUMENT_XML}: {e}")))?
        .read_to_string(&mut xml)
        .map_err(|e| StepError::msg(format!("read {DOCUMENT_XML}: {e}")))?;
    Ok(paragraphs_from_xml(&xml))
}

// ---------------------------------------------------------------------------
// docx.replace_placeholders
// ---------------------------------------------------------------------------

pub struct ReplacePlaceholdersAction;
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ReplaceIn {
    /// 模板 .docx 路径(fs.read 闸门)。
    template: PathBuf,
    /// 输出 .docx 路径(fs.write 闸门)。
    out: PathBuf,
    /// `{{key}}` → 值映射。非字符串值按 JSON 文本写入。
    values: Map<String, Value>,
}

#[async_trait]
impl Action for ReplacePlaceholdersAction {
    fn id(&self) -> &'static str {
        "docx.replace_placeholders"
    }
    fn summary(&self) -> &'static str {
        "Fill {{key}} placeholders in a .docx template and save to a new file"
    }
    fn schema(&self) -> &'static serde_json::Value {
        static SCHEMA: Lazy<Value> = Lazy::new(crate::schema::derive::<ReplaceIn>);
        &SCHEMA
    }
    async fn execute(&self, ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let ReplaceIn {
            template,
            out,
            values,
        } = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("docx.replace_placeholders input invalid: {e}")))?;
        // 读模板 + 写输出,两个闸门都先于碰文件校验。
        ctx.ensure_fs_read(&template)?;
        ctx.ensure_fs_write(&out)?;

        let tpl = template.clone();
        let dst = out.clone();
        let replaced = tokio::task::spawn_blocking(move || replace_placeholders(&tpl, &dst, &values))
            .await
            .map_err(|e| StepError::msg(format!("docx.replace_placeholders join: {e}")))??;
        Ok(ActionResult::from(serde_json::json!({
            "out": out,
            "replaced": replaced,
        })))
    }
}

/// 逐条目把 `word/document.xml` 里的 `{{key}}` 替换为转义后的值,其余 zip 条目原样
/// 转写到 `dst`。返回实际命中替换的占位符 token 数(命中即累加 1,无论出现几次)。
fn replace_placeholders(
    template: &std::path::Path,
    dst: &std::path::Path,
    values: &Map<String, Value>,
) -> Result<usize, StepError> {
    let bytes = std::fs::read(template)
        .map_err(|e| StepError::msg(format!("read {}: {e}", template.display())))?;
    let mut zip = zip::ZipArchive::new(Cursor::new(bytes))
        .map_err(|e| StepError::msg(format!("open docx {}: {e}", template.display())))?;

    let mut out_buf = Vec::new();
    let mut replaced = 0usize;
    {
        let mut writer = zip::ZipWriter::new(Cursor::new(&mut out_buf));
        for i in 0..zip.len() {
            let mut entry = zip
                .by_index(i)
                .map_err(|e| StepError::msg(format!("docx entry {i}: {e}")))?;
            let name = entry.name().to_string();
            let mut data = Vec::new();
            entry
                .read_to_end(&mut data)
                .map_err(|e| StepError::msg(format!("read entry {name}: {e}")))?;
            let opts: zip::write::FileOptions<()> =
                zip::write::FileOptions::default().compression_method(zip::CompressionMethod::Deflated);
            writer
                .start_file(&name, opts)
                .map_err(|e| StepError::msg(format!("write entry {name}: {e}")))?;
            if name == DOCUMENT_XML {
                let mut xml = String::from_utf8_lossy(&data).into_owned();
                for (k, v) in values {
                    let token = format!("{{{{{k}}}}}");
                    if xml.contains(&token) {
                        replaced += 1;
                    }
                    let val = match v {
                        Value::String(s) => s.clone(),
                        other => other.to_string(),
                    };
                    xml = xml.replace(&token, &xml_escape(&val));
                }
                writer
                    .write_all(xml.as_bytes())
                    .map_err(|e| StepError::msg(format!("write {DOCUMENT_XML}: {e}")))?;
            } else {
                writer
                    .write_all(&data)
                    .map_err(|e| StepError::msg(format!("write {name}: {e}")))?;
            }
        }
        writer
            .finish()
            .map_err(|e| StepError::msg(format!("finalize docx: {e}")))?;
    }

    if let Some(parent) = dst.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    std::fs::write(dst, &out_buf)
        .map_err(|e| StepError::msg(format!("save {}: {e}", dst.display())))?;
    Ok(replaced)
}
