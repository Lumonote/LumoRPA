use std::collections::{BTreeMap, BTreeSet, VecDeque};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::RiskLevel;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentPlan {
    pub id: String,
    pub objective: String,
    pub nodes: Vec<PlanNode>,
    #[serde(default)]
    pub metadata: BTreeMap<String, Value>,
}

impl AgentPlan {
    pub fn new(
        id: impl Into<String>,
        objective: impl Into<String>,
        nodes: Vec<PlanNode>,
    ) -> Self {
        Self {
            id: id.into(),
            objective: objective.into(),
            nodes,
            metadata: BTreeMap::new(),
        }
    }

    pub fn stable_hash(&self) -> String {
        stable_json_hash(&serde_json::to_value(self).expect("AgentPlan is serializable"))
    }

    pub fn node(&self, id: &str) -> Option<&PlanNode> {
        self.nodes.iter().find(|node| node.id == id)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanNode {
    pub id: String,
    #[serde(default)]
    pub depends_on: Vec<String>,
    pub capability_id: String,
    #[serde(default)]
    pub arguments: Value,
    pub risk: RiskLevel,
    pub timeout_ms: u64,
    pub retry_limit: u32,
    #[serde(default)]
    pub expected_output_schema: Option<Value>,
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum PlanError {
    #[error("plan id must not be empty")]
    EmptyPlanId,
    #[error("plan contains no nodes")]
    EmptyPlan,
    #[error("plan node id must not be empty")]
    EmptyNodeId,
    #[error("duplicate plan node `{0}`")]
    DuplicateNode(String),
    #[error("node `{node_id}` depends on unknown node `{dependency}`")]
    UnknownDependency { node_id: String, dependency: String },
    #[error("plan contains a dependency cycle involving {0:?}")]
    Cycle(Vec<String>),
    #[error("node `{0}` must have a positive timeout")]
    InvalidTimeout(String),
}

pub fn validate_dag(plan: &AgentPlan) -> Result<(), PlanError> {
    if plan.id.trim().is_empty() {
        return Err(PlanError::EmptyPlanId);
    }
    if plan.nodes.is_empty() {
        return Err(PlanError::EmptyPlan);
    }

    let mut nodes = BTreeMap::new();
    for node in &plan.nodes {
        if node.id.trim().is_empty() {
            return Err(PlanError::EmptyNodeId);
        }
        if node.timeout_ms == 0 {
            return Err(PlanError::InvalidTimeout(node.id.clone()));
        }
        if nodes.insert(node.id.as_str(), node).is_some() {
            return Err(PlanError::DuplicateNode(node.id.clone()));
        }
    }

    let mut indegree = BTreeMap::<&str, usize>::new();
    let mut outgoing = BTreeMap::<&str, Vec<&str>>::new();
    for node in &plan.nodes {
        let mut unique_dependencies = BTreeSet::new();
        for dependency in &node.depends_on {
            if !nodes.contains_key(dependency.as_str()) {
                return Err(PlanError::UnknownDependency {
                    node_id: node.id.clone(),
                    dependency: dependency.clone(),
                });
            }
            if unique_dependencies.insert(dependency.as_str()) {
                *indegree.entry(node.id.as_str()).or_default() += 1;
                outgoing
                    .entry(dependency.as_str())
                    .or_default()
                    .push(node.id.as_str());
            }
        }
        indegree.entry(node.id.as_str()).or_default();
    }

    let mut ready = indegree
        .iter()
        .filter_map(|(id, degree)| (*degree == 0).then_some(*id))
        .collect::<VecDeque<_>>();
    let mut visited = 0;
    while let Some(id) = ready.pop_front() {
        visited += 1;
        if let Some(children) = outgoing.get(id) {
            for child in children {
                let degree = indegree.get_mut(child).expect("child was indexed");
                *degree -= 1;
                if *degree == 0 {
                    ready.push_back(child);
                }
            }
        }
    }

    if visited != plan.nodes.len() {
        let cyclic = indegree
            .into_iter()
            .filter_map(|(id, degree)| (degree > 0).then_some(id.to_string()))
            .collect();
        return Err(PlanError::Cycle(cyclic));
    }
    Ok(())
}

pub(crate) fn stable_json_hash(value: &Value) -> String {
    fn canonicalize(value: &Value) -> Value {
        match value {
            Value::Array(values) => values.iter().map(canonicalize).collect(),
            Value::Object(values) => {
                let mut entries = values.iter().collect::<Vec<_>>();
                entries.sort_unstable_by_key(|(key, _)| *key);
                Value::Object(
                    entries
                        .into_iter()
                        .map(|(key, value)| (key.clone(), canonicalize(value)))
                        .collect(),
                )
            }
            scalar => scalar.clone(),
        }
    }

    let bytes = serde_json::to_vec(&canonicalize(value)).expect("JSON serialization cannot fail");
    format!("{:x}", Sha256::digest(bytes))
}
