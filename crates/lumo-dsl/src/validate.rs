use crate::ast::{Flow, Step};
use std::collections::HashSet;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ValidationError {
    #[error("flow `{flow}` step id collision: `{id}`")]
    DuplicateStepId { flow: String, id: String },

    #[error("flow `{flow}` step `{id}` action is empty")]
    EmptyAction { flow: String, id: String },

    #[error(
        "flow `{flow}` step `{id}` has `do` but action `{action}` is not a control-flow action"
    )]
    StrayBlock {
        flow: String,
        id: String,
        action: String,
    },

    #[error("flow `{flow}` step `{id}` missing required `do` block for action `{action}`")]
    MissingDoBlock {
        flow: String,
        id: String,
        action: String,
    },

    #[error("flow `{flow}` step `{id}` enables AI but `spec.capabilities.llm` is empty")]
    AiMissingLlmCapability { flow: String, id: String },
}

/// A small set of action ids that may carry `do/else/catch/finally` children.
/// Kept in DSL to avoid a circular dep with `lumo-actions`.
pub fn is_control_action(id: &str) -> bool {
    matches!(
        id,
        "control.if"
            | "control.for"
            | "control.for_each"
            | "control.try"
            | "control.parallel"
            | "excel.for_each_row"
            | "browser.for_each"
    )
}

pub fn validate(flow: &Flow) -> Result<(), ValidationError> {
    let mut seen = HashSet::new();
    let flow_ai_enabled = flow.metadata.ai.as_ref().map(|a| a.enabled).unwrap_or(true);
    let has_llm_cap = !flow.spec.capabilities.llm.is_empty();
    walk(
        &flow.metadata.id,
        &flow.spec.steps,
        &mut seen,
        flow_ai_enabled,
        has_llm_cap,
    )?;
    Ok(())
}

fn walk(
    flow: &str,
    steps: &[Step],
    seen: &mut HashSet<String>,
    flow_ai_enabled: bool,
    has_llm_cap: bool,
) -> Result<(), ValidationError> {
    for s in steps {
        if !seen.insert(s.id.clone()) {
            return Err(ValidationError::DuplicateStepId {
                flow: flow.into(),
                id: s.id.clone(),
            });
        }
        if s.action.trim().is_empty() {
            return Err(ValidationError::EmptyAction {
                flow: flow.into(),
                id: s.id.clone(),
            });
        }
        let has_block = s.do_.is_some()
            || s.else_.is_some()
            || s.catch_.is_some()
            || s.finally_.is_some()
            || s.branches.is_some();
        if has_block && !is_control_action(&s.action) {
            return Err(ValidationError::StrayBlock {
                flow: flow.into(),
                id: s.id.clone(),
                action: s.action.clone(),
            });
        }
        if s.branches.is_some() && s.action != "control.parallel" {
            return Err(ValidationError::StrayBlock {
                flow: flow.into(),
                id: s.id.clone(),
                action: s.action.clone(),
            });
        }
        // control.parallel accepts either `branches:` (multi-step branches) or
        // `do:` (each top-level step becomes a one-step branch).
        let parallel_has_body = s.do_.is_some() || s.branches.is_some();
        if matches!(
            s.action.as_str(),
            "control.for"
                | "control.for_each"
                | "control.try"
                | "excel.for_each_row"
                | "browser.for_each"
        ) && s.do_.is_none()
        {
            return Err(ValidationError::MissingDoBlock {
                flow: flow.into(),
                id: s.id.clone(),
                action: s.action.clone(),
            });
        }
        if s.action == "control.parallel" && !parallel_has_body {
            return Err(ValidationError::MissingDoBlock {
                flow: flow.into(),
                id: s.id.clone(),
                action: s.action.clone(),
            });
        }
        // AI enablement requires LLM capability when the flow-level master is on.
        if flow_ai_enabled {
            if let Some(ai) = &s.ai {
                if ai.is_enabled() && !has_llm_cap {
                    return Err(ValidationError::AiMissingLlmCapability {
                        flow: flow.into(),
                        id: s.id.clone(),
                    });
                }
            }
        }
        for child in s.children() {
            walk(flow, child, seen, flow_ai_enabled, has_llm_cap)?;
        }
    }
    Ok(())
}
