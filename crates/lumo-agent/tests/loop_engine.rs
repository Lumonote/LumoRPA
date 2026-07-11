use std::{
    collections::BTreeMap,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};

use async_trait::async_trait;
use lumo_agent::{
    AdapterRegistry, AgentBudget, AgentPlan, AgentProfile, AgentProfileDraft, CapabilityCatalog,
    CapabilityDescriptor, CapabilityKind, CapabilitySource, InvocationAdapter, InvocationContext,
    InvocationError, InvocationRequest, InvocationResult, LoopEngine, LoopStatus, PlanNode,
    RiskLevel,
};
use lumo_core::CancelToken;
use serde_json::json;

fn capability(id: &str) -> CapabilityDescriptor {
    let mut descriptor = CapabilityDescriptor {
        id: id.into(),
        source: CapabilitySource::Flow {
            path: format!("/{id}"),
        },
        name: id.into(),
        description: String::new(),
        input_schema: json!({"type": "object"}),
        output_schema: None,
        aliases: vec![],
        examples: vec![],
        risk: RiskLevel::L0,
        enabled: true,
        version_hash: String::new(),
    };
    descriptor.refresh_version_hash();
    descriptor
}

fn node(id: &str, depends_on: &[&str], retry_limit: u32, timeout_ms: u64) -> PlanNode {
    PlanNode {
        id: id.into(),
        depends_on: depends_on.iter().map(|value| (*value).into()).collect(),
        capability_id: id.into(),
        arguments: json!({}),
        risk: RiskLevel::L0,
        timeout_ms,
        retry_limit,
        expected_output_schema: Some(json!({"type": "object"})),
    }
}

struct TestAdapter {
    active: AtomicUsize,
    max_active: AtomicUsize,
    starts: Mutex<Vec<String>>,
    delay_ms: u64,
    fail_first: bool,
}

#[async_trait]
impl InvocationAdapter for TestAdapter {
    fn source_kind(&self) -> CapabilityKind {
        CapabilityKind::Flow
    }

    async fn invoke(
        &self,
        request: InvocationRequest,
        context: InvocationContext,
    ) -> Result<InvocationResult, InvocationError> {
        self.starts.lock().unwrap().push(context.node_id.clone());
        let active = self.active.fetch_add(1, Ordering::SeqCst) + 1;
        self.max_active.fetch_max(active, Ordering::SeqCst);
        tokio::time::sleep(Duration::from_millis(self.delay_ms)).await;
        self.active.fetch_sub(1, Ordering::SeqCst);
        if context.cancel.is_cancelled() {
            return Err(InvocationError::Cancelled);
        }
        if self.fail_first && request.attempt == 1 {
            return Err(InvocationError::Failed("retry me".into()));
        }
        Ok(InvocationResult {
            output: json!({"node": context.node_id}),
            duration_ms: self.delay_ms,
            tokens_used: 0,
            cost_usd_micro: 0,
            metadata: BTreeMap::new(),
        })
    }
}

fn engine(ids: &[&str], adapter: Arc<TestAdapter>) -> LoopEngine {
    let catalog = CapabilityCatalog::new(ids.iter().map(|id| capability(id)).collect()).unwrap();
    let draft = AgentProfileDraft {
        max_parallel: 16,
        ..AgentProfileDraft::default()
    };
    let profile = AgentProfile::validate(draft, &catalog).unwrap();
    let mut adapters = AdapterRegistry::default();
    adapters.register(adapter);
    LoopEngine::new(catalog, profile, adapters)
}

#[tokio::test]
async fn serial_dependencies_and_outputs_are_deterministic() {
    let adapter = Arc::new(TestAdapter {
        active: AtomicUsize::new(0),
        max_active: AtomicUsize::new(0),
        starts: Mutex::new(vec![]),
        delay_ms: 2,
        fail_first: false,
    });
    let engine = engine(&["z", "a"], adapter.clone());
    let plan = AgentPlan::new("run-1", "serial", vec![node("z", &[], 0, 100), node("a", &["z"], 0, 100)]);
    let report = engine
        .execute(plan, AgentBudget::new(10, 10_000, 100, 100), CancelToken::new())
        .await;

    assert_eq!(report.status, LoopStatus::Completed);
    assert_eq!(*adapter.starts.lock().unwrap(), vec!["z", "a"]);
    assert_eq!(report.outputs.keys().cloned().collect::<Vec<_>>(), vec!["a", "z"]);
}

#[tokio::test]
async fn parallelism_is_hard_capped_at_four() {
    let ids = ["a", "b", "c", "d", "e", "f"];
    let adapter = Arc::new(TestAdapter {
        active: AtomicUsize::new(0),
        max_active: AtomicUsize::new(0),
        starts: Mutex::new(vec![]),
        delay_ms: 25,
        fail_first: false,
    });
    let engine = engine(&ids, adapter.clone());
    let plan = AgentPlan::new(
        "run-2",
        "parallel",
        ids.iter().map(|id| node(id, &[], 0, 200)).collect(),
    );
    let report = engine
        .execute(plan, AgentBudget::new(10, 10_000, 100, 100), CancelToken::new())
        .await;

    assert_eq!(report.status, LoopStatus::Completed);
    assert_eq!(adapter.max_active.load(Ordering::SeqCst), 4);
}

#[tokio::test]
async fn retry_timeout_and_cancel_are_bounded() {
    let retrying = Arc::new(TestAdapter {
        active: AtomicUsize::new(0),
        max_active: AtomicUsize::new(0),
        starts: Mutex::new(vec![]),
        delay_ms: 2,
        fail_first: true,
    });
    let report = engine(&["a"], retrying)
        .execute(
            AgentPlan::new("retry", "retry", vec![node("a", &[], 1, 100)]),
            AgentBudget::new(5, 10_000, 100, 100),
            CancelToken::new(),
        )
        .await;
    assert_eq!(report.status, LoopStatus::Completed);
    assert_eq!(report.attempts["a"], 2);

    let slow = Arc::new(TestAdapter {
        active: AtomicUsize::new(0),
        max_active: AtomicUsize::new(0),
        starts: Mutex::new(vec![]),
        delay_ms: 100,
        fail_first: false,
    });
    let timed_out = engine(&["a"], slow.clone())
        .execute(
            AgentPlan::new("timeout", "timeout", vec![node("a", &[], 0, 5)]),
            AgentBudget::new(5, 10_000, 100, 100),
            CancelToken::new(),
        )
        .await;
    assert_eq!(timed_out.status, LoopStatus::Failed);

    let cancel = CancelToken::new();
    let cancel_later = cancel.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(5)).await;
        cancel_later.cancel();
    });
    let cancelled = engine(&["a"], slow)
        .execute(
            AgentPlan::new("cancel", "cancel", vec![node("a", &[], 0, 500)]),
            AgentBudget::new(5, 10_000, 100, 100),
            cancel,
        )
        .await;
    assert_eq!(cancelled.status, LoopStatus::Cancelled);
}

#[tokio::test]
async fn budget_exhaustion_stops_before_an_unreserved_transition() {
    let adapter = Arc::new(TestAdapter {
        active: AtomicUsize::new(0),
        max_active: AtomicUsize::new(0),
        starts: Mutex::new(vec![]),
        delay_ms: 1,
        fail_first: false,
    });
    let report = engine(&["a", "b"], adapter.clone())
        .execute(
            AgentPlan::new("budget", "budget", vec![node("a", &[], 0, 100), node("b", &["a"], 0, 100)]),
            AgentBudget::new(1, 10_000, 100, 100),
            CancelToken::new(),
        )
        .await;
    assert_eq!(report.status, LoopStatus::BudgetExceeded);
    assert_eq!(*adapter.starts.lock().unwrap(), vec!["a"]);
}
