//! Clipboard actions — `clipboard.get` / `clipboard.set` (S-class F-5).
//!
//! Plain-text clipboard via `arboard`. Each call builds a short-lived
//! `Clipboard` inside `spawn_blocking` (arboard's handle is `!Send` and the call
//! is blocking). Headless / no-display environments surface a clear
//! `clipboard unavailable: …` error instead of panicking.
//!
//! No capability gate — these are local, info-only actions like `system.env_get`.
//! Two caveats flow authors should know: (1) reading the clipboard can expose
//! sensitive data (e.g. a password manager's last copy); (2) on Linux/X11 the
//! contents written may not persist after this process exits.

use async_trait::async_trait;
use lumo_core::error::StepError;
use lumo_core::{Action, ActionRegistry, ActionResult, StepCtx};
use once_cell::sync::Lazy;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::Value;

pub fn register(r: &mut ActionRegistry) {
    r.register(GetAction);
    r.register(SetAction);

    // ---- 兼容族:clipboard.get_text / set_text / clear / set_file ----
    // get_text / set_text 是同义别名,直接委托既有实现,避免双份 arboard 逻辑。
    r.register(GetTextAction);
    r.register(SetTextAction);
    r.register(ClearAction);
    r.register(SetFileAction);
}

// Indirection so the arboard backend can be swapped for a per-target stub
// (Task 7 fallback) without touching the action bodies.
fn clipboard_get_text() -> Result<String, String> {
    let mut cb = arboard::Clipboard::new().map_err(|e| format!("clipboard unavailable: {e}"))?;
    cb.get_text().map_err(|e| format!("clipboard read: {e}"))
}
fn clipboard_set_text(text: String) -> Result<(), String> {
    let mut cb = arboard::Clipboard::new().map_err(|e| format!("clipboard unavailable: {e}"))?;
    cb.set_text(text)
        .map_err(|e| format!("clipboard write: {e}"))
}

pub struct GetAction;

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct GetIn {}

#[async_trait]
impl Action for GetAction {
    fn id(&self) -> &'static str {
        "clipboard.get"
    }
    fn summary(&self) -> &'static str {
        "Read text from the system clipboard"
    }
    fn schema(&self) -> &'static Value {
        static SCHEMA: Lazy<Value> = Lazy::new(crate::schema::derive::<GetIn>);
        &SCHEMA
    }
    async fn execute(&self, _ctx: &mut StepCtx, _input: Value) -> Result<ActionResult, StepError> {
        let text = tokio::task::spawn_blocking(clipboard_get_text)
            .await
            .map_err(|e| StepError::msg(format!("clipboard.get task: {e}")))?
            .map_err(StepError::msg)?;
        Ok(ActionResult::from(serde_json::json!({ "text": text })))
    }
}

pub struct SetAction;

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SetIn {
    text: String,
}

#[async_trait]
impl Action for SetAction {
    fn id(&self) -> &'static str {
        "clipboard.set"
    }
    fn summary(&self) -> &'static str {
        "Write text to the system clipboard"
    }
    fn schema(&self) -> &'static Value {
        static SCHEMA: Lazy<Value> = Lazy::new(crate::schema::derive::<SetIn>);
        &SCHEMA
    }
    async fn execute(&self, _ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let SetIn { text } = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("clipboard.set input invalid: {e}")))?;
        tokio::task::spawn_blocking(move || clipboard_set_text(text))
            .await
            .map_err(|e| StepError::msg(format!("clipboard.set task: {e}")))?
            .map_err(StepError::msg)?;
        Ok(ActionResult::from(serde_json::json!({ "ok": true })))
    }
}
// ---------------------------------------------------------------------------
// 兼容族:clipboard.* 剪贴板
// ---------------------------------------------------------------------------
//
// 剪贴板指令族的四件套(get_text / set_text / clear / set_file)。
// get_text 与 set_text 是既有 clipboard.get / clipboard.set 的**同义别名**
// (命名),复用同一条 arboard 路径 —— 不复制实现,只复制入口。

/// 名:读取剪贴板文本(等价 `clipboard.get`)。
pub struct GetTextAction;
#[async_trait]
impl Action for GetTextAction {
    fn id(&self) -> &'static str {
        "clipboard.get_text"
    }
    fn summary(&self) -> &'static str {
        "Read clipboard text （同 `clipboard.get`）"
    }
    fn schema(&self) -> &'static Value {
        static SCHEMA: Lazy<Value> = Lazy::new(crate::schema::derive::<GetIn>);
        &SCHEMA
    }
    async fn execute(&self, ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        GetAction.execute(ctx, input).await
    }
}

/// 名:写入剪贴板文本(等价 `clipboard.set`)。
pub struct SetTextAction;
#[async_trait]
impl Action for SetTextAction {
    fn id(&self) -> &'static str {
        "clipboard.set_text"
    }
    fn summary(&self) -> &'static str {
        "Write clipboard text （同 `clipboard.set`）"
    }
    fn schema(&self) -> &'static Value {
        static SCHEMA: Lazy<Value> = Lazy::new(crate::schema::derive::<SetIn>);
        &SCHEMA
    }
    async fn execute(&self, ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        SetAction.execute(ctx, input).await
    }
}

/// 清空剪贴板(写入空串;arboard 无跨平台 clear,空串是等价可移植写法)。
pub struct ClearAction;
#[async_trait]
impl Action for ClearAction {
    fn id(&self) -> &'static str {
        "clipboard.clear"
    }
    fn summary(&self) -> &'static str {
        "Clear the clipboard by writing an empty string"
    }
    fn schema(&self) -> &'static Value {
        static SCHEMA: Lazy<Value> = Lazy::new(crate::schema::derive::<GetIn>);
        &SCHEMA
    }
    async fn execute(&self, _ctx: &mut StepCtx, _input: Value) -> Result<ActionResult, StepError> {
        tokio::task::spawn_blocking(|| clipboard_set_text(String::new()))
            .await
            .map_err(|e| StepError::msg(format!("clipboard.clear task: {e}")))?
            .map_err(StepError::msg)?;
        Ok(ActionResult::from(serde_json::json!({ "ok": true })))
    }
}

/// 把文件放进剪贴板(供「粘贴文件」)。
///
/// arboard 只暴露文本通道,跨平台的文件剪贴板格式(macOS `NSFilenamesPboardType`、
/// Windows `CF_HDROP`)各写各的。这里复用既有 arboard 文本通道放入**绝对路径**,
/// 这是无新依赖下最诚实的可移植行为:内容可被下游粘贴为路径文本,
/// 但**不等于**系统的「复制文件」语义 —— 所以返回值里显式标注 mode,
/// 让流程作者看得见差异,而不是以为拿到了原生文件剪贴。
pub struct SetFileAction;
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SetFileIn {
    path: std::path::PathBuf,
}
#[async_trait]
impl Action for SetFileAction {
    fn id(&self) -> &'static str {
        "clipboard.set_file"
    }
    fn summary(&self) -> &'static str {
        "Put a file path on the clipboard (path-text mode, not native file copy)"
    }
    fn schema(&self) -> &'static Value {
        static SCHEMA: Lazy<Value> = Lazy::new(crate::schema::derive::<SetFileIn>);
        &SCHEMA
    }
    async fn execute(&self, ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let SetFileIn { path } = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("clipboard.set_file input invalid: {e}")))?;
        ctx.ensure_fs_read(&path)?;
        let text = path.display().to_string();
        tokio::task::spawn_blocking(move || clipboard_set_text(text))
            .await
            .map_err(|e| StepError::msg(format!("clipboard.set_file task: {e}")))?
            .map_err(StepError::msg)?;
        Ok(ActionResult::from(serde_json::json!({
            "path": path,
            "ok": true,
            "mode": "path_text",
        })))
    }
}
