use lumo_agent::{
    validate_dag, AgentBudget, AgentEventDraft, AgentEventKind, EventSequence, PlanError,
    AgentPlan, PlanNode, RiskLevel,
};
use serde_json::json;

fn node(id: &str, depends_on: &[&str]) -> PlanNode {
    PlanNode {
        id: id.into(),
        depends_on: depends_on.iter().map(|value| (*value).into()).collect(),
        capability_id: format!("flow:{id}"),
        arguments: json!({}),
        risk: RiskLevel::L0,
        timeout_ms: 1_000,
        retry_limit: 0,
        expected_output_schema: None,
    }
}

#[test]
fn rejects_cycles_and_unknown_dependencies() {
    let cyclic = AgentPlan::new("plan-1", "cycle", vec![node("a", &["b"]), node("b", &["a"])]);
    assert!(matches!(validate_dag(&cyclic), Err(PlanError::Cycle(_))));

    let missing = AgentPlan::new("plan-2", "missing", vec![node("a", &["ghost"])]);
    assert!(matches!(
        validate_dag(&missing),
        Err(PlanError::UnknownDependency { .. })
    ));
}

#[test]
fn event_sequences_are_monotonic_per_run() {
    let sequence = EventSequence::default();
    let first = sequence.stamp(AgentEventDraft::new("run-a", AgentEventKind::NodeQueued));
    let second = sequence.stamp(AgentEventDraft::new("run-a", AgentEventKind::NodeStarted));
    let other_run = sequence.stamp(AgentEventDraft::new("run-b", AgentEventKind::RunStarted));

    assert_eq!((first.seq, second.seq, other_run.seq), (1, 2, 1));
    assert!(first.timestamp <= second.timestamp);
}

#[test]
fn hard_budget_exhaustion_never_overcommits() {
    let mut budget = AgentBudget::new(2, 10_000, 100, 50);
    budget.reserve_step().unwrap();
    budget.reserve_step().unwrap();
    assert_eq!(budget.reserve_step().unwrap_err().limit(), "steps");

    budget.charge_usage(60, 20).unwrap();
    assert_eq!(budget.charge_usage(41, 0).unwrap_err().limit(), "tokens");
    assert_eq!(budget.charge_usage(0, 31).unwrap_err().limit(), "cost");
    assert_eq!((budget.steps_used(), budget.tokens_used(), budget.cost_used_usd_micro()), (2, 60, 20));
}
