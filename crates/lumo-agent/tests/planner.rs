use std::sync::Arc;

use async_trait::async_trait;
use lumo_agent::{
    AgentProfile, AgentProfileDraft, AiPlanModel, CapabilityCatalog, CapabilityDescriptor,
    CapabilitySource, Planner, PlannerError, RankedCandidate, RiskLevel,
};
use serde_json::json;

struct Model(&'static str);

#[async_trait]
impl AiPlanModel for Model {
    async fn generate(
        &self,
        _utterance: &str,
        _candidates: &[RankedCandidate],
    ) -> Result<String, String> {
        Ok(self.0.into())
    }
}

fn fixture() -> (CapabilityCatalog, AgentProfile) {
    let mut descriptor = CapabilityDescriptor {
        id: "orders".into(),
        source: CapabilitySource::Flow {
            path: "/orders".into(),
        },
        name: "orders".into(),
        description: String::new(),
        input_schema: json!({"type": "object", "required": ["id"]}),
        output_schema: None,
        aliases: vec![],
        examples: vec![],
        risk: RiskLevel::L0,
        enabled: true,
        version_hash: String::new(),
    };
    descriptor.refresh_version_hash();
    let catalog = CapabilityCatalog::new(vec![descriptor]).unwrap();
    let profile = AgentProfile::validate(AgentProfileDraft::default(), &catalog).unwrap();
    (catalog, profile)
}

#[tokio::test]
async fn malformed_and_schema_invalid_model_plans_are_rejected() {
    let (catalog, profile) = fixture();
    let malformed = Planner::new(
        catalog.clone(),
        profile.clone(),
        Arc::new(Model("not-json")),
    );
    assert!(matches!(
        malformed.plan("orders", vec![]).await,
        Err(PlannerError::MalformedPlan(_))
    ));

    let invalid = r#"{"id":"p","objective":"orders","nodes":[{"id":"a","dependsOn":[],"capabilityId":"orders","arguments":{},"risk":"L0","timeoutMs":1000,"retryLimit":0,"expectedOutputSchema":null}],"metadata":{}}"#
        .replace("\\\"", "\"");
    struct OwnedModel(String);
    #[async_trait]
    impl AiPlanModel for OwnedModel {
        async fn generate(
            &self,
            _utterance: &str,
            _candidates: &[RankedCandidate],
        ) -> Result<String, String> {
            Ok(self.0.clone())
        }
    }
    let planner = Planner::new(catalog, profile, Arc::new(OwnedModel(invalid)));
    assert!(matches!(
        planner.plan("orders", vec![]).await,
        Err(PlannerError::InvalidPlan(_))
    ));
}

#[tokio::test]
async fn invalid_model_plan_is_repaired_once() {
    struct RepairModel;
    #[async_trait]
    impl AiPlanModel for RepairModel {
        async fn generate(
            &self,
            _utterance: &str,
            _candidates: &[RankedCandidate],
        ) -> Result<String, String> {
            Ok("not-json".into())
        }

        async fn repair(
            &self,
            _utterance: &str,
            _candidates: &[RankedCandidate],
            previous_output: &str,
            validation_error: &str,
        ) -> Result<String, String> {
            assert_eq!(previous_output, "not-json");
            assert!(validation_error.contains("malformed"));
            Ok(r#"{"id":"p","objective":"orders","nodes":[{"id":"a","dependsOn":[],"capabilityId":"orders","arguments":{"id":"42"},"risk":"L0","timeoutMs":1000,"retryLimit":0,"expectedOutputSchema":null}],"metadata":{}}"#.into())
        }
    }

    let (catalog, profile) = fixture();
    let planner = Planner::new(catalog, profile, Arc::new(RepairModel));
    assert_eq!(planner.plan("orders", vec![]).await.unwrap().nodes.len(), 1);
}
