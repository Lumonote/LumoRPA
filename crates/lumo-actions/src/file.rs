//! File system actions.

use async_trait::async_trait;
use lumo_core::{Action, ActionRegistry, ActionResult, StepCtx};
use lumo_core::error::StepError;
use serde::Deserialize;
use serde_json::Value;
use std::path::PathBuf;

pub fn register(r: &mut ActionRegistry) {
    r.register(ReadAction);
    r.register(WriteAction);
    r.register(ExistsAction);
}

pub struct ReadAction;
#[derive(Deserialize)]
struct ReadIn { path: PathBuf }

#[async_trait]
impl Action for ReadAction {
    fn id(&self) -> &'static str { "file.read" }
    fn summary(&self) -> &'static str { "Read a text file" }
    async fn execute(&self, _ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let ReadIn { path } = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("file.read input invalid: {e}")))?;
        let content = tokio::fs::read_to_string(&path).await
            .map_err(|e| StepError::msg(format!("read {}: {e}", path.display())))?;
        Ok(ActionResult::from(Value::String(content)))
    }
}

pub struct WriteAction;
#[derive(Deserialize)]
struct WriteIn { path: PathBuf, content: String, #[serde(default)] append: bool }

#[async_trait]
impl Action for WriteAction {
    fn id(&self) -> &'static str { "file.write" }
    fn summary(&self) -> &'static str { "Write a text file (create or overwrite)" }
    async fn execute(&self, _ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let WriteIn { path, content, append } = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("file.write input invalid: {e}")))?;
        if let Some(parent) = path.parent() {
            let _ = tokio::fs::create_dir_all(parent).await;
        }
        if append {
            use tokio::io::AsyncWriteExt;
            let mut f = tokio::fs::OpenOptions::new().create(true).append(true).open(&path).await
                .map_err(|e| StepError::msg(format!("open append {}: {e}", path.display())))?;
            f.write_all(content.as_bytes()).await
                .map_err(|e| StepError::msg(format!("write {}: {e}", path.display())))?;
        } else {
            tokio::fs::write(&path, content.as_bytes()).await
                .map_err(|e| StepError::msg(format!("write {}: {e}", path.display())))?;
        }
        Ok(ActionResult::from(serde_json::json!({ "path": path })))
    }
}

pub struct ExistsAction;
#[derive(Deserialize)]
struct ExistsIn { path: PathBuf }

#[async_trait]
impl Action for ExistsAction {
    fn id(&self) -> &'static str { "file.exists" }
    fn summary(&self) -> &'static str { "Test whether a path exists" }
    async fn execute(&self, _ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let ExistsIn { path } = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("file.exists input invalid: {e}")))?;
        Ok(ActionResult::from(Value::Bool(tokio::fs::try_exists(&path).await.unwrap_or(false))))
    }
}
