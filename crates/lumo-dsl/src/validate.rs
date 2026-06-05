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

    #[error(
        "flow `{flow}` step `{id}` references undeclared resource `{resource}` (not in spec.resources)"
    )]
    UnknownResource {
        flow: String,
        id: String,
        resource: String,
    },
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
    let resources: HashSet<&str> = flow.spec.resources.keys().map(String::as_str).collect();
    walk(
        &flow.metadata.id,
        &flow.spec.steps,
        &mut seen,
        flow_ai_enabled,
        has_llm_cap,
        &resources,
    )?;
    Ok(())
}

fn walk(
    flow: &str,
    steps: &[Step],
    seen: &mut HashSet<String>,
    flow_ai_enabled: bool,
    has_llm_cap: bool,
    resources: &HashSet<&str>,
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
        // T3: a per-step resource reference must resolve to a declared
        // `spec.resources` entry — a typo'd handle could never be opened.
        if let Some(res) = &s.resource {
            if !resources.contains(res.as_str()) {
                return Err(ValidationError::UnknownResource {
                    flow: flow.into(),
                    id: s.id.clone(),
                    resource: res.clone(),
                });
            }
        }
        for child in s.children() {
            walk(flow, child, seen, flow_ai_enabled, has_llm_cap, resources)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse_str;

    #[test]
    fn unknown_resource_ref_is_rejected() {
        let y = r#"
apiVersion: lumorpa.io/v1
kind: Flow
metadata: { id: t, version: 0.1.0 }
spec:
  steps:
    - id: q
      action: db.query
      resource: ghost
"#;
        let flow = parse_str(y).expect("parses (parse and validate are independent)");
        let err = validate(&flow).expect_err("undeclared resource ref must be rejected");
        assert!(
            matches!(err, ValidationError::UnknownResource { .. }),
            "got: {err}"
        );
    }

    #[test]
    fn declared_resource_ref_is_accepted() {
        let y = r#"
apiVersion: lumorpa.io/v1
kind: Flow
metadata: { id: t, version: 0.1.0 }
spec:
  resources:
    db:
      kind: sqlite
      url: "/tmp/x.db"
  steps:
    - id: q
      action: db.query
      resource: db
"#;
        let flow = parse_str(y).expect("parses");
        assert_eq!(flow.spec.resources.len(), 1);
        assert_eq!(flow.spec.resources["db"].kind, "sqlite");
        validate(&flow).expect("a declared resource ref must validate");
    }

    #[test]
    fn resource_ref_in_nested_block_is_validated() {
        // The check must recurse into control-flow children, not just top level.
        let y = r#"
apiVersion: lumorpa.io/v1
kind: Flow
metadata: { id: t, version: 0.1.0 }
spec:
  steps:
    - id: loop
      action: control.for_each
      with: { items: [1, 2] }
      do:
        - id: inner
          action: db.query
          resource: ghost
"#;
        let flow = parse_str(y).expect("parses");
        match validate(&flow).expect_err("nested undeclared resource ref must be rejected") {
            ValidationError::UnknownResource { resource, id, .. } => {
                assert_eq!(resource, "ghost");
                assert_eq!(id, "inner", "the failing step should be the nested one");
            }
            other => panic!("expected UnknownResource, got: {other}"),
        }
    }

    #[test]
    fn resource_decl_flattens_kind_specific_config() {
        let y = r#"
apiVersion: lumorpa.io/v1
kind: Flow
metadata: { id: t, version: 0.1.0 }
spec:
  resources:
    browser:
      kind: chromium.cdp
      profile: stealth-default
      headless: true
  steps:
    - id: a
      action: control.log
      with: { message: x }
"#;
        let flow = parse_str(y).expect("parses");
        let r = &flow.spec.resources["browser"];
        assert_eq!(r.kind, "chromium.cdp");
        assert_eq!(r.profile.as_deref(), Some("stealth-default"));
        // kind-specific `headless` flattens into `config`, not a hard struct field.
        assert_eq!(
            r.config.get("headless").and_then(|v| v.as_bool()),
            Some(true)
        );
    }
}
