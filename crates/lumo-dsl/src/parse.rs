use crate::ast::Flow;
use std::path::Path;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ParseError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("yaml: {0}")]
    Yaml(#[from] serde_yaml::Error),
    #[error("unsupported apiVersion `{0}` (expected `lumorpa.io/v1`)")]
    UnsupportedApi(String),
    #[error("unsupported kind `{0}` (expected `Flow`)")]
    UnsupportedKind(String),
}

pub fn parse_str(s: &str) -> Result<Flow, ParseError> {
    let flow: Flow = serde_yaml::from_str(s)?;
    if flow.api_version != "lumorpa.io/v1" {
        return Err(ParseError::UnsupportedApi(flow.api_version));
    }
    if flow.kind != "Flow" {
        return Err(ParseError::UnsupportedKind(flow.kind));
    }
    Ok(flow)
}

pub fn parse_file(path: impl AsRef<Path>) -> Result<Flow, ParseError> {
    let s = std::fs::read_to_string(path)?;
    parse_str(&s)
}
