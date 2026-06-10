use thiserror::Error;

#[derive(Debug, Error)]
pub enum ExecError {
    #[error("dsl: {0}")]
    Dsl(#[from] lumo_dsl::ParseError),
    #[error("template: {0}")]
    Template(#[from] lumo_dsl::TemplateError),
    #[error("validate: {0}")]
    Validate(#[from] lumo_dsl::ValidationError),
    #[error("storage: {0}")]
    Storage(#[from] lumo_storage::StorageError),
    #[error("unknown action `{0}`")]
    UnknownAction(String),
    #[error("step `{step}` failed: {source}")]
    Step {
        step: String,
        #[source]
        source: StepError,
    },
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    /// P1-1: the run was cancelled via its `CancelToken` before completing.
    #[error("run cancelled")]
    Cancelled,
    /// F-20: the run paused at a breakpoint / single-step before executing a
    /// step. Propagated as an `Err` to unwind the step loop (like `Cancelled`),
    /// but `run()` reports it via `RunReport.paused_at` and returns `Ok` — the
    /// authoritative "where" lives on the ctx's debug controller, not here.
    #[error("run paused at breakpoint")]
    Paused,
    /// P1-1: a step exceeded the configured per-step timeout.
    #[error("step `{step}` timed out after {ms}ms")]
    Timeout { step: String, ms: u64 },
    /// 指令集 P1:`control.break` 发出的循环中断信号。以 `Err` 形式向上 unwind,
    /// 由最近的循环容器(while / for / for_each)消化:终止该循环。穿过
    /// `control.try` 时不得被 catch 捕获;逃逸出循环作用域(parallel 分支边界 /
    /// flow 顶层)即按下面的错误信息作为运行期兜底报错。
    #[error("`control.break` used outside of a loop (step `{step}`)")]
    Break { step: String },
    /// 指令集 P1:`control.continue` 发出的跳轮信号。语义同 [`ExecError::Break`],
    /// 但循环容器消化后跳到下一轮而非终止循环。
    #[error("`control.continue` used outside of a loop (step `{step}`)")]
    Continue { step: String },
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

/// Kind of capability that was denied. Used by UI for "+ add to whitelist" buttons.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CapKind {
    Network,
    #[serde(rename = "fs.read")]
    FsRead,
    #[serde(rename = "fs.write")]
    FsWrite,
    Llm,
    Mcp,
    Desktop,
}

impl std::fmt::Display for CapKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CapKind::Network => write!(f, "network"),
            CapKind::FsRead => write!(f, "fs.read"),
            CapKind::FsWrite => write!(f, "fs.write"),
            CapKind::Llm => write!(f, "llm"),
            CapKind::Mcp => write!(f, "mcp"),
            CapKind::Desktop => write!(f, "desktop"),
        }
    }
}

#[derive(Debug, Error)]
pub enum StepError {
    #[error("{0}")]
    Message(String),
    #[error("user fail: {0}")]
    UserFail(String),
    #[error("retried {attempts} times, last error: {last}")]
    RetriesExceeded { attempts: u32, last: String },
    /// Capability sandbox denial. UI surfaces a "+ add to whitelist" button keyed off `kind`.
    #[error("capability denied: {kind} `{target}` is not declared")]
    CapabilityDenied { kind: CapKind, target: String },
    /// Selector targeting (CSS/XPath/A11y) failed to locate an element.
    /// VM checks for this kind to trigger AI heal-selector fallback.
    #[error("selector not found: {0}")]
    SelectorNotFound(String),
    /// Extraction (read text / attribute / table) returned empty / failed.
    /// VM checks for this kind to trigger AI extract-visual fallback.
    #[error("extract failed: {0}")]
    ExtractFailed(String),
    /// `control.if` cond expression evaluated to non-bool / null / error.
    /// VM checks for this kind to trigger AI decide fallback.
    #[error("cond eval error: {0}")]
    CondError(String),
    /// AI budget exhausted (max_calls_per_run). Surfaces as raw error, never tries AI again this run.
    #[error("AI budget exhausted ({max} calls per run)")]
    BudgetExceeded { max: u32 },
    /// P0-2:步级超时降级成的可重试错误。仅当该步 `retry.on` 显式包含
    /// `timeout` 且还有重试预算时,VM 才把 `StepOutcome::TimedOut` 转成本
    /// 变体送进重试循环;其余情况一律保持 `ExecError::Timeout` 硬中断语义。
    #[error("timed out after {ms}ms")]
    Timeout { ms: u64 },
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

impl StepError {
    pub fn msg(s: impl Into<String>) -> Self {
        Self::Message(s.into())
    }
}

/// Classification of any `StepError` for VM hook dispatch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorKind {
    SelectorNotFound,
    ExtractFailed,
    CondError,
    CapabilityDenied,
    BudgetExceeded,
    /// P0-2:步级超时(仅在 `retry.on` 显式列出 `timeout` 时可重试)。
    Timeout,
    Other,
}

impl ErrorKind {
    /// Stable snake_case spelling of the kind. Matches the serde representation
    /// and the names accepted in a step's `retry.on` filter (F-16).
    pub fn as_str(&self) -> &'static str {
        match self {
            ErrorKind::SelectorNotFound => "selector_not_found",
            ErrorKind::ExtractFailed => "extract_failed",
            ErrorKind::CondError => "cond_error",
            ErrorKind::CapabilityDenied => "capability_denied",
            ErrorKind::BudgetExceeded => "budget_exceeded",
            ErrorKind::Timeout => "timeout",
            ErrorKind::Other => "other",
        }
    }
}

impl StepError {
    pub fn kind(&self) -> ErrorKind {
        match self {
            StepError::SelectorNotFound(_) => ErrorKind::SelectorNotFound,
            StepError::ExtractFailed(_) => ErrorKind::ExtractFailed,
            StepError::CondError(_) => ErrorKind::CondError,
            StepError::CapabilityDenied { .. } => ErrorKind::CapabilityDenied,
            StepError::BudgetExceeded { .. } => ErrorKind::BudgetExceeded,
            StepError::Timeout { .. } => ErrorKind::Timeout,
            _ => ErrorKind::Other,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `ErrorKind::as_str` must stay aligned with the serde snake_case
    /// representation: `retry.on` names are matched against `as_str`, while the
    /// API/UI serialize the same kinds via serde — drift would silently break
    /// `retry.on` matching.
    #[test]
    fn error_kind_as_str_matches_serde() {
        for kind in [
            ErrorKind::SelectorNotFound,
            ErrorKind::ExtractFailed,
            ErrorKind::CondError,
            ErrorKind::CapabilityDenied,
            ErrorKind::BudgetExceeded,
            ErrorKind::Timeout,
            ErrorKind::Other,
        ] {
            let serde = serde_json::to_value(kind).unwrap();
            assert_eq!(
                serde,
                serde_json::Value::String(kind.as_str().to_string()),
                "as_str drifted from serde for {kind:?}"
            );
        }
    }

    /// P1-6:lumo-dsl 的 `RETRY_ON_KINDS` 白名单是本 enum `as_str` 的静态副本
    /// (lumo-dsl 不能依赖 lumo-core,否则循环依赖)。本测试断言两边一致——
    /// 新增 ErrorKind 而忘了同步白名单时在这里炸,而不是让合法的 `retry.on`
    /// 过不了 validate。
    #[test]
    fn retry_on_kinds_match_dsl_whitelist() {
        let all = [
            ErrorKind::SelectorNotFound,
            ErrorKind::ExtractFailed,
            ErrorKind::CondError,
            ErrorKind::CapabilityDenied,
            ErrorKind::BudgetExceeded,
            ErrorKind::Timeout,
            ErrorKind::Other,
        ];
        let actual: std::collections::BTreeSet<&str> = all.iter().map(|k| k.as_str()).collect();
        let whitelist: std::collections::BTreeSet<&str> =
            lumo_dsl::RETRY_ON_KINDS.iter().copied().collect();
        assert_eq!(
            whitelist, actual,
            "lumo-dsl RETRY_ON_KINDS 与 ErrorKind::as_str 漂移了——两边必须同步更新"
        );
    }
}
