use std::{sync::Arc, time::Duration, time::Instant};

use async_trait::async_trait;
use lumo_actions::mcp::{McpClient, McpTransportConfig};
use serde_json::Value;

use super::{
    CapabilityKind, InvocationAdapter, InvocationContext, InvocationError, InvocationRequest,
    InvocationResult,
};
use crate::CapabilitySource;

#[derive(Debug, Clone)]
pub struct McpConnectionProfile {
    pub transport: McpTransportConfig,
}

pub trait McpProfileResolver: Send + Sync {
    fn resolve(&self, server: &str) -> Result<McpConnectionProfile, InvocationError>;
}

#[async_trait]
pub trait McpToolInvoker: Send + Sync {
    async fn call(
        &self,
        server: &str,
        tool: &str,
        arguments: Value,
        context: &InvocationContext,
        timeout_ms: u64,
    ) -> Result<Value, InvocationError>;
}

pub struct McpClientInvoker {
    profiles: Arc<dyn McpProfileResolver>,
}

impl McpClientInvoker {
    pub fn new(profiles: Arc<dyn McpProfileResolver>) -> Self {
        Self { profiles }
    }
}

#[async_trait]
impl McpToolInvoker for McpClientInvoker {
    async fn call(
        &self,
        server: &str,
        tool: &str,
        arguments: Value,
        context: &InvocationContext,
        timeout_ms: u64,
    ) -> Result<Value, InvocationError> {
        let profile = self.profiles.resolve(server)?;
        let timeout = Duration::from_millis(timeout_ms);
        let mut client = McpClient::connect(profile.transport, timeout)
            .await
            .map_err(|error| InvocationError::Unavailable(error.to_string()))?;
        tokio::select! {
            _ = context.cancel.cancelled() => {
                client.close().await;
                Err(InvocationError::Cancelled)
            }
            result = client.call_tool(tool, arguments) => {
                let result = result.map_err(|error| InvocationError::Failed(error.to_string()));
                client.close().await;
                result
            }
        }
    }
}

pub struct McpAdapter {
    invoker: Arc<dyn McpToolInvoker>,
}

impl McpAdapter {
    pub fn new(invoker: Arc<dyn McpToolInvoker>) -> Self {
        Self { invoker }
    }
}

#[async_trait]
impl InvocationAdapter for McpAdapter {
    fn source_kind(&self) -> CapabilityKind {
        CapabilityKind::Mcp
    }

    async fn invoke(
        &self,
        request: InvocationRequest,
        context: InvocationContext,
    ) -> Result<InvocationResult, InvocationError> {
        let CapabilitySource::Mcp { server, tool } = &request.capability.source else {
            return Err(InvocationError::InvalidSource);
        };
        let started = Instant::now();
        let output = tokio::time::timeout(
            Duration::from_millis(request.timeout_ms),
            self.invoker.call(
                server,
                tool,
                request.arguments,
                &context,
                request.timeout_ms,
            ),
        )
        .await
        .map_err(|_| InvocationError::Timeout {
            duration_ms: request.timeout_ms,
        })??;
        Ok(InvocationResult::new(output, started))
    }
}
