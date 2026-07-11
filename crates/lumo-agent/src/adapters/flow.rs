use std::{sync::Arc, time::Duration, time::Instant};

use async_trait::async_trait;
use lumo_core::{FlowVm, RunOptions};

use super::{
    CapabilityKind, InvocationAdapter, InvocationContext, InvocationError, InvocationRequest,
    InvocationResult,
};
use crate::CapabilitySource;

pub type FlowVmFactory = Arc<dyn Fn() -> FlowVm + Send + Sync>;

pub struct FlowAdapter {
    vm_factory: FlowVmFactory,
}

impl FlowAdapter {
    pub fn new(vm_factory: FlowVmFactory) -> Self {
        Self { vm_factory }
    }
}

#[async_trait]
impl InvocationAdapter for FlowAdapter {
    fn source_kind(&self) -> CapabilityKind {
        CapabilityKind::Flow
    }

    async fn invoke(
        &self,
        request: InvocationRequest,
        context: InvocationContext,
    ) -> Result<InvocationResult, InvocationError> {
        let CapabilitySource::Flow { path } = &request.capability.source else {
            return Err(InvocationError::InvalidSource);
        };
        let flow = lumo_dsl::parse_file(path)
            .map_err(|error| InvocationError::Unavailable(error.to_string()))?;
        lumo_dsl::validate(&flow)
            .map_err(|error| InvocationError::Unavailable(error.to_string()))?;
        let started = Instant::now();
        let vm = (self.vm_factory)().with_cancel(context.cancel.clone());
        let run = vm.run(
            &flow,
            RunOptions {
                inputs: request.arguments,
                trigger_kind: "agent:flow".into(),
            },
        );
        let report = tokio::select! {
            _ = context.cancel.cancelled() => return Err(InvocationError::Cancelled),
            result = tokio::time::timeout(Duration::from_millis(request.timeout_ms), run) => {
                match result {
                    Ok(result) => result.map_err(|error| InvocationError::Failed(error.to_string()))?,
                    Err(_) => return Err(InvocationError::Timeout { duration_ms: request.timeout_ms }),
                }
            }
        };
        if !report.success {
            return Err(InvocationError::Failed("flow completed unsuccessfully".into()));
        }
        Ok(InvocationResult::new(
            report.outputs.unwrap_or_default(),
            started,
        ))
    }
}
