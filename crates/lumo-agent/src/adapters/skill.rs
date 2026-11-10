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
        let snapshot = super::flow::source_snapshot(&context)?;
        let (source, content) = if let Some(snapshot) = &snapshot {
            let (source, content) = snapshot
                .skill_source(name)
                .map_err(InvocationError::Unavailable)?;
            (source.to_path_buf(), content.as_bytes().to_vec())
        } else {
            let skill = self
                .skills
                .get(name)
                .ok_or_else(|| InvocationError::Unavailable(format!("unknown skill `{name}`")))?;
            let content = std::fs::read(&skill.source)
                .map_err(|error| InvocationError::Unavailable(error.to_string()))?;
            (skill.source.clone(), content)
        };
        if !request.capability.source_content_matches(&content) {
            return Err(InvocationError::Unavailable(
                "skill source changed after planning; create a new plan".into(),
            ));
        }
        let text = std::str::from_utf8(&content)
            .map_err(|error| InvocationError::Unavailable(error.to_string()))?;
        let skill = lumo_skills::loader::load_skill_text(&source, text)
            .map_err(|error| InvocationError::Unavailable(error.to_string()))?;
        if skill.name() != name {
            return Err(InvocationError::Unavailable(
                "skill name changed after planning".into(),
            ));
        }
        let started = Instant::now();
        // The host factory applies the active Agent Profile's capability
        // clamp; run_vm preserves cleanup on cancellation or an outer abort.
        let report = run_vm(
            (self.vm_factory)(&skill.flow, Some(&skill.source)).with_source_snapshot(snapshot),
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
        Ok(super::flow::invocation_result(report, started))
    }
}
