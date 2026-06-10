//! Human-in-the-loop 三动作（P1）—— `human.input` / `human.confirm` /
//! `human.approve`。
//!
//! 三者都经宿主注入的 [`HumanPrompter`]（见 `lumo-core::human`）等待真人回执；
//! 未注入实现的宿主上立刻报错（"host does not support human interaction"），
//! 绝不静默挂起。等待全程与运行取消（`CancelToken`）和自身 `timeout_ms`
//! 竞速；宿主全局步级超时（如桌面端 `LUMO_STEP_TIMEOUT_MS`，默认 10 分钟）
//! 由 VM 的 `select!` 在外层强制执行，本动作不绕过 —— 实际等待上限是二者较小
//! 值。超时语义：`human.input` / `human.confirm` 配了 `default` 则回落默认值，
//! 否则报 `StepError::Timeout`（`retry.on: [timeout]` 照常适用）；
//! `human.approve` 永不默认放行，超时一律报错。
//!
//! `human.*` 不触碰宿主 UI 之外的世界，故不设 capability 门禁；`human.approve`
//! 的 `notify` 字段复用 `notify.send` 的输入形状与网络门禁（见
//! [`crate::notify::send_value`]）。审批回执 v1 走与 confirm 相同的 prompter
//! 通道；webhook 回执留待后续。

use async_trait::async_trait;
use lumo_core::error::StepError;
use lumo_core::{
    Action, ActionRegistry, ActionResult, CancelToken, HumanPromptKind, HumanPromptRequest,
    HumanResponse, StepCtx,
};
use once_cell::sync::Lazy;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::Value;
use std::time::Duration;

pub fn register(r: &mut ActionRegistry) {
    r.register(InputAction);
    r.register(ConfirmAction);
    r.register(ApproveAction);
}

/// human.* 默认等待 1 小时 —— 远大于常规步骤；宿主的全局步级超时
/// （`LUMO_STEP_TIMEOUT_MS`）仍是硬上限，需要更长审批时间时同步调大它。
fn default_timeout_ms() -> u64 {
    3_600_000
}

/// 与 vm.rs 的 `wait_cancel` 同形：无取消令牌时永不分辨，在 `select!` 里安静闲置。
async fn wait_cancelled(cancel: &Option<CancelToken>) {
    match cancel {
        Some(c) => c.cancelled().await,
        None => std::future::pending::<()>().await,
    }
}

enum WaitOutcome {
    Response(HumanResponse),
    TimedOut,
}

/// 把一次提示发给宿主并等待回执，同时与运行取消、自身 `timeout_ms` 竞速。
/// 取消优先（`biased`）：取消的运行不该再吃掉一次超时默认值。
async fn wait_for_human(
    ctx: &StepCtx,
    action: &str,
    req: HumanPromptRequest,
) -> Result<WaitOutcome, StepError> {
    let prompter = ctx.human_prompter().ok_or_else(|| {
        StepError::msg(format!(
            "{action}: host does not support human interaction (no prompter attached)"
        ))
    })?;
    let cancel = ctx.cancel_token();
    let timeout_ms = req.timeout_ms;
    let fut = prompter.prompt(req);
    tokio::pin!(fut);
    tokio::select! {
        biased;
        _ = wait_cancelled(&cancel) => Err(StepError::msg(format!(
            "{action}: run cancelled while waiting for human response"
        ))),
        _ = tokio::time::sleep(Duration::from_millis(timeout_ms)) => Ok(WaitOutcome::TimedOut),
        r = &mut fut => r.map(WaitOutcome::Response),
    }
}

fn build_request(
    ctx: &StepCtx,
    kind: HumanPromptKind,
    message: String,
    default: Option<Value>,
    timeout_ms: u64,
) -> HumanPromptRequest {
    HumanPromptRequest {
        kind,
        message,
        default,
        timeout_ms,
        run_id: ctx.run_id().to_string(),
        step_path: ctx.current_step_path().unwrap_or_default(),
    }
}

/// 宿主回执 → 文本（`human.input`）。宿主标准形状是 string；其余 JSON 标量
/// 序列化兜底，避免一个宽容的前端把数字/布尔回执变成报错。
fn response_text(value: Value) -> String {
    match value {
        Value::String(s) => s,
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

/// 宿主回执 → 是/否（`human.confirm` / `human.approve`）。标准形状是 bool；
/// 也接受 y/yes/true/1、n/no/false/0 的字符串形式（大小写不敏感）。
fn response_bool(action: &str, value: &Value) -> Result<bool, StepError> {
    match value {
        Value::Bool(b) => Ok(*b),
        Value::String(s) => match s.trim().to_ascii_lowercase().as_str() {
            "y" | "yes" | "true" | "1" => Ok(true),
            "n" | "no" | "false" | "0" => Ok(false),
            other => Err(StepError::msg(format!(
                "{action}: cannot interpret human response `{other}` as yes/no"
            ))),
        },
        other => Err(StepError::msg(format!(
            "{action}: cannot interpret human response {other} as yes/no"
        ))),
    }
}

// ─── human.input ────────────────────────────────────────────────────────────

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct InputIn {
    /// 展示给操作员的提示文案。
    message: String,
    /// 超时回落的默认文本；不配则超时报错。
    #[serde(default)]
    default: Option<String>,
    #[serde(default = "default_timeout_ms")]
    timeout_ms: u64,
}

pub struct InputAction;

#[async_trait]
impl Action for InputAction {
    fn id(&self) -> &'static str {
        "human.input"
    }
    fn summary(&self) -> &'static str {
        "Ask the operator for a line of text and return {value} (host-provided prompt channel)"
    }
    fn schema(&self) -> &'static Value {
        static S: Lazy<Value> = Lazy::new(crate::schema::derive::<InputIn>);
        &S
    }
    async fn execute(&self, ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let InputIn {
            message,
            default,
            timeout_ms,
        } = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("human.input input invalid: {e}")))?;
        let req = build_request(
            ctx,
            HumanPromptKind::Input,
            message,
            default.clone().map(Value::String),
            timeout_ms,
        );
        match wait_for_human(ctx, "human.input", req).await? {
            WaitOutcome::Response(r) => Ok(ActionResult::from(serde_json::json!({
                "value": response_text(r.value),
            }))),
            WaitOutcome::TimedOut => match default {
                Some(d) => Ok(ActionResult::from(serde_json::json!({ "value": d }))),
                None => Err(StepError::Timeout { ms: timeout_ms }),
            },
        }
    }
}

// ─── human.confirm ──────────────────────────────────────────────────────────

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ConfirmIn {
    /// 展示给操作员的确认问句。
    message: String,
    /// 超时回落的默认答案；不配则超时报错。
    #[serde(default)]
    default: Option<bool>,
    #[serde(default = "default_timeout_ms")]
    timeout_ms: u64,
}

pub struct ConfirmAction;

#[async_trait]
impl Action for ConfirmAction {
    fn id(&self) -> &'static str {
        "human.confirm"
    }
    fn summary(&self) -> &'static str {
        "Ask the operator a yes/no question and return {confirmed} (timeout falls back to `default`)"
    }
    fn schema(&self) -> &'static Value {
        static S: Lazy<Value> = Lazy::new(crate::schema::derive::<ConfirmIn>);
        &S
    }
    async fn execute(&self, ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let ConfirmIn {
            message,
            default,
            timeout_ms,
        } = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("human.confirm input invalid: {e}")))?;
        let req = build_request(
            ctx,
            HumanPromptKind::Confirm,
            message,
            default.map(Value::Bool),
            timeout_ms,
        );
        match wait_for_human(ctx, "human.confirm", req).await? {
            WaitOutcome::Response(r) => {
                let confirmed = response_bool("human.confirm", &r.value)?;
                Ok(ActionResult::from(
                    serde_json::json!({ "confirmed": confirmed }),
                ))
            }
            WaitOutcome::TimedOut => match default {
                Some(d) => Ok(ActionResult::from(serde_json::json!({ "confirmed": d }))),
                None => Err(StepError::Timeout { ms: timeout_ms }),
            },
        }
    }
}

// ─── human.approve ──────────────────────────────────────────────────────────

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ApproveIn {
    /// 展示给审批人的审批事项描述。
    message: String,
    /// 可选：先经 notify 通道发审批通知，输入形状与 `notify.send` 完全一致
    /// （provider/url/text/payload/...），沿用其网络 capability 门禁。
    #[serde(default)]
    notify: Option<Value>,
    #[serde(default = "default_timeout_ms")]
    timeout_ms: u64,
}

pub struct ApproveAction;

#[async_trait]
impl Action for ApproveAction {
    fn id(&self) -> &'static str {
        "human.approve"
    }
    fn summary(&self) -> &'static str {
        "Request human approval (optional notify.send-shaped notification first); returns {approved, by, comment} — never auto-approves on timeout"
    }
    fn schema(&self) -> &'static Value {
        static S: Lazy<Value> = Lazy::new(crate::schema::derive::<ApproveIn>);
        &S
    }
    async fn execute(&self, ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let ApproveIn {
            message,
            notify,
            timeout_ms,
        } = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("human.approve input invalid: {e}")))?;
        // 先发通知（可选）：失败即失败本步 —— 审批人收不到通知却默默等满
        // 超时是最难排查的形态。门禁与投递完全复用 notify.send。
        if let Some(notify) = notify {
            crate::notify::send_value(ctx, "human.approve.notify", notify).await?;
        }
        let req = build_request(ctx, HumanPromptKind::Approve, message, None, timeout_ms);
        match wait_for_human(ctx, "human.approve", req).await? {
            WaitOutcome::Response(r) => {
                let approved = response_bool("human.approve", &r.value)?;
                Ok(ActionResult::from(serde_json::json!({
                    "approved": approved,
                    "by": r.by,
                    "comment": r.comment,
                })))
            }
            // 审批永不默认放行：超时一律报错（可用 retry.on: [timeout] 重试）。
            WaitOutcome::TimedOut => Err(StepError::Timeout { ms: timeout_ms }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lumo_core::HumanPrompter;
    use lumo_dsl::Capabilities;
    use std::sync::Arc;

    fn ctx() -> StepCtx {
        StepCtx::new(
            "run-human".into(),
            "flow-human".into(),
            ActionRegistry::new(),
            None,
            Value::Null,
            Capabilities::default(),
            Vec::new(),
        )
    }

    /// 固定回执的 mock 宿主。
    struct Fixed {
        value: Value,
        by: Option<String>,
    }

    #[async_trait]
    impl HumanPrompter for Fixed {
        async fn prompt(&self, _req: HumanPromptRequest) -> Result<HumanResponse, StepError> {
            Ok(HumanResponse {
                value: self.value.clone(),
                by: self.by.clone(),
                comment: None,
            })
        }
    }

    /// 永不回执的 mock 宿主（驱动超时 / 取消路径）。
    struct Never;

    #[async_trait]
    impl HumanPrompter for Never {
        async fn prompt(&self, _req: HumanPromptRequest) -> Result<HumanResponse, StepError> {
            std::future::pending().await
        }
    }

    fn with_prompter(p: impl HumanPrompter + 'static) -> StepCtx {
        ctx().with_human_prompter(Some(Arc::new(p)))
    }

    #[tokio::test]
    async fn human_actions_error_without_prompter() {
        // 未注入 prompter 的宿主必须立刻显式报错，绝不静默挂起。
        let mut c = ctx();
        for (action, input) in [
            (
                "human.input",
                serde_json::json!({ "message": "name?" }),
            ),
            (
                "human.confirm",
                serde_json::json!({ "message": "go?" }),
            ),
            (
                "human.approve",
                serde_json::json!({ "message": "ship it?" }),
            ),
        ] {
            let err = match action {
                "human.input" => InputAction.execute(&mut c, input).await,
                "human.confirm" => ConfirmAction.execute(&mut c, input).await,
                _ => ApproveAction.execute(&mut c, input).await,
            }
            .unwrap_err()
            .to_string();
            assert!(
                err.contains("does not support human interaction"),
                "{action} should name the missing prompter, got: {err}"
            );
        }
    }

    #[tokio::test]
    async fn input_round_trips_host_text() {
        let mut c = with_prompter(Fixed {
            value: Value::String("Alice".into()),
            by: None,
        });
        let out = InputAction
            .execute(&mut c, serde_json::json!({ "message": "name?" }))
            .await
            .unwrap()
            .output;
        assert_eq!(out, serde_json::json!({ "value": "Alice" }));
    }

    #[tokio::test]
    async fn confirm_round_trips_bool_and_yes_string() {
        let mut c = with_prompter(Fixed {
            value: Value::Bool(true),
            by: None,
        });
        let out = ConfirmAction
            .execute(&mut c, serde_json::json!({ "message": "go?" }))
            .await
            .unwrap()
            .output;
        assert_eq!(out, serde_json::json!({ "confirmed": true }));

        // 宽容形状：CLI/前端把 "y" 当回执也能解析。
        let mut c = with_prompter(Fixed {
            value: Value::String("y".into()),
            by: None,
        });
        let out = ConfirmAction
            .execute(&mut c, serde_json::json!({ "message": "go?" }))
            .await
            .unwrap()
            .output;
        assert_eq!(out, serde_json::json!({ "confirmed": true }));
    }

    #[tokio::test]
    async fn confirm_timeout_falls_back_to_default() {
        let mut c = with_prompter(Never);
        let out = ConfirmAction
            .execute(
                &mut c,
                serde_json::json!({ "message": "go?", "default": false, "timeout_ms": 40 }),
            )
            .await
            .unwrap()
            .output;
        assert_eq!(out, serde_json::json!({ "confirmed": false }));
    }

    #[tokio::test]
    async fn confirm_timeout_without_default_errors() {
        let mut c = with_prompter(Never);
        let err = ConfirmAction
            .execute(
                &mut c,
                serde_json::json!({ "message": "go?", "timeout_ms": 40 }),
            )
            .await
            .unwrap_err();
        assert!(
            matches!(err, StepError::Timeout { ms: 40 }),
            "timeout without default must surface StepError::Timeout, got: {err}"
        );
    }

    #[tokio::test]
    async fn cancel_interrupts_pending_wait() {
        let cancel = CancelToken::new();
        let mut c = with_prompter(Never).with_cancel(Some(cancel.clone()));
        let canceller = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(30)).await;
            cancel.cancel();
        });
        let err = InputAction
            .execute(
                &mut c,
                serde_json::json!({ "message": "name?", "timeout_ms": 60_000 }),
            )
            .await
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("cancelled"),
            "cancel must interrupt the human wait, got: {err}"
        );
        canceller.await.unwrap();
    }

    #[tokio::test]
    async fn approve_round_trips_with_by() {
        let mut c = with_prompter(Fixed {
            value: Value::Bool(true),
            by: Some("ops@corp".into()),
        });
        let out = ApproveAction
            .execute(&mut c, serde_json::json!({ "message": "deploy?" }))
            .await
            .unwrap()
            .output;
        assert_eq!(
            out,
            serde_json::json!({ "approved": true, "by": "ops@corp", "comment": null })
        );
    }

    #[tokio::test]
    async fn approve_timeout_never_auto_approves() {
        let mut c = with_prompter(Never);
        let err = ApproveAction
            .execute(
                &mut c,
                serde_json::json!({ "message": "deploy?", "timeout_ms": 40 }),
            )
            .await
            .unwrap_err();
        assert!(matches!(err, StepError::Timeout { ms: 40 }), "got: {err}");
    }

    #[tokio::test]
    async fn approve_notify_is_network_gated() {
        // notify 部分沿用 notify.send 的网络门禁：空 capabilities 下直接拒绝，
        // 不发请求也不进入等待。
        let mut c = with_prompter(Fixed {
            value: Value::Bool(true),
            by: None,
        });
        let err = ApproveAction
            .execute(
                &mut c,
                serde_json::json!({
                    "message": "deploy?",
                    "notify": { "provider": "webhook", "url": "https://hooks.example.com/x", "text": "approve pls" },
                }),
            )
            .await
            .unwrap_err();
        assert!(
            matches!(err, StepError::CapabilityDenied { .. }),
            "ungranted notify host must be denied, got: {err}"
        );
    }
}
