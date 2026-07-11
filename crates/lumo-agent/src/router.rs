use std::sync::Arc;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{
    AgentPlan, AgentProfile, CapabilityCatalog, CapabilityDescriptor, PlanNode, PlannerBackend,
    RankedCandidate,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SystemControlIntent {
    Cancel,
    Pause,
    Resume,
    StopSpeaking,
    OpenMissionControl,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum RouteOutcome {
    Control(SystemControlIntent),
    Plan(AgentPlan),
    Clarify {
        question: String,
        candidate_ids: Vec<String>,
    },
}

pub struct Router {
    catalog: CapabilityCatalog,
    profile: AgentProfile,
    planner: Arc<dyn PlannerBackend>,
}

impl Router {
    pub fn new(
        catalog: CapabilityCatalog,
        profile: AgentProfile,
        planner: Arc<dyn PlannerBackend>,
    ) -> Self {
        Self { catalog, profile, planner }
    }

    pub async fn route(&self, utterance: &str) -> Result<RouteOutcome, String> {
        if let Some(control) = control_intent(utterance) {
            return Ok(RouteOutcome::Control(control));
        }

        let exact = self
            .catalog
            .exact_alias(utterance)
            .into_iter()
            .filter(|capability| self.is_visible(capability))
            .collect::<Vec<_>>();
        if exact.len() == 1 {
            return Ok(RouteOutcome::Plan(single_node_plan(utterance, &exact[0])));
        }
        if exact.len() > 1 {
            return Ok(clarify(exact.iter().map(|capability| capability.id.clone()).collect()));
        }

        let mut ranked = self
            .catalog
            .all()
            .into_iter()
            .filter(|capability| self.is_visible(capability))
            .map(|capability| RankedCandidate {
                score: local_score(utterance, &capability),
                capability_id: capability.id.clone(),
            })
            .collect::<Vec<_>>();
        ranked.sort_by(|left, right| {
            right
                .score
                .total_cmp(&left.score)
                .then_with(|| left.capability_id.cmp(&right.capability_id))
        });

        if let Some(top) = ranked.first() {
            let gap = top.score - ranked.get(1).map(|candidate| candidate.score).unwrap_or(0.0);
            if top.score >= 0.75 && gap >= 0.15 {
                if let Some(capability) = self.catalog.get(&top.capability_id) {
                    return Ok(RouteOutcome::Plan(single_node_plan(utterance, &capability)));
                }
            }
            if top.score >= 0.5 && ranked.get(1).is_some_and(|next| top.score - next.score < 0.15) {
                return Ok(clarify(
                    ranked.iter().take(2).map(|candidate| candidate.capability_id.clone()).collect(),
                ));
            }
        }

        if ranked.is_empty() {
            return Ok(RouteOutcome::Clarify {
                question: "No available capability matches this request.".into(),
                candidate_ids: vec![],
            });
        }
        self.planner
            .plan(utterance, ranked.into_iter().take(12).collect())
            .await
            .map(RouteOutcome::Plan)
    }

    fn is_visible(&self, capability: &CapabilityDescriptor) -> bool {
        capability.enabled
            && (self.profile.visible_capabilities.is_empty()
                || self.profile.visible_capabilities.contains(&capability.id))
    }
}

fn single_node_plan(utterance: &str, capability: &CapabilityDescriptor) -> AgentPlan {
    let id = format!("route-{:x}", Sha256::digest(utterance.as_bytes()));
    AgentPlan::new(
        id,
        utterance,
        vec![PlanNode {
            id: "execute".into(),
            depends_on: vec![],
            capability_id: capability.id.clone(),
            arguments: serde_json::json!({}),
            risk: capability.risk,
            timeout_ms: 30_000,
            retry_limit: 1,
            expected_output_schema: capability.output_schema.clone(),
        }],
    )
}

fn clarify(candidate_ids: Vec<String>) -> RouteOutcome {
    RouteOutcome::Clarify {
        question: "Which capability did you mean?".into(),
        candidate_ids,
    }
}

fn control_intent(utterance: &str) -> Option<SystemControlIntent> {
    match utterance.trim().to_lowercase().as_str() {
        "cancel" | "stop" | "取消" | "停止任务" => Some(SystemControlIntent::Cancel),
        "pause" | "暂停" => Some(SystemControlIntent::Pause),
        "resume" | "continue" | "继续" => Some(SystemControlIntent::Resume),
        "stop speaking" | "别说了" => Some(SystemControlIntent::StopSpeaking),
        "open mission control" | "打开任务中心" => Some(SystemControlIntent::OpenMissionControl),
        _ => None,
    }
}

fn local_score(utterance: &str, capability: &CapabilityDescriptor) -> f64 {
    let utterance = utterance.trim().to_lowercase();
    let mut score = 0.0_f64;
    for phrase in std::iter::once(&capability.name)
        .chain(capability.aliases.iter())
        .chain(capability.examples.iter())
    {
        let phrase = phrase.trim().to_lowercase();
        if !phrase.is_empty() && utterance.contains(&phrase) {
            score = score.max(if utterance == phrase { 1.0 } else { 0.85 });
        }
    }
    let description_words = capability
        .description
        .split_whitespace()
        .filter(|word| utterance.contains(&word.to_lowercase()))
        .count();
    if description_words > 0 {
        score = score.max((description_words as f64 * 0.15).min(0.6));
    }
    score
}
