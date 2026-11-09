use async_trait::async_trait;
use chrono::Utc;
use lumo_agent::*;
use lumo_core::CancelToken;
use lumo_storage::{NewAgentJob, Repo};
use serde_json::json;
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
    time::Duration,
};

struct Adapter {
    starts: Mutex<Vec<String>>,
    block: bool,
    repo: Repo,
}
#[async_trait]
impl InvocationAdapter for Adapter {
    fn source_kind(&self) -> CapabilityKind {
        CapabilityKind::Flow
    }
    async fn invoke(
        &self,
        _: InvocationRequest,
        context: InvocationContext,
    ) -> Result<InvocationResult, InvocationError> {
        assert!(
            self.repo
                .list_job_nodes("job")
                .unwrap()
                .iter()
                .any(|node| node.node_id == context.node_id && node.state == "running"),
            "inflight checkpoint must precede external execution"
        );
        self.starts.lock().unwrap().push(context.node_id.clone());
        if self.block && context.node_id == "b" {
            std::future::pending::<()>().await;
        }
        Ok(InvocationResult {
            output: json!({"node":context.node_id}),
            duration_ms: 25,
            tokens_used: 4,
            cost_usd_micro: 4,
            metadata: BTreeMap::new(),
        })
    }
}
fn setup(repo: &Repo) {
    let now = Utc::now();
    repo.enqueue_job(&NewAgentJob {
        id: "job".into(),
        idempotency_key: "key".into(),
        payload: json!({}),
        schedule_kind: "one_shot".into(),
        schedule_spec: json!({}),
        priority: 5,
        available_at: now,
        max_attempts: 3,
        created_at: now,
    })
    .unwrap();
    repo.acquire_job_lease("first", now, chrono::Duration::seconds(30))
        .unwrap()
        .unwrap();
    repo.bind_job_run("job", "first", "run", now).unwrap();
}
fn plan(risk: RiskLevel) -> AgentPlan {
    AgentPlan::new(
        "run",
        "durable test",
        vec![
            PlanNode {
                id: "a".into(),
                depends_on: vec![],
                capability_id: "a".into(),
                arguments: json!({}),
                risk,
                timeout_ms: 5_000,
                retry_limit: 0,
                expected_output_schema: None,
            },
            PlanNode {
                id: "b".into(),
                depends_on: vec!["a".into()],
                capability_id: "b".into(),
                arguments: json!({}),
                risk: RiskLevel::L0,
                timeout_ms: 5_000,
                retry_limit: 0,
                expected_output_schema: None,
            },
        ],
    )
}
fn engine(repo: &Repo, worker: &str, adapter: Arc<Adapter>) -> LoopEngine {
    engine_version(repo, worker, adapter, "")
}
fn engine_version(repo: &Repo, worker: &str, adapter: Arc<Adapter>, version: &str) -> LoopEngine {
    let catalog = CapabilityCatalog::new(
        ["a", "b"]
            .into_iter()
            .map(|id| {
                let mut cap = CapabilityDescriptor {
                    id: id.into(),
                    source: CapabilitySource::Flow {
                        path: format!("/{id}"),
                    },
                    name: id.into(),
                    description: version.into(),
                    input_schema: json!({"type":"object"}),
                    output_schema: None,
                    aliases: vec![],
                    examples: vec![],
                    risk: RiskLevel::L0,
                    enabled: true,
                    version_hash: String::new(),
                };
                cap.refresh_version_hash();
                cap
            })
            .collect(),
    )
    .unwrap();
    let profile = AgentProfile::validate(AgentProfileDraft::default(), &catalog).unwrap();
    let mut adapters = AdapterRegistry::default();
    adapters.register(adapter);
    LoopEngine::new(catalog, profile, adapters).with_job_checkpoint(JobCheckpointStore::new(
        repo.clone(),
        "job",
        worker,
    ))
}
async fn wait_started(adapter: &Adapter, id: &str) {
    tokio::time::timeout(Duration::from_secs(1), async {
        while !adapter.starts.lock().unwrap().iter().any(|node| node == id) {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
}
#[tokio::test]
async fn restart_restores_completed_output_and_skips_completed_side_effect() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("jobs.db");
    let repo = Repo::open(&path).unwrap();
    setup(&repo);
    let adapter = Arc::new(Adapter {
        starts: Mutex::new(vec![]),
        block: true,
        repo: repo.clone(),
    });
    let first_engine = engine(&repo, "first", adapter.clone());
    let task = tokio::spawn(async move {
        first_engine
            .execute(
                plan(RiskLevel::L1),
                AgentBudget::new(10, 10_000, 100, 100),
                CancelToken::new(),
            )
            .await
    });
    wait_started(&adapter, "b").await;
    task.abort();
    let _ = task.await;
    let reopened = Repo::open(&path).unwrap();
    reopened
        .recover_expired_jobs(Utc::now() + chrono::Duration::seconds(31))
        .unwrap();
    reopened
        .acquire_job_lease(
            "second",
            Utc::now() + chrono::Duration::seconds(32),
            chrono::Duration::seconds(30),
        )
        .unwrap()
        .unwrap();
    let resumed = Arc::new(Adapter {
        starts: Mutex::new(vec![]),
        block: false,
        repo: reopened.clone(),
    });
    let report = engine(&reopened, "second", resumed.clone())
        .execute(
            plan(RiskLevel::L1),
            AgentBudget::new(10, 10_000, 100, 100),
            CancelToken::new(),
        )
        .await;
    assert_eq!(report.status, LoopStatus::Completed);
    assert_eq!(*resumed.starts.lock().unwrap(), ["b"]);
    assert_eq!(report.outputs["a"].output, json!({"node":"a"}));
}
#[tokio::test]
async fn checkpoint_failure_prevents_any_external_dispatch() {
    let repo = Repo::open_in_memory().unwrap();
    setup(&repo);
    repo.with_raw(|connection| connection.execute_batch("CREATE TRIGGER checkpoint_failure BEFORE INSERT ON agent_job_nodes BEGIN SELECT RAISE(ABORT,'disk failed'); END")).unwrap();
    let adapter = Arc::new(Adapter {
        starts: Mutex::new(vec![]),
        block: false,
        repo: repo.clone(),
    });
    let report = engine(&repo, "first", adapter.clone())
        .execute(
            plan(RiskLevel::L0),
            AgentBudget::new(10, 10_000, 100, 100),
            CancelToken::new(),
        )
        .await;
    assert_eq!(report.status, LoopStatus::Failed);
    assert!(adapter.starts.lock().unwrap().is_empty());
}

#[tokio::test]
async fn unknown_nonidempotent_node_requires_human_verification_before_restart() {
    let repo = Repo::open_in_memory().unwrap();
    setup(&repo);
    let adapter = Arc::new(Adapter {
        starts: Mutex::new(vec![]),
        block: true,
        repo: repo.clone(),
    });
    let first_engine = engine(&repo, "first", adapter.clone());
    let mut unsafe_plan = plan(RiskLevel::L1);
    unsafe_plan.nodes[1].risk = RiskLevel::L1;
    let execution_plan = unsafe_plan.clone();
    let task = tokio::spawn(async move {
        first_engine
            .execute(
                execution_plan,
                AgentBudget::new(10, 10_000, 100, 100),
                CancelToken::new(),
            )
            .await
    });
    wait_started(&adapter, "b").await;
    task.abort();
    let _ = task.await;
    repo.recover_expired_jobs(Utc::now() + chrono::Duration::seconds(31))
        .unwrap();
    assert_eq!(repo.get_job("job").unwrap().unwrap().state, "unknown");
    assert!(repo
        .acquire_job_lease(
            "next",
            Utc::now() + chrono::Duration::seconds(32),
            chrono::Duration::seconds(30)
        )
        .unwrap()
        .is_none());
    let verified = InvocationResult {
        output: json!({"verified":"already applied"}),
        duration_ms: 0,
        tokens_used: 0,
        cost_usd_micro: 0,
        metadata: BTreeMap::new(),
    };
    assert!(repo
        .resolve_unknown_job_node(
            "job",
            "b",
            Some(serde_json::to_value(verified).unwrap()),
            Utc::now()
        )
        .unwrap());
    repo.acquire_job_lease("next", Utc::now(), chrono::Duration::seconds(30))
        .unwrap()
        .unwrap();
    let resumed = Arc::new(Adapter {
        starts: Mutex::new(vec![]),
        block: false,
        repo: repo.clone(),
    });
    let report = engine(&repo, "next", resumed.clone())
        .execute(
            unsafe_plan,
            AgentBudget::new(10, 10_000, 100, 100),
            CancelToken::new(),
        )
        .await;
    assert_eq!(report.status, LoopStatus::Completed);
    assert!(resumed.starts.lock().unwrap().is_empty());
    assert_eq!(
        report.outputs["b"].output,
        json!({"verified":"already applied"})
    );
}

#[tokio::test]
async fn changed_capability_or_lost_lease_never_replays_a_durable_node() {
    let repo = Repo::open_in_memory().unwrap();
    setup(&repo);
    let first = Arc::new(Adapter {
        starts: Mutex::new(vec![]),
        block: false,
        repo: repo.clone(),
    });
    assert_eq!(
        engine(&repo, "first", first)
            .execute(
                plan(RiskLevel::L1),
                AgentBudget::new(10, 10_000, 100, 100),
                CancelToken::new()
            )
            .await
            .status,
        LoopStatus::Completed
    );
    repo.retry_job("job", "first", Utc::now(), Utc::now(), "retry")
        .unwrap();
    repo.acquire_job_lease("next", Utc::now(), chrono::Duration::seconds(30))
        .unwrap()
        .unwrap();
    let changed = Arc::new(Adapter {
        starts: Mutex::new(vec![]),
        block: false,
        repo: repo.clone(),
    });
    let report = engine_version(&repo, "next", changed.clone(), "new schema or tool version")
        .execute(
            plan(RiskLevel::L1),
            AgentBudget::new(10, 10_000, 100, 100),
            CancelToken::new(),
        )
        .await;
    assert_eq!(report.status, LoopStatus::Failed);
    assert!(report.error.unwrap().contains("no longer matches"));
    assert!(changed.starts.lock().unwrap().is_empty());

    let repo = Repo::open_in_memory().unwrap();
    setup(&repo);
    let lost = Arc::new(Adapter {
        starts: Mutex::new(vec![]),
        block: false,
        repo: repo.clone(),
    });
    let report = engine(&repo, "wrong-owner", lost.clone())
        .execute(
            plan(RiskLevel::L1),
            AgentBudget::new(10, 10_000, 100, 100),
            CancelToken::new(),
        )
        .await;
    assert_eq!(report.status, LoopStatus::Failed);
    assert!(lost.starts.lock().unwrap().is_empty());
}

#[tokio::test]
async fn restarted_job_keeps_recorded_step_token_cost_and_active_runtime_budgets() {
    for (limit, (steps, runtime, tokens, cost), dispatches_b) in [
        ("step", (2, 10_000, 100, 100), false),
        ("token", (10, 10_000, 5, 100), true),
        ("cost", (10, 10_000, 100, 5), true),
        ("runtime", (10, 25, 100, 100), false),
    ] {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("jobs.db");
        let repo = Repo::open(&path).unwrap();
        setup(&repo);
        let first = Arc::new(Adapter {
            starts: Mutex::new(vec![]),
            block: true,
            repo: repo.clone(),
        });
        let first_engine = engine(&repo, "first", first.clone());
        let task = tokio::spawn(async move {
            first_engine
                .execute(
                    plan(RiskLevel::L0),
                    AgentBudget::new(10, 10_000, 100, 100),
                    CancelToken::new(),
                )
                .await
        });
        wait_started(&first, "b").await;
        task.abort();
        let _ = task.await;
        let reopened = Repo::open(&path).unwrap();
        reopened
            .recover_expired_jobs(Utc::now() + chrono::Duration::seconds(31))
            .unwrap();
        reopened
            .acquire_job_lease(
                "second",
                Utc::now() + chrono::Duration::seconds(32),
                chrono::Duration::seconds(30),
            )
            .unwrap()
            .unwrap();
        let resumed = Arc::new(Adapter {
            starts: Mutex::new(vec![]),
            block: false,
            repo: reopened.clone(),
        });
        let report = engine(&reopened, "second", resumed.clone())
            .execute(
                plan(RiskLevel::L0),
                AgentBudget::new(steps, runtime, tokens, cost),
                CancelToken::new(),
            )
            .await;
        assert_eq!(report.status, LoopStatus::BudgetExceeded, "{limit}");
        assert!(
            report.error.as_deref().unwrap().contains(limit),
            "{report:?}"
        );
        assert_eq!(
            *resumed.starts.lock().unwrap(),
            if dispatches_b { vec!["b"] } else { vec![] },
            "{limit}"
        );
        assert_eq!(report.outputs["a"].tokens_used, 4);
    }
}
