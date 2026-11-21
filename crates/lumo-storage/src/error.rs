use thiserror::Error;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum SendBlock {
    #[error("{0}")]
    Recipient(&'static str),
    #[error("本任务今日发送配额已用完（UTC）")]
    DailyLimit,
    #[error("尚未达到发送间隔，还需等待 {0} 毫秒")]
    Interval(u64),
}

#[derive(Debug, Error)]
pub enum StorageError {
    #[error("sqlite: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
    #[error("crypto: {0}")]
    Crypto(String),
    #[error("not found: {0}")]
    NotFound(String),
    #[error("invalid acquisition data: {0}")]
    Validation(String),
    #[error(transparent)]
    SendBlocked(#[from] SendBlock),
}
