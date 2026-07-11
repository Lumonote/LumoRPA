use std::collections::BTreeSet;

use serde_json::Value;
use thiserror::Error;

use crate::{
    evaluate_node, validate_dag, AgentPlan, AgentProfile, CapabilityCatalog, InvocationResult,
    PlanError, PlanNode, PolicyDecision,
};

#[derive(Debug, Clone)]
pub struct ValidatedPlan {
    pub plan: AgentPlan,
    pub approval_required: BTreeSet<String>,
}

pub struct PlanValidator<'a> {
    catalog: &'a CapabilityCatalog,
    profile: &'a AgentProfile,
}

impl<'a> PlanValidator<'a> {
    pub fn new(catalog: &'a CapabilityCatalog, profile: &'a AgentProfile) -> Self {
        Self { catalog, profile }
    }

    pub fn validate(&self, plan: AgentPlan) -> Result<ValidatedPlan, PlanValidationError> {
        validate_dag(&plan)?;
        let mut approval_required = BTreeSet::new();
        for node in &plan.nodes {
            let capability = self
                .catalog
                .get(&node.capability_id)
                .ok_or_else(|| PlanValidationError::UnknownCapability(node.capability_id.clone()))?;
            if !capability.enabled
                || (!self.profile.visible_capabilities.is_empty()
                    && !self.profile.visible_capabilities.contains(&capability.id))
            {
                return Err(PlanValidationError::InvisibleCapability(capability.id.clone()));
            }
            validate_json_schema(&capability.input_schema, &node.arguments).map_err(|reason| {
                PlanValidationError::InvalidArguments {
                    node_id: node.id.clone(),
                    reason,
                }
            })?;
            match evaluate_node(node, &capability, self.profile) {
                PolicyDecision::Allow => {}
                PolicyDecision::RequireApproval { .. } => {
                    approval_required.insert(node.id.clone());
                }
                PolicyDecision::Deny { reason } => {
                    return Err(PlanValidationError::PolicyDenied {
                        node_id: node.id.clone(),
                        reason,
                    });
                }
            }
        }
        Ok(ValidatedPlan {
            plan,
            approval_required,
        })
    }
}

pub fn validate_invocation_result(
    node: &PlanNode,
    result: &InvocationResult,
) -> Result<(), PlanValidationError> {
    if let Some(schema) = &node.expected_output_schema {
        validate_json_schema(schema, &result.output).map_err(|reason| {
            PlanValidationError::InvalidOutput {
                node_id: node.id.clone(),
                reason,
            }
        })?;
    }
    Ok(())
}

pub fn validate_json_schema(schema: &Value, value: &Value) -> Result<(), String> {
    let Some(schema) = schema.as_object() else {
        return Err("schema must be an object".into());
    };
    if let Some(expected) = schema.get("type").and_then(Value::as_str) {
        let valid = match expected {
            "object" => value.is_object(),
            "array" => value.is_array(),
            "string" => value.is_string(),
            "number" => value.is_number(),
            "integer" => value.as_i64().is_some() || value.as_u64().is_some(),
            "boolean" => value.is_boolean(),
            "null" => value.is_null(),
            _ => return Err(format!("unsupported schema type `{expected}`")),
        };
        if !valid {
            return Err(format!("expected {expected}"));
        }
    }
    if let Some(required) = schema.get("required").and_then(Value::as_array) {
        let object = value
            .as_object()
            .ok_or_else(|| "required fields need an object".to_string())?;
        for field in required.iter().filter_map(Value::as_str) {
            if !object.contains_key(field) {
                return Err(format!("missing required field `{field}`"));
            }
        }
    }
    if let (Some(properties), Some(object)) = (
        schema.get("properties").and_then(Value::as_object),
        value.as_object(),
    ) {
        for (name, property_schema) in properties {
            if let Some(property) = object.get(name) {
                validate_json_schema(property_schema, property)
                    .map_err(|reason| format!("field `{name}`: {reason}"))?;
            }
        }
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum PlanValidationError {
    #[error(transparent)]
    Dag(#[from] PlanError),
    #[error("unknown capability `{0}`")]
    UnknownCapability(String),
    #[error("capability `{0}` is not visible to this profile")]
    InvisibleCapability(String),
    #[error("invalid arguments for node `{node_id}`: {reason}")]
    InvalidArguments { node_id: String, reason: String },
    #[error("invalid output for node `{node_id}`: {reason}")]
    InvalidOutput { node_id: String, reason: String },
    #[error("policy denied node `{node_id}`: {reason}")]
    PolicyDenied { node_id: String, reason: String },
}
