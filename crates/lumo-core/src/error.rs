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
    /// P1-1: a step exceeded the configured per-step timeout.
    #[error("step `{step}` timed out after {ms}ms")]
    Timeout { step: String, ms: u64 },
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
}

impl std::fmt::Display for CapKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CapKind::Network => write!(f, "network"),
            CapKind::FsRead => write!(f, "fs.read"),
            CapKind::FsWrite => write!(f, "fs.write"),
            CapKind::Llm => write!(f, "llm"),
            CapKind::Mcp => write!(f, "mcp"),
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
    Other,
}

impl StepError {
    pub fn kind(&self) -> ErrorKind {
        match self {
            StepError::SelectorNotFound(_) => ErrorKind::SelectorNotFound,
            StepError::ExtractFailed(_) => ErrorKind::ExtractFailed,
            StepError::CondError(_) => ErrorKind::CondError,
            StepError::CapabilityDenied { .. } => ErrorKind::CapabilityDenied,
            StepError::BudgetExceeded { .. } => ErrorKind::BudgetExceeded,
            _ => ErrorKind::Other,
        }
    }
}
