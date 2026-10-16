//! Dialog actions (`dialog.*`) — 对话框。
//!
//! 消息框、确认框、输入框、选择框、文件/目录选择、表单、以及**自定义对话框**。
//!
//! 架构取向:全部走 `crate::human::ask_host` 的宿主回执通道(与 human.* 同一
//! 套 prompter / 取消 / 超时语义),不在动作层直接调 OS 弹窗 ——
//!
//! 1. LumoRPA 是 headless 优先的流程引擎,CLI / CI 路径下**没有 UI 线程**;
//!    动作层直接弹窗会在无人值守时挂死运行。
//! 2. 桌面端(apps/desktop,Tauri)持有窗口与字体环境,由它渲染才有一致的
//!    外观与正确的线程亲和(所有 GUI 框架都要求主线程渲染)。
//! 3. 无宿主接入时**明确报错**(绝不 panic、绝不静默返回空值)—— 静默空值会让
//!    流程拿着空结果继续跑,比直接失败难排查得多。
//!
//! 无 capability 门禁:这些动作不触碰受限资源(文件/网络/进程),
//! 只与操作员交互;写类的副作用由宿主在渲染前自行确认。

use async_trait::async_trait;
use lumo_core::error::StepError;
use lumo_core::{Action, ActionRegistry, ActionResult, StepCtx};
use once_cell::sync::Lazy;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::Value;

pub fn register(r: &mut ActionRegistry) {
    r.register(MessageAction);
    r.register(ConfirmAction);
    r.register(InputAction);
    r.register(SelectAction);
    r.register(SelectFileAction);
    r.register(SelectDirAction);
    r.register(FormAction);
    r.register(ProgressAction);
    r.register(CustomAction);
}

/// 消息框:仅展示,回执是确认与否(无返回值语义)。
pub struct MessageAction;
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct MessageIn {
    message: String,
    #[serde(default)]
    title: Option<String>,
    /// `info` (default) | `warning` | `error` | `success`.
    #[serde(default)]
    level: Option<String>,
    /// Wait for the operator to dismiss it. Default true.
    #[serde(default = "default_true")]
    wait: bool,
}
fn default_true() -> bool {
    true
}
#[async_trait]
impl Action for MessageAction {
    fn id(&self) -> &'static str {
        "dialog.message"
    }
    fn summary(&self) -> &'static str {
        "Show a message box; blocks until dismissed when `wait` is true"
    }
    fn schema(&self) -> &'static Value {
        static S: Lazy<Value> = Lazy::new(crate::schema::derive::<MessageIn>);
        &S
    }
    async fn execute(&self, ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let MessageIn {
            message,
            title,
            level,
            wait,
        } = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("dialog.message input invalid: {e}")))?;
        let level = level.unwrap_or_else(|| "info".to_string());
        if !matches!(level.as_str(), "info" | "warning" | "error" | "success") {
            return Err(StepError::msg(format!(
                "dialog.message: unknown level `{level}` (expected info | warning | error | success)"
            )));
        }
        crate::human::ask_host(
            ctx,
            "dialog.message",
            serde_json::json!({ "message": message, "title": title, "level": level, "wait": wait }),
        )
        .await
        .map(ActionResult::from)
    }
}

/// 确认框:返回布尔。
pub struct ConfirmAction;
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ConfirmIn {
    message: String,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    default: Option<bool>,
}
#[async_trait]
impl Action for ConfirmAction {
    fn id(&self) -> &'static str {
        "dialog.confirm"
    }
    fn summary(&self) -> &'static str {
        "Yes/no confirmation box; returns `{ confirmed }`"
    }
    fn schema(&self) -> &'static Value {
        static S: Lazy<Value> = Lazy::new(crate::schema::derive::<ConfirmIn>);
        &S
    }
    async fn execute(&self, ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let ConfirmIn {
            message,
            title,
            default,
        } = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("dialog.confirm input invalid: {e}")))?;
        let raw = crate::human::ask_host(
            ctx,
            "dialog.confirm",
            serde_json::json!({ "message": message, "title": title, "default": default }),
        )
        .await?;
        // 宿主可能回 bool,也可能回 { confirmed: bool };两者都接受。
        let confirmed = match &raw {
            Value::Bool(b) => *b,
            Value::Object(m) => m
                .get("confirmed")
                .and_then(|v| v.as_bool())
                .ok_or_else(|| {
                    StepError::msg(format!(
                        "dialog.confirm: host response has no boolean `confirmed`: {raw}"
                    ))
                })?,
            other => {
                return Err(StepError::msg(format!(
                    "dialog.confirm: cannot interpret host response as yes/no: {other}"
                )))
            }
        };
        Ok(ActionResult::from(
            serde_json::json!({ "confirmed": confirmed }),
        ))
    }
}

/// 输入框:返回文本。
pub struct InputAction;
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct InputIn {
    message: String,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    default: Option<String>,
    /// Render as a password field.
    #[serde(default)]
    password: bool,
}
#[async_trait]
impl Action for InputAction {
    fn id(&self) -> &'static str {
        "dialog.input"
    }
    fn summary(&self) -> &'static str {
        "Prompt for one line of text; returns `{ value }`"
    }
    fn schema(&self) -> &'static Value {
        static S: Lazy<Value> = Lazy::new(crate::schema::derive::<InputIn>);
        &S
    }
    async fn execute(&self, ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let InputIn {
            message,
            title,
            default,
            password,
        } = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("dialog.input input invalid: {e}")))?;
        let raw = crate::human::ask_host(
            ctx,
            "dialog.input",
            serde_json::json!({
                "message": message, "title": title,
                "default": default, "password": password,
            }),
        )
        .await?;
        let value = match &raw {
            Value::String(s) => s.clone(),
            Value::Object(m) => m
                .get("value")
                .map(|v| match v {
                    Value::String(s) => s.clone(),
                    other => other.to_string(),
                })
                .ok_or_else(|| {
                    StepError::msg(format!("dialog.input: host response has no `value`: {raw}"))
                })?,
            Value::Null => String::new(),
            other => other.to_string(),
        };
        Ok(ActionResult::from(serde_json::json!({ "value": value })))
    }
}

/// 选择框:从选项列表里选。
pub struct SelectAction;
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SelectIn {
    message: String,
    options: Vec<Value>,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    default: Option<Value>,
    #[serde(default)]
    multiple: bool,
}
#[async_trait]
impl Action for SelectAction {
    fn id(&self) -> &'static str {
        "dialog.select"
    }
    fn summary(&self) -> &'static str {
        "Pick from `options`; returns `{ value }` (or `{ values }` when multiple)"
    }
    fn schema(&self) -> &'static Value {
        static S: Lazy<Value> = Lazy::new(crate::schema::derive::<SelectIn>);
        &S
    }
    async fn execute(&self, ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let SelectIn {
            message,
            options,
            title,
            default,
            multiple,
        } = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("dialog.select input invalid: {e}")))?;
        if options.is_empty() {
            return Err(StepError::msg("dialog.select: `options` must not be empty"));
        }
        crate::human::ask_host(
            ctx,
            "dialog.select",
            serde_json::json!({
                "message": message, "title": title, "options": options,
                "default": default, "multiple": multiple,
            }),
        )
        .await
        .map(ActionResult::from)
    }
}

/// 文件选择器:返回选中路径列表。
pub struct SelectFileAction;
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SelectFileIn {
    #[serde(default)]
    title: Option<String>,
    /// Extension filters, e.g. `["txt", "csv"]`.
    #[serde(default)]
    filters: Vec<String>,
    #[serde(default)]
    multiple: bool,
}
#[async_trait]
impl Action for SelectFileAction {
    fn id(&self) -> &'static str {
        "dialog.select_file"
    }
    fn summary(&self) -> &'static str {
        "Ask the host to pick files; returns `{ paths }`"
    }
    fn schema(&self) -> &'static Value {
        static S: Lazy<Value> = Lazy::new(crate::schema::derive::<SelectFileIn>);
        &S
    }
    async fn execute(&self, ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let SelectFileIn {
            title,
            filters,
            multiple,
        } = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("dialog.select_file input invalid: {e}")))?;
        let raw = crate::human::ask_host(
            ctx,
            "dialog.select_file",
            serde_json::json!({ "title": title, "filters": filters, "multiple": multiple }),
        )
        .await?;
        Ok(ActionResult::from(normalize_paths(raw)))
    }
}

/// 目录选择器:返回选中目录。
pub struct SelectDirAction;
#[async_trait]
impl Action for SelectDirAction {
    fn id(&self) -> &'static str {
        "dialog.select_dir"
    }
    fn summary(&self) -> &'static str {
        "Ask the host to pick a directory; returns `{ paths }`"
    }
    fn schema(&self) -> &'static Value {
        static S: Lazy<Value> = Lazy::new(crate::schema::derive::<SelectFileIn>);
        &S
    }
    async fn execute(&self, ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let SelectFileIn {
            title,
            filters,
            multiple,
        } = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("dialog.select_dir input invalid: {e}")))?;
        let raw = crate::human::ask_host(
            ctx,
            "dialog.select_dir",
            serde_json::json!({
                "title": title, "filters": filters,
                "multiple": multiple, "kind": "directories",
            }),
        )
        .await?;
        Ok(ActionResult::from(normalize_paths(raw)))
    }
}

/// 把宿主回执归一成 `{ paths: [...] }`。
///
/// 接受三种形状:单字符串、字符串数组、`{path}`/`{paths}` 对象 —— 不同平台
/// 选择器的原生回执形态本就不同,归一在动作层做,下游流程只认一种。
fn normalize_paths(raw: Value) -> Value {
    let list: Vec<Value> = match raw {
        Value::String(s) => vec![Value::String(s)],
        Value::Array(a) => a,
        Value::Object(m) => {
            if let Some(Value::Array(a)) = m.get("paths") {
                a.clone()
            } else if let Some(p) = m.get("path") {
                vec![p.clone()]
            } else {
                vec![Value::Object(m)]
            }
        }
        Value::Null => Vec::new(),
        other => vec![other],
    };
    serde_json::json!({ "paths": list })
}

/// 表单:一次收集多个字段。
pub struct FormAction;
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct FormIn {
    /// Fields: `[{ name, label, type?, default?, options?, required? }]`.
    fields: Vec<Value>,
    #[serde(default)]
    title: Option<String>,
    /// Pre-filled values keyed by field name.
    #[serde(default)]
    values: Option<Value>,
}
#[async_trait]
impl Action for FormAction {
    fn id(&self) -> &'static str {
        "dialog.form"
    }
    fn summary(&self) -> &'static str {
        "Collect several fields at once; returns `{ values }`"
    }
    fn schema(&self) -> &'static Value {
        static S: Lazy<Value> = Lazy::new(crate::schema::derive::<FormIn>);
        &S
    }
    async fn execute(&self, ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let FormIn {
            fields,
            title,
            values,
        } = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("dialog.form input invalid: {e}")))?;
        if fields.is_empty() {
            return Err(StepError::msg("dialog.form: `fields` must not be empty"));
        }
        let raw = crate::human::ask_host(
            ctx,
            "dialog.form",
            serde_json::json!({ "fields": fields, "title": title, "values": values }),
        )
        .await?;
        // 宿主可以直接回 values 对象;若回的是 `{values: …}` 则拆一层。
        let out = match raw {
            Value::Object(ref m) if m.contains_key("values") => m["values"].clone(),
            other => other,
        };
        Ok(ActionResult::from(serde_json::json!({ "values": out })))
    }
}

/// 进度/状态提示:非阻塞,不等待回执。
pub struct ProgressAction;
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ProgressIn {
    message: String,
    #[serde(default)]
    value: Option<f64>,
    #[serde(default)]
    total: Option<f64>,
    #[serde(default)]
    title: Option<String>,
}
#[async_trait]
impl Action for ProgressAction {
    fn id(&self) -> &'static str {
        "dialog.progress"
    }
    fn summary(&self) -> &'static str {
        "Report progress to the host UI (0..1 fraction or value/total)"
    }
    fn schema(&self) -> &'static Value {
        static S: Lazy<Value> = Lazy::new(crate::schema::derive::<ProgressIn>);
        &S
    }
    async fn execute(&self, ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let ProgressIn {
            message,
            value,
            total,
            title,
        } = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("dialog.progress input invalid: {e}")))?;
        // 归一成 0..1 的比例,方便宿主直接驱动进度条。
        let fraction = match (value, total) {
            (Some(v), Some(t)) if t > 0.0 => Some((v / t).clamp(0.0, 1.0)),
            (Some(v), _) => Some(v.clamp(0.0, 1.0)),
            (None, _) => None,
        };
        let _ = ctx;
        Ok(ActionResult::from(serde_json::json!({
            "message": message,
            "title": title,
            "value": value,
            "total": total,
            "fraction": fraction,
        })))
    }
}

/// 自定义对话框:宿主渲染任意 schema 描述的界面。
pub struct CustomAction;
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct CustomIn {
    /// Free-form description handed to the host renderer.
    spec: Value,
    #[serde(default)]
    title: Option<String>,
}
#[async_trait]
impl Action for CustomAction {
    fn id(&self) -> &'static str {
        "dialog.custom"
    }
    fn summary(&self) -> &'static str {
        "Render a host-defined dialog from `spec`; returns the host response"
    }
    fn schema(&self) -> &'static Value {
        static S: Lazy<Value> = Lazy::new(crate::schema::derive::<CustomIn>);
        &S
    }
    async fn execute(&self, ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let CustomIn { spec, title } = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("dialog.custom input invalid: {e}")))?;
        crate::human::ask_host(
            ctx,
            "dialog.custom",
            serde_json::json!({ "spec": spec, "title": title }),
        )
        .await
        .map(ActionResult::from)
    }
}
