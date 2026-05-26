use crate::ast::{Flow, Step};
use std::collections::HashSet;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ValidationError {
    #[error("flow `{flow}` step id collision: `{id}`")]
    DuplicateStepId { flow: String, id: String },

    #[error("flow `{flow}` step `{id}` action is empty")]
    EmptyAction { flow: String, id: String },

    #[error("flow `{flow}` step `{id}` has `do` but action `{action}` is not a control-flow action")]
    StrayBlock { flow: String, id: String, action: String },

    #[error("flow `{flow}` step `{id}` missing required `do` block for action `{action}`")]
    MissingDoBlock { flow: String, id: String, action: String },
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
    walk(&flow.metadata.id, &flow.spec.steps, &mut seen)?;
    Ok(())
}

fn walk(flow: &str, steps: &[Step], seen: &mut HashSet<String>) -> Result<(), ValidationError> {
    for s in steps {
        if !seen.insert(s.id.clone()) {
            return Err(ValidationError::DuplicateStepId { flow: flow.into(), id: s.id.clone() });
        }
        if s.action.trim().is_empty() {
            return Err(ValidationError::EmptyAction { flow: flow.into(), id: s.id.clone() });
        }
        let has_block = s.do_.is_some() || s.else_.is_some() || s.catch_.is_some() || s.finally_.is_some();
        if has_block && !is_control_action(&s.action) {
            return Err(ValidationError::StrayBlock {
                flow: flow.into(),
                id: s.id.clone(),
                action: s.action.clone(),
            });
        }
        if matches!(
            s.action.as_str(),
            "control.for" | "control.for_each" | "control.try" | "control.parallel"
                | "excel.for_each_row" | "browser.for_each"
        ) && s.do_.is_none()
        {
            return Err(ValidationError::MissingDoBlock {
                flow: flow.into(),
                id: s.id.clone(),
                action: s.action.clone(),
            });
        }
        for child in s.children() {
            walk(flow, child, seen)?;
        }
    }
    Ok(())
}
