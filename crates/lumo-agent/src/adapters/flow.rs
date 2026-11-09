use std::{path::Path, sync::Arc, time::Duration, time::Instant};

use async_trait::async_trait;
use lumo_core::{CancelToken, FlowVm, RunOptions, RunReport};
use lumo_dsl::Flow;

use super::{
    CapabilityKind, InvocationAdapter, InvocationContext, InvocationError, InvocationRequest,
    InvocationResult,
};
use crate::CapabilitySource;

pub type FlowVmFactory = Arc<dyn Fn() -> FlowVm + Send + Sync>;
/// Build the runtime using the invoked document's settings and directory.
pub type ContextualFlowVmFactory = Arc<dyn Fn(&Flow, Option<&Path>) -> FlowVm + Send + Sync>;

/// An outer agent deadline or task abort may drop the adapter at any await.
/// Request cooperative cancellation while leaving the VM task alive to run
/// its teardown hooks and persist the terminal state.
struct CancelOnDrop(CancelToken);

impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        self.0.cancel();
    }
}

pub(super) async fn run_vm(
    vm: FlowVm,
    flow: Flow,
    options: RunOptions,
    context: &InvocationContext,
    timeout_ms: u64,
) -> Result<RunReport, InvocationError> {
    // A private token prevents a single timed-out node from cancelling its
    // siblings or making a retry inherit an already-cancelled parent token.
    let cancel = CancelToken::new();
    let _cancel_on_drop = CancelOnDrop(cancel.clone());
    let task = tokio::spawn(async move { vm.with_cancel(cancel).run(&flow, options).await });
    tokio::select! {
        biased;
        _ = context.cancel.cancelled() => Err(InvocationError::Cancelled),
        result = tokio::time::timeout(Duration::from_millis(timeout_ms), task) => {
            match result {
                Ok(Ok(result)) => result.map_err(|error| InvocationError::Failed(error.to_string())),
                Ok(Err(error)) => Err(InvocationError::Failed(format!("flow task failed: {error}"))),
                Err(_) => Err(InvocationError::Timeout { duration_ms: timeout_ms }),
            }
        }
    }
}

pub(super) fn invocation_result(report: RunReport, started: Instant) -> InvocationResult {
    let mut result = InvocationResult::new(report.outputs.unwrap_or_default(), started);
    result.tokens_used = report.tokens_used;
    result.cost_usd_micro = report.cost_usd_micro;
    result
        .metadata
        .insert("flowRunId".into(), report.run_id.into());
    result
}

pub struct FlowAdapter {
    vm_factory: ContextualFlowVmFactory,
}

impl FlowAdapter {
    pub fn new(vm_factory: FlowVmFactory) -> Self {
        Self::new_contextual(Arc::new(move |_, _| vm_factory()))
    }

    pub fn new_contextual(vm_factory: ContextualFlowVmFactory) -> Self {
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
        let content =
            std::fs::read(path).map_err(|error| InvocationError::Unavailable(error.to_string()))?;
        if !request.capability.source_content_matches(&content) {
            return Err(InvocationError::Unavailable(
                "flow source changed after planning; create a new plan".into(),
            ));
        }
        let text = std::str::from_utf8(&content)
            .map_err(|error| InvocationError::Unavailable(error.to_string()))?;
        let flow = lumo_dsl::parse_str(text)
            .map_err(|error| InvocationError::Unavailable(error.to_string()))?;
        lumo_dsl::validate(&flow)
            .map_err(|error| InvocationError::Unavailable(error.to_string()))?;
        let started = Instant::now();
        let report = run_vm(
            (self.vm_factory)(&flow, Some(Path::new(path))),
            flow,
            RunOptions {
                inputs: request.arguments,
                trigger_kind: "agent:flow".into(),
            },
            &context,
            request.timeout_ms,
        )
        .await?;
        if !report.success {
            return Err(InvocationError::Failed(
                "flow completed unsuccessfully".into(),
            ));
        }
        Ok(invocation_result(report, started))
    }
}
