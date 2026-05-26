//! LumoRPA recorder (placeholder skeleton for M2).
//!
//! In M1 we only expose the trait + a simple in-memory buffer to
//! demonstrate the integration surface. CDP/AccessKit hooks land in M2.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RawEvent {
    pub source: String,
    pub kind: String,
    pub at_ms: i64,
    pub payload: serde_json::Value,
}

#[async_trait]
pub trait Recorder: Send + Sync {
    async fn start(&self) -> anyhow::Result<()>;
    async fn stop(&self) -> anyhow::Result<Vec<RawEvent>>;
}

pub struct NoopRecorder;

#[async_trait]
impl Recorder for NoopRecorder {
    async fn start(&self) -> anyhow::Result<()> {
        tracing::info!("recorder: noop start (real impl in M2)");
        Ok(())
    }
    async fn stop(&self) -> anyhow::Result<Vec<RawEvent>> {
        Ok(Vec::new())
    }
}
