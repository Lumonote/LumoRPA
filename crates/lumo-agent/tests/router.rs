use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use lumo_agent::{
    AgentPlan, AgentProfile, AgentProfileDraft, CapabilityCatalog, CapabilityDescriptor,
    CapabilitySource, PlanNode, PlannerBackend, RankedCandidate, RiskLevel, RouteOutcome, Router,
    SystemControlIntent,
};
use serde_json::json;

fn capability(id: &str, alias: &str) -> CapabilityDescriptor {
    let mut descriptor = CapabilityDescriptor {
        id: id.into(),
        source: CapabilitySource::Flow { path: format!("/{id}") },
        name: id.into(),
        description: format!("perform {alias}"),
        input_schema: json!({"type": "object"}),
        output_schema: None,
        aliases: vec![alias.into()],
        examples: vec![alias.into()],
        risk: RiskLevel::L0,
        enabled: true,
        version_hash: String::new(),
    };
    descriptor.refresh_version_hash();
    descriptor
}

struct FakePlanner {
    seen: Mutex<Vec<String>>,
}

#[async_trait]
impl PlannerBackend for FakePlanner {
    async fn plan(
        &self,
        utterance: &str,
        candidates: Vec<RankedCandidate>,
    ) -> Result<AgentPlan, String> {
        self.seen.lock().unwrap().extend(candidates.iter().map(|c| c.capability_id.clone()));
        let id = candidates.first().unwrap().capability_id.clone();
        Ok(AgentPlan::new(
            "ai-plan",
            utterance,
            vec![PlanNode {
                id: "execute".into(),
                depends_on: vec![],
                capability_id: id,
                arguments: json!({}),
                risk: RiskLevel::L0,
                timeout_ms: 1_000,
                retry_limit: 0,
                expected_output_schema: None,
            }],
        ))
    }
}

fn router(capabilities: Vec<CapabilityDescriptor>, planner: Arc<FakePlanner>) -> Router {
    let catalog = CapabilityCatalog::new(capabilities).unwrap();
    let profile = AgentProfile::validate(AgentProfileDraft::default(), &catalog).unwrap();
    Router::new(catalog, profile, planner)
}

#[tokio::test]
async fn control_precedes_alias_and_exact_alias_precedes_ai() {
    let planner = Arc::new(FakePlanner { seen: Mutex::new(vec![]) });
    let control_router = router(vec![capability("cancel-flow", "cancel")], planner.clone());
    assert_eq!(control_router.route("cancel").await.unwrap(), RouteOutcome::Control(SystemControlIntent::Cancel));

    let alias_router = router(vec![capability("orders", "查订单")], planner.clone());
    assert!(matches!(alias_router.route("查订单").await.unwrap(), RouteOutcome::Plan(_)));
    assert!(planner.seen.lock().unwrap().is_empty());
}

#[tokio::test]
async fn ambiguous_alias_clarifies_and_ai_only_sees_filtered_candidates() {
    let planner = Arc::new(FakePlanner { seen: Mutex::new(vec![]) });
    let ambiguous_router = router(
        vec![capability("a", "运行"), capability("b", "运行")],
        planner.clone(),
    );
    assert!(matches!(ambiguous_router.route("运行").await.unwrap(), RouteOutcome::Clarify { .. }));

    let mut disabled = capability("hidden", "secret");
    disabled.enabled = false;
    disabled.refresh_version_hash();
    let ai_router = router(vec![capability("visible", "orders"), disabled], planner.clone());
    assert!(matches!(ai_router.route("something unrelated").await.unwrap(), RouteOutcome::Plan(_)));
    assert_eq!(*planner.seen.lock().unwrap(), vec!["visible"]);
}
