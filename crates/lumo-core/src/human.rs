//! P1（人机交互）：宿主注入的 Human-in-the-loop 提示通道。
//!
//! `human.input` / `human.confirm` / `human.approve` 三动作经
//! [`crate::StepCtx::human_prompter`] 取宿主实现并等待真人回执。引擎不内置任何
//! 交互方式 —— CLI 宿主在 TTY 上读 stdin，桌面宿主 emit Tauri 事件等前端回调；
//! 未注入实现的宿主（serve / mcp / 子流程 / 测试）上动作立刻报错，绝不静默挂起。

use crate::error::StepError;
use async_trait::async_trait;
use serde::Serialize;
use serde_json::Value;

/// 提示种类，对应三个 `human.*` 动作。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum HumanPromptKind {
    Input,
    Confirm,
    Approve,
}

/// 一次人机提示请求。`timeout_ms` 是动作层的等待上限（宿主可用来渲染倒计时）；
/// 真正的截止由动作侧 `select!` 与 VM 的全局步级超时共同控制，prompter 实现
/// 无须自己计时 —— 等待方 drop 本 future 即视为放弃本次提示。
#[derive(Debug, Clone, Serialize)]
pub struct HumanPromptRequest {
    pub kind: HumanPromptKind,
    pub message: String,
    /// input：默认文本；confirm：默认 y/n。原样透传给宿主用于渲染。
    pub default: Option<Value>,
    pub timeout_ms: u64,
    pub run_id: String,
    /// 完整嵌套步骤路径（如 `loop/item.3/ask`），供宿主界面定位提示来源。
    pub step_path: String,
}

/// 宿主回执。`value` 的形状随 kind：input → string；confirm/approve → bool。
#[derive(Debug, Clone)]
pub struct HumanResponse {
    pub value: Value,
    /// approve：回执人（可选）。
    pub by: Option<String>,
    /// approve：审批备注（可选）。
    pub comment: Option<String>,
}

/// 宿主实现的人机交互通道。实现必须对 drop 取消友好（动作侧把本 future 放进
/// `tokio::select!` 与运行取消 / 超时竞速），不要在其中做不可中断的阻塞。
#[async_trait]
pub trait HumanPrompter: Send + Sync {
    async fn prompt(&self, req: HumanPromptRequest) -> Result<HumanResponse, StepError>;
}
