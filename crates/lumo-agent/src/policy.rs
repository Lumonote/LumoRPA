use std::collections::{BTreeMap, BTreeSet};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::{
    plan::stable_json_hash, AgentPlan, AgentProfile, CapabilityDescriptor, PermissionDecision,
    PlanNode, RiskLevel,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ApprovalStrength {
    Standard,
    Strengthened,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "decision", rename_all = "camelCase")]
pub enum PolicyDecision {
    Allow,
    RequireApproval {
        strength: ApprovalStrength,
        reason: String,
    },
    Deny {
        reason: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ApprovalSnapshot {
    pub plan_hash: String,
    pub capability_versions: BTreeMap<String, String>,
    pub schema_hashes: BTreeMap<String, String>,
    pub approved_nodes: BTreeSet<String>,
    pub approved_at: DateTime<Utc>,
}

impl ApprovalSnapshot {
    pub fn capture(
        plan: &AgentPlan,
        capabilities: &[CapabilityDescriptor],
        approved_nodes: BTreeSet<String>,
    ) -> Self {
        let capability_versions = capabilities
            .iter()
            .map(|capability| (capability.id.clone(), capability.version_hash.clone()))
            .collect();
        let schema_hashes = capabilities
            .iter()
            .map(|capability| {
                (
                    capability.id.clone(),
                    stable_json_hash(&capability.input_schema),
                )
            })
            .collect();
        Self {
            plan_hash: plan.stable_hash(),
            capability_versions,
            schema_hashes,
            approved_nodes,
            approved_at: Utc::now(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "decision", rename_all = "camelCase")]
pub enum ReplanDecision {
    Preserve,
    RequiresApproval { node_ids: BTreeSet<String> },
    Reject { reason: String },
}

pub fn evaluate_node(
    node: &PlanNode,
    capability: &CapabilityDescriptor,
    profile: &AgentProfile,
) -> PolicyDecision {
    if !capability.enabled {
        return PolicyDecision::Deny {
            reason: format!("capability `{}` is disabled", capability.id),
        };
    }
    if node.capability_id != capability.id {
        return PolicyDecision::Deny {
            reason: "plan node does not match the resolved capability".into(),
        };
    }
    if node.risk < capability.risk {
        return PolicyDecision::Deny {
            reason: format!(
                "node declares {:?}, below capability risk {:?}",
                node.risk, capability.risk
            ),
        };
    }

    let rule = profile.permission_rules.iter().rev().find(|rule| {
        node.risk <= rule.max_risk && pattern_matches(&rule.capability_pattern, &capability.id)
    });
    if matches!(rule.map(|rule| rule.decision), Some(PermissionDecision::Deny)) {
        return PolicyDecision::Deny {
            reason: "denied by agent profile permission rule".into(),
        };
    }

    match node.risk {
        RiskLevel::L3 => PolicyDecision::RequireApproval {
            strength: ApprovalStrength::Strengthened,
            reason: "L3 capabilities require strengthened confirmation".into(),
        },
        RiskLevel::L2 => PolicyDecision::RequireApproval {
            strength: ApprovalStrength::Standard,
            reason: "L2 capabilities require confirmation".into(),
        },
        RiskLevel::L0 | RiskLevel::L1 => {
            if matches!(rule.map(|rule| rule.decision), Some(PermissionDecision::Ask))
                || node.risk > profile.max_auto_risk
            {
                PolicyDecision::RequireApproval {
                    strength: ApprovalStrength::Standard,
                    reason: "agent profile requires confirmation".into(),
                }
            } else {
                PolicyDecision::Allow
            }
        }
    }
}

pub fn validate_replan(
    old: &AgentPlan,
    new: &AgentPlan,
    approval: &ApprovalSnapshot,
) -> ReplanDecision {
    if approval.plan_hash != old.stable_hash() {
        return ReplanDecision::Reject {
            reason: "approval snapshot does not match the prior plan".into(),
        };
    }

    let old_nodes = old
        .nodes
        .iter()
        .map(|node| (node.id.as_str(), node))
        .collect::<BTreeMap<_, _>>();
    let mut requires_approval = BTreeSet::new();
    for node in &new.nodes {
        match old_nodes.get(node.id.as_str()) {
            Some(previous) => {
                if node.risk < previous.risk {
                    return ReplanDecision::Reject {
                        reason: format!(
                            "replacement for node `{}` lowers risk from {:?} to {:?}",
                            node.id, previous.risk, node.risk
                        ),
                    };
                }
                let materially_changed = node.capability_id != previous.capability_id
                    || node.arguments != previous.arguments
                    || node.risk != previous.risk
                    || node.timeout_ms > previous.timeout_ms
                    || node.depends_on != previous.depends_on;
                if materially_changed || !approval.approved_nodes.contains(&node.id) {
                    requires_approval.insert(node.id.clone());
                }
            }
            None => {
                requires_approval.insert(node.id.clone());
            }
        }
    }

    if requires_approval.is_empty() {
        ReplanDecision::Preserve
    } else {
        ReplanDecision::RequiresApproval {
            node_ids: requires_approval,
        }
    }
}

fn pattern_matches(pattern: &str, value: &str) -> bool {
    if pattern == "*" || pattern == value {
        return true;
    }
    match pattern.split_once('*') {
        Some((prefix, suffix)) => value.starts_with(prefix) && value.ends_with(suffix),
        None => false,
    }
}
