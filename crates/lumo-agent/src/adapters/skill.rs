use std::{sync::Arc, time::Instant};

use async_trait::async_trait;
use lumo_core::RunOptions;
use lumo_skills::SkillRegistry;

use super::{
    flow::run_vm, CapabilityKind, ContextualFlowVmFactory, FlowVmFactory, InvocationAdapter,
    InvocationContext, InvocationError, InvocationRequest, InvocationResult,
};
use crate::CapabilitySource;

pub struct SkillAdapter {
    skills: Arc<SkillRegistry>,
    vm_factory: ContextualFlowVmFactory,
}

impl SkillAdapter {
    pub fn new(skills: Arc<SkillRegistry>, vm_factory: FlowVmFactory) -> Self {
        Self::new_contextual(skills, Arc::new(move |_, _| vm_factory()))
    }

    pub fn new_contextual(skills: Arc<SkillRegistry>, vm_factory: ContextualFlowVmFactory) -> Self {
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
        // The host factory applies the active Agent Profile's capability
        // clamp; run_vm preserves cleanup on cancellation or an outer abort.
        let report = run_vm(
            (self.vm_factory)(&skill.flow, Some(&skill.source)),
            skill.flow.clone(),
            RunOptions {
                inputs: request.arguments,
                trigger_kind: format!("agent:skill:{name}"),
            },
            &context,
            request.timeout_ms,
        )
        .await?;
        if !report.success {
            return Err(InvocationError::Failed(
                "skill completed unsuccessfully".into(),
            ));
        }
        Ok(InvocationResult::new(
            report.outputs.unwrap_or_default(),
            started,
        ))
    }
}
