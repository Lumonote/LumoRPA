use std::{sync::Arc, time::Duration, time::Instant};

use async_trait::async_trait;
use lumo_core::RunOptions;
use lumo_skills::SkillRegistry;

use super::{
    CapabilityKind, FlowVmFactory, InvocationAdapter, InvocationContext, InvocationError,
    InvocationRequest, InvocationResult,
};
use crate::CapabilitySource;

pub struct SkillAdapter {
    skills: Arc<SkillRegistry>,
    vm_factory: FlowVmFactory,
}

impl SkillAdapter {
    pub fn new(skills: Arc<SkillRegistry>, vm_factory: FlowVmFactory) -> Self {
        Self { skills, vm_factory }
    }
}

#[async_trait]
impl InvocationAdapter for SkillAdapter {
    fn source_kind(&self) -> CapabilityKind {
        CapabilityKind::Skill
    }

    async fn invoke(
        &self,
        request: InvocationRequest,
        context: InvocationContext,
    ) -> Result<InvocationResult, InvocationError> {
        let CapabilitySource::Skill { name, .. } = &request.capability.source else {
            return Err(InvocationError::InvalidSource);
        };
        let skill = self
            .skills
            .get(name)
            .ok_or_else(|| InvocationError::Unavailable(format!("unknown skill `{name}`")))?;
        let started = Instant::now();
        // The host factory is responsible for applying the active Agent
        // Profile's capability clamp; cancellation is always shared here.
        let vm = (self.vm_factory)().with_cancel(context.cancel.clone());
        let run = vm.run(
            &skill.flow,
            RunOptions {
                inputs: request.arguments,
                trigger_kind: format!("agent:skill:{name}"),
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
            return Err(InvocationError::Failed("skill completed unsuccessfully".into()));
        }
        Ok(InvocationResult::new(
            report.outputs.unwrap_or_default(),
            started,
        ))
    }
}
