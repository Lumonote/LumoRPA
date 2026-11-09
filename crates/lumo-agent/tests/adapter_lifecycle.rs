//! Adapter deadlines must stop the VM cooperatively. Dropping a VM future
//! skips both resource teardown and its persisted terminal run state.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use lumo_agent::{
    CapabilityDescriptor, CapabilitySource, FlowAdapter, InvocationAdapter, InvocationContext,
    InvocationError, InvocationRequest, RiskLevel, SkillAdapter,
};
use lumo_core::{Action, ActionRegistry, ActionResult, FlowVm, RunTeardown, StepCtx, StepError};
use lumo_skills::SkillRegistry;
use lumo_storage::Repo;
use serde_json::{json, Value};
use tokio::sync::Notify;

struct WaitAction(Arc<Notify>);

#[async_trait]
impl Action for WaitAction {
    fn id(&self) -> &'static str {
        "test.wait"
    }

    async fn execute(&self, _ctx: &mut StepCtx, _input: Value) -> Result<ActionResult, StepError> {
        self.0.notify_one();
        std::future::pending().await
    }
}

struct Teardown(Arc<Notify>);

#[async_trait]
impl RunTeardown for Teardown {
    async fn teardown(&self, _run_id: &str) {
        self.0.notify_one();
    }
}

struct Fixture {
    _dir: tempfile::TempDir,
    flow: CapabilityDescriptor,
    skill: CapabilityDescriptor,
    flow_adapter: Arc<FlowAdapter>,
    skill_adapter: Arc<SkillAdapter>,
    started: Arc<Notify>,
    cleaned: Arc<Notify>,
    repo: Repo,
}

fn descriptor(id: &str, source: CapabilitySource) -> CapabilityDescriptor {
    let mut descriptor = CapabilityDescriptor {
        id: id.into(),
        source,
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

impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let flow_path = dir.path().join("wait.yaml");
        std::fs::write(
            &flow_path,
            "apiVersion: lumorpa.io/v1\nkind: Flow\nmetadata: { id: wait }\nspec:\n  steps:\n    - { id: wait, action: test.wait, with: {} }\n",
        )
        .unwrap();
        let skill_path = dir.path().join("SKILL.md");
        std::fs::write(
            &skill_path,
            "---\nname: wait-skill\n---\n```yaml\nsteps:\n  - { id: wait, action: test.wait, with: {} }\n```\n",
        )
        .unwrap();
        let skills = Arc::new(SkillRegistry::new());
        skills.insert(lumo_skills::loader::load_skill_file(&skill_path).unwrap());
        let started = Arc::new(Notify::new());
        let cleaned = Arc::new(Notify::new());
        let mut registry = ActionRegistry::new();
        registry.register(WaitAction(started.clone()));
        registry.register_teardown(Arc::new(Teardown(cleaned.clone())));
        let repo = Repo::open_in_memory().unwrap();
        let vm_repo = repo.clone();
        let factory = Arc::new(move || FlowVm::new(registry.clone(), Some(vm_repo.clone())));
        Self {
            flow: descriptor(
                "flow:wait",
                CapabilitySource::Flow {
                    path: flow_path.display().to_string(),
                },
            ),
            skill: descriptor(
                "skill:wait-skill",
                CapabilitySource::Skill {
                    name: "wait-skill".into(),
                    source: skill_path.display().to_string(),
                },
            ),
            flow_adapter: Arc::new(FlowAdapter::new(factory.clone())),
            skill_adapter: Arc::new(SkillAdapter::new(skills, factory)),
            started,
            cleaned,
            repo,
            _dir: dir,
        }
    }

    fn request(&self, skill: bool, timeout_ms: u64) -> InvocationRequest {
        InvocationRequest {
            capability: if skill {
                self.skill.clone()
            } else {
                self.flow.clone()
            },
            arguments: json!({}),
            attempt: 1,
            timeout_ms,
        }
    }

    async fn assert_cleanup(&self) {
        tokio::time::timeout(Duration::from_secs(2), self.cleaned.notified())
            .await
            .expect("VM must reach resource teardown after the adapter stops waiting");
        // Teardown precedes terminal persistence, so allow the VM to finish
        // that final small operation before asserting the durable result.
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                let runs = self.repo.list_runs(10).unwrap();
                if runs.iter().all(|run| run.state == "cancelled") && !runs.is_empty() {
                    assert!(runs.iter().all(|run| run.finished_at.is_some()));
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("cancelled VM must persist a terminal run state");
    }
}

#[tokio::test]
async fn flow_timeout_cleans_up_and_does_not_cancel_the_agent() {
    let fixture = Fixture::new();
    let context = InvocationContext::new("agent-run", "flow-node");
    let error = fixture
        .flow_adapter
        .invoke(fixture.request(false, 50), context.clone())
        .await
        .unwrap_err();
    assert_eq!(error, InvocationError::Timeout { duration_ms: 50 });
    assert!(
        !context.cancel.is_cancelled(),
        "other nodes and retries must remain runnable"
    );
    fixture.assert_cleanup().await;
}

#[tokio::test]
async fn skill_timeout_cleans_up_and_persists_terminal_state() {
    let fixture = Fixture::new();
    let error = fixture
        .skill_adapter
        .invoke(
            fixture.request(true, 50),
            InvocationContext::new("agent-run", "skill-node"),
        )
        .await
        .unwrap_err();
    assert_eq!(error, InvocationError::Timeout { duration_ms: 50 });
    fixture.assert_cleanup().await;
}

#[tokio::test]
async fn cancelling_the_agent_still_cleans_up_the_flow() {
    let fixture = Fixture::new();
    let adapter = fixture.flow_adapter.clone();
    let request = fixture.request(false, 10_000);
    let context = InvocationContext::new("agent-run", "flow-node");
    let cancel = context.cancel.clone();
    let task = tokio::spawn(async move { adapter.invoke(request, context).await });
    tokio::time::timeout(Duration::from_secs(2), fixture.started.notified())
        .await
        .unwrap();
    cancel.cancel();
    assert_eq!(task.await.unwrap().unwrap_err(), InvocationError::Cancelled);
    fixture.assert_cleanup().await;
}

#[tokio::test]
async fn aborting_the_adapter_task_still_cancels_and_cleans_up_the_vm() {
    let fixture = Fixture::new();
    let adapter = fixture.skill_adapter.clone();
    let request = fixture.request(true, 10_000);
    let task = tokio::spawn(async move {
        adapter
            .invoke(request, InvocationContext::new("agent-run", "skill-node"))
            .await
    });
    tokio::time::timeout(Duration::from_secs(2), fixture.started.notified())
        .await
        .unwrap();
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    fixture.assert_cleanup().await;
}
