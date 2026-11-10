use async_trait::async_trait;
use lumo_agent::*;
use lumo_core::{
    error::StepError, Action, ActionRegistry, ActionResult, CancelToken, FlowVm, StepCtx,
};
use lumo_storage::{AiCallInsert, Repo};
use serde_json::{json, Value};
use std::{
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    time::Duration,
};

struct MeteredAction {
    calls: Arc<AtomicUsize>,
    hang: bool,
}
#[async_trait]
impl Action for MeteredAction {
    fn id(&self) -> &'static str {
        "test.metered_attempt"
    }
    fn summary(&self) -> &'static str {
        "Record usage before succeeding or failing"
    }
    async fn execute(&self, ctx: &mut StepCtx, _input: Value) -> Result<ActionResult, StepError> {
        let call = self.calls.fetch_add(1, Ordering::SeqCst);
        ctx.repo()
            .unwrap()
            .record_ai_call(AiCallInsert {
                flow_run_id: ctx.run_id(),
                step_id: Some("meter"),
                helper: "test",
                provider: "test",
                model: "test",
                input_tokens: 3,
                output_tokens: 2,
                latency_ms: 1,
                cost_usd_micro: 9,
            })
            .unwrap();
        if self.hang {
            let _unreported = ctx.begin_usage_call();
            std::future::pending::<()>().await;
        }
        if call == 0 {
            return Err(StepError::msg("first metered call failed"));
        }
        Ok(ActionResult::from(json!({"done":true})))
    }
}

fn setup(repo: &Repo, path: &std::path::Path, calls: Arc<AtomicUsize>, hang: bool) -> LoopEngine {
    let descriptor = CapabilityDescriptor {
        id: "flow:test".into(),
        source: CapabilitySource::Flow {
            path: path.display().to_string(),
        },
        name: "test".into(),
        description: "".into(),
        input_schema: json!({"type":"object"}),
        output_schema: None,
        aliases: vec![],
        examples: vec![],
        risk: RiskLevel::L0,
        enabled: true,
        version_hash: String::new(),
    };
    let mut descriptor = descriptor;
    descriptor.refresh_version_hash();
    let catalog = CapabilityCatalog::new(vec![descriptor]).unwrap();
    let profile = AgentProfile::validate(AgentProfileDraft::default(), &catalog).unwrap();
    let vm_repo = repo.clone();
    let factory = Arc::new(move || {
        let mut registry = ActionRegistry::new();
        registry.register(MeteredAction {
            calls: calls.clone(),
            hang,
        });
        FlowVm::new(registry, Some(vm_repo.clone()))
    });
    let mut adapters = AdapterRegistry::default();
    adapters.register(Arc::new(FlowAdapter::new(factory)));
    LoopEngine::new(catalog, profile, adapters).with_usage_ledger(repo.clone())
}
fn plan(run_id: &str, timeout: u64) -> AgentPlan {
    AgentPlan::new(
        run_id,
        "metered retry",
        vec![PlanNode {
            id: "node".into(),
            capability_id: "flow:test".into(),
            depends_on: vec![],
            arguments: json!({}),
            risk: RiskLevel::L0,
            timeout_ms: timeout,
            retry_limit: 2,
            expected_output_schema: None,
        }],
    )
}
fn source(path: &std::path::Path) {
    std::fs::write(path,"apiVersion: lumorpa.io/v1\nkind: Flow\nmetadata: {id: metered}\nspec:\n  capabilities: {llm: ['*']}\n  steps:\n    - {id: meter, action: test.metered_attempt, with: {}}\n").unwrap();
}

#[tokio::test]
async fn failed_vm_usage_is_preserved_and_charged_before_retrying() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("flow.yaml");
    source(&path);
    let repo = Repo::open_in_memory().unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let engine = setup(&repo, &path, calls.clone(), false);
    let report = engine
        .execute(
            plan("all-attempts", 1000),
            AgentBudget::new(10, 10000, 100, 100),
            CancelToken::new(),
        )
        .await;
    assert_eq!(report.status, LoopStatus::Completed);
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    let rows = repo.list_invocation_attempts("all-attempts").unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].state, "failed");
    assert_eq!(rows[1].state, "completed");
    assert_eq!(rows.iter().map(|r| r.tokens_used.unwrap()).sum::<u64>(), 10);
    assert_eq!(
        rows.iter().map(|r| r.cost_usd_micro.unwrap()).sum::<u64>(),
        18
    );
    for row in &rows {
        assert_eq!(row.usage_state, "recorded");
        assert!(repo
            .get_run(row.flow_run_id.as_deref().unwrap())
            .unwrap()
            .is_some());
    }
    // A resumed attempt must pay for the failed call too, rather than starting
    // with the successful output's budget or granting a fresh allowance.
    let limited = engine
        .execute(
            plan("all-attempts", 1000),
            AgentBudget::new(10, 10000, 9, 100),
            CancelToken::new(),
        )
        .await;
    assert_eq!(limited.status, LoopStatus::BudgetExceeded);
    assert_eq!(calls.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn failures_exhausting_token_budget_do_not_get_a_free_retry() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("flow.yaml");
    source(&path);
    let repo = Repo::open_in_memory().unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let report = setup(&repo, &path, calls.clone(), false)
        .execute(
            plan("failed-budget", 1000),
            AgentBudget::new(10, 10000, 4, 100),
            CancelToken::new(),
        )
        .await;
    assert_eq!(report.status, LoopStatus::BudgetExceeded);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        repo.list_invocation_attempts("failed-budget").unwrap()[0].tokens_used,
        Some(5)
    );
}

#[tokio::test]
async fn timeout_marks_unreported_spend_unknown_and_blocks_automatic_retry() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("flow.yaml");
    source(&path);
    let repo = Repo::open_in_memory().unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let engine = setup(&repo, &path, calls.clone(), true);
    let report = engine
        .execute(
            plan("timeout-usage", 20),
            AgentBudget::new(10, 10000, 100, 100),
            CancelToken::new(),
        )
        .await;
    assert_eq!(report.status, LoopStatus::BudgetExceeded);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let flow = repo.list_invocation_flow_runs("timeout-usage").unwrap();
            if flow.iter().all(|run| run.state != "running") {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let row = &repo.list_invocation_attempts("timeout-usage").unwrap()[0];
    assert_eq!(row.usage_state, "unknown");
    assert_eq!(row.tokens_used, Some(5));
    let restarted = engine
        .execute(
            plan("timeout-usage", 20),
            AgentBudget::new(10, 10000, 100, 100),
            CancelToken::new(),
        )
        .await;
    assert_eq!(restarted.status, LoopStatus::BudgetExceeded);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    // Once the operator has checked the final bill, a new process can resume
    // without losing the failed call's usage or counting it twice.
    let saved = repo
        .list_invocation_attempts("timeout-usage")
        .unwrap()
        .remove(0);
    assert!(repo
        .resolve_invocation_usage(&lumo_storage::InvocationUsageResolution {
            run_id: "timeout-usage".into(),
            node_id: "node".into(),
            attempt: 1,
            expected_revision: saved.revision,
            tokens_used: 5,
            cost_usd_micro: 9,
            resolved_by: "tester".into(),
            reason: "provider confirmed final usage".into(),
        })
        .unwrap());
    let resumed = setup(&repo, &path, calls.clone(), false)
        .execute(
            plan("timeout-usage", 1000),
            AgentBudget::new(10, 10000, 100, 100),
            CancelToken::new(),
        )
        .await;
    assert_eq!(resumed.status, LoopStatus::Completed);
    let rows = repo.list_invocation_attempts("timeout-usage").unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(
        rows.iter().map(|row| row.tokens_used.unwrap()).sum::<u64>(),
        10
    );
}

#[tokio::test]
async fn ledger_write_failure_prevents_dispatch() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("flow.yaml");
    source(&path);
    let repo = Repo::open_in_memory().unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    repo.with_raw(|conn|conn.execute_batch("CREATE TRIGGER ledger_failure BEFORE INSERT ON agent_invocation_attempts BEGIN SELECT RAISE(ABORT,'full'); END")).unwrap();
    let report = setup(&repo, &path, calls.clone(), false)
        .execute(
            plan("no-dispatch", 1000),
            AgentBudget::new(10, 10000, 100, 100),
            CancelToken::new(),
        )
        .await;
    assert_eq!(report.status, LoopStatus::Failed);
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}
