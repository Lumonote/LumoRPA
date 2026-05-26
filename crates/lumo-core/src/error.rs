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
    Step { step: String, #[source] source: StepError },
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

#[derive(Debug, Error)]
pub enum StepError {
    #[error("{0}")]
    Message(String),
    #[error("user fail: {0}")]
    UserFail(String),
    #[error("retried {attempts} times, last error: {last}")]
    RetriesExceeded { attempts: u32, last: String },
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

impl StepError {
    pub fn msg(s: impl Into<String>) -> Self {
        Self::Message(s.into())
    }
}
