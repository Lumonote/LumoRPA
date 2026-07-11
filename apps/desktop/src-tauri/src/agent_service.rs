use async_trait::async_trait;
use chrono::Utc;
use lumo_agent::{
    AdapterRegistry, AgentBudget, AgentEvent, AgentEventDraft, AgentEventKind, AgentEventPublisher,
    AgentHarness, AgentPlan, AgentProfile, AgentProfileDraft, CapabilityCatalog,
    CapabilityDescriptor, CapabilitySource, EventSink, FlowAdapter, InvocationError, LoopEngine,
    LoopReport, LoopStatus, McpAdapter, McpClientInvoker, McpConnectionProfile, McpProfileResolver,
    PlannerBackend, RankedCandidate, RiskLevel, RouteOutcome, Router, SkillAdapter,
};
use lumo_core::{CancelToken, FlowVm};
use lumo_storage::{AgentRunRow, Repo};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::{collections::BTreeMap, path::PathBuf, sync::Arc};
use tauri::{Emitter, Wry};

use super::{
    app_home, build_action_registry, list_flow_library, load_skill_registry, mcp_commands,
};

type AppHandle = tauri::AppHandle<Wry>;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentStartInput {
    pub utterance: String,
    #[serde(default)]
    pub profile_id: Option<String>,
    #[serde(default)]
    pub supplied_plan: Option<AgentPlan>,
}

#[async_trait]
pub trait DesktopAgentFactory: Send + Sync {
    async fn catalog(&self, profile_id: Option<&str>) -> Result<CapabilityCatalog, String>;
    async fn profile(&self, profile_id: Option<&str>) -> Result<AgentProfile, String>;
    async fn adapters(&self, profile: &AgentProfile) -> Result<AdapterRegistry, String>;
    async fn plan(
        &self,
        utterance: &str,
        catalog: &CapabilityCatalog,
        profile: &AgentProfile,
    ) -> Result<AgentPlan, String>;
}

pub trait DesktopAgentEventEmitter: Send + Sync {
    fn emit(&self, event: &AgentEvent);
}

#[cfg(test)]
pub struct NoopDesktopAgentEventEmitter;

#[cfg(test)]
impl DesktopAgentEventEmitter for NoopDesktopAgentEventEmitter {
    fn emit(&self, _event: &AgentEvent) {}
}

pub struct TauriDesktopAgentEventEmitter {
    app: AppHandle,
}

impl TauriDesktopAgentEventEmitter {
    pub fn new(app: AppHandle) -> Self {
        Self { app }
    }
}

impl DesktopAgentEventEmitter for TauriDesktopAgentEventEmitter {
    fn emit(&self, event: &AgentEvent) {
        let _ = self.app.emit("lumo://agent-event", event);
    }
}

#[derive(Clone)]
struct PersistingDesktopEventPublisher {
    sink: EventSink,
    emitter: Arc<dyn DesktopAgentEventEmitter>,
}

#[async_trait]
impl AgentEventPublisher for PersistingDesktopEventPublisher {
    async fn publish(&self, draft: AgentEventDraft) -> Result<(), String> {
        let event = self
            .sink
            .publish(draft)
            .await
            .map_err(|error| error.to_string())?;
        self.emitter.emit(&event);
        Ok(())
    }
}

#[derive(Clone)]
pub struct DesktopAgentService {
    factory: Arc<dyn DesktopAgentFactory>,
    repo: Repo,
    events: Arc<dyn AgentEventPublisher>,
}

impl DesktopAgentService {
    pub fn new(
        factory: Arc<dyn DesktopAgentFactory>,
        repo: Repo,
        emitter: Arc<dyn DesktopAgentEventEmitter>,
    ) -> Self {
        let events = Arc::new(PersistingDesktopEventPublisher {
            sink: EventSink::new(Arc::new(repo.clone()), 256),
            emitter,
        });
        Self {
            factory,
            repo,
            events,
        }
    }

    pub async fn start(&self, input: AgentStartInput) -> Result<AgentExecution, String> {
        let utterance = input.utterance.trim();
        if utterance.is_empty() {
            return Err("agent utterance must not be empty".into());
        }
        let catalog = self.factory.catalog(input.profile_id.as_deref()).await?;
        let profile = self.factory.profile(input.profile_id.as_deref()).await?;
        let adapters = self.factory.adapters(&profile).await?;
        let mut plan = match input.supplied_plan {
            Some(plan) => plan,
            None => self.factory.plan(utterance, &catalog, &profile).await?,
        };
        let run_id = ulid::Ulid::new().to_string();
        plan.id.clone_from(&run_id);
        lumo_agent::validate_dag(&plan).map_err(|error| error.to_string())?;

        self.repo
            .create_agent_run(&AgentRunRow {
                id: run_id.clone(),
                profile_id: Some(profile.id.clone()),
                utterance: Some(utterance.to_string()),
                plan_json: Some(serde_json::to_value(&plan).map_err(|error| error.to_string())?),
                approval_json: None,
                state: "running".into(),
                started_at: Utc::now(),
                finished_at: None,
                error: None,
            })
            .map_err(|error| error.to_string())?;
        if let Err(error) = self.publish_start_events(&run_id, &plan).await {
            let _ =
                self.repo
                    .update_agent_run_state(&run_id, "failed", Some(Utc::now()), Some(&error));
            return Err(error);
        }

        let cancel = CancelToken::new();
        let harness = AgentHarness::new(
            LoopEngine::new(catalog, profile.clone(), adapters)
                .with_event_publisher(self.events.clone()),
        );
        let budget = AgentBudget::new(
            profile.max_steps,
            profile.max_runtime_ms,
            profile.max_tokens,
            profile.max_cost_usd_micro,
        );
        let execution_plan = plan.clone();
        let execution_cancel = cancel.clone();
        let repo = self.repo.clone();
        let completion = tokio::spawn(async move {
            let report = harness
                .execute(execution_plan, budget, execution_cancel)
                .await;
            let (state, finished_at) = run_state(report.status);
            repo.update_agent_run_state(
                &report.run_id,
                state,
                finished_at.then(Utc::now),
                report.error.as_deref(),
            )
            .map_err(|error| error.to_string())?;
            Ok(report)
        });

        Ok(AgentExecution {
            run_id,
            plan,
            cancel,
            _completion: completion,
        })
    }

    async fn publish_start_events(&self, run_id: &str, plan: &AgentPlan) -> Result<(), String> {
        self.events
            .publish(AgentEventDraft::new(run_id, AgentEventKind::SessionStarted))
            .await?;
        self.events
            .publish(
                AgentEventDraft::new(run_id, AgentEventKind::PlanCreated)
                    .payload(json!({ "plan": plan })),
            )
            .await
    }
}

fn run_state(status: LoopStatus) -> (&'static str, bool) {
    match status {
        LoopStatus::Completed => ("completed", true),
        LoopStatus::Failed | LoopStatus::BudgetExceeded => ("failed", true),
        LoopStatus::Cancelled => ("cancelled", true),
        LoopStatus::WaitingApproval => ("waiting_approval", false),
    }
}

pub struct AgentExecution {
    run_id: String,
    pub plan: AgentPlan,
    pub cancel: CancelToken,
    _completion: tokio::task::JoinHandle<Result<LoopReport, String>>,
}

impl AgentExecution {
    pub(crate) fn run_id(&self) -> &str {
        &self.run_id
    }

    pub(crate) fn cancel_token(&self) -> CancelToken {
        self.cancel.clone()
    }

    pub(crate) async fn wait(self) -> Result<LoopReport, String> {
        self._completion
            .await
            .map_err(|error| format!("agent execution task failed: {error}"))?
    }

    pub fn into_session_parts(self) -> (String, AgentPlan, CancelToken) {
        (self.run_id, self.plan, self.cancel)
    }
}

pub struct ProductionDesktopAgentFactory {
    app: AppHandle,
    repo: Repo,
    home: PathBuf,
}

impl ProductionDesktopAgentFactory {
    pub fn new(app: AppHandle, repo: Repo) -> Result<Self, String> {
        let home = app_home(&app)?;
        Ok(Self { app, repo, home })
    }

    fn capability_catalog(&self) -> Result<CapabilityCatalog, String> {
        let mut descriptors = BTreeMap::<String, CapabilityDescriptor>::new();
        for flow in list_flow_library(self.app.clone())? {
            if !flow.valid {
                continue;
            }
            let logical_id = flow.id.clone().unwrap_or_else(|| flow.file_name.clone());
            let id = format!("flow:{logical_id}");
            let mut aliases = vec![logical_id.clone()];
            if let Some(name) = flow.name.clone() {
                aliases.push(name);
            }
            let mut descriptor = CapabilityDescriptor {
                id: id.clone(),
                source: CapabilitySource::Flow { path: flow.path },
                name: flow.name.unwrap_or(logical_id),
                description: flow.description.unwrap_or_default(),
                input_schema: json!({"type": "object"}),
                output_schema: None,
                aliases,
                examples: Vec::new(),
                risk: RiskLevel::L1,
                enabled: true,
                version_hash: String::new(),
            };
            descriptor.refresh_version_hash();
            descriptors.entry(id).or_insert(descriptor);
        }

        for skill in load_skill_registry(&self.home, None).all() {
            let name = skill.name().to_string();
            let id = format!("skill:{name}");
            let mut descriptor = CapabilityDescriptor {
                id: id.clone(),
                source: CapabilitySource::Skill {
                    name: name.clone(),
                    source: skill.source.display().to_string(),
                },
                name: name.clone(),
                description: skill.description().unwrap_or_default().to_string(),
                input_schema: json!({"type": "object"}),
                output_schema: None,
                aliases: vec![name],
                examples: Vec::new(),
                risk: RiskLevel::L1,
                enabled: true,
                version_hash: String::new(),
            };
            descriptor.refresh_version_hash();
            descriptors.entry(id).or_insert(descriptor);
        }

        for server in self
            .repo
            .list_mcp_servers()
            .map_err(|error| error.to_string())?
        {
            for tool in self
                .repo
                .list_mcp_tools(&server.id)
                .map_err(|error| error.to_string())?
            {
                let mut descriptor =
                    CapabilityDescriptor::mcp(&server.id, &tool.name, tool.input_schema);
                descriptor.description = tool.description;
                descriptor.output_schema = tool.output_schema;
                descriptor.aliases =
                    vec![tool.name.clone(), format!("{} {}", server.name, tool.name)];
                descriptor.risk = parse_risk(&tool.risk)?;
                descriptor.enabled = server.enabled && tool.enabled;
                descriptor.refresh_version_hash();
                descriptors.insert(descriptor.id.clone(), descriptor);
            }
        }
        CapabilityCatalog::new(descriptors.into_values().collect())
            .map_err(|error| error.to_string())
    }

    fn profile_draft(&self, profile_id: Option<&str>) -> Result<AgentProfileDraft, String> {
        let stored = match profile_id {
            Some(id) => self.repo.with_raw(|connection| {
                let mut statement =
                    connection.prepare("SELECT config_json FROM agent_profiles WHERE id=?")?;
                let mut rows = statement.query([id])?;
                rows.next()?
                    .map(|row| row.get::<_, String>(0))
                    .transpose()
            }),
            None => self.repo.with_raw(|connection| {
                let mut statement = connection.prepare(
                    "SELECT config_json FROM agent_profiles ORDER BY is_default DESC, updated_at DESC LIMIT 1",
                )?;
                let mut rows = statement.query([])?;
                rows.next()?
                    .map(|row| row.get::<_, String>(0))
                    .transpose()
            }),
        }
        .map_err(|error| error.to_string())?;

        match stored {
            Some(raw) => serde_json::from_str(&raw)
                .map_err(|error| format!("agent profile is invalid: {error}")),
            None if profile_id.is_some() => Err(format!(
                "agent profile `{}` was not found",
                profile_id.unwrap_or_default()
            )),
            None => Ok(AgentProfileDraft::default()),
        }
    }
}

#[async_trait]
impl DesktopAgentFactory for ProductionDesktopAgentFactory {
    async fn catalog(&self, _profile_id: Option<&str>) -> Result<CapabilityCatalog, String> {
        self.capability_catalog()
    }

    async fn profile(&self, profile_id: Option<&str>) -> Result<AgentProfile, String> {
        let catalog = self.capability_catalog()?;
        AgentProfile::validate(self.profile_draft(profile_id)?, &catalog)
            .map_err(|error| error.to_string())
    }

    async fn adapters(&self, _profile: &AgentProfile) -> Result<AdapterRegistry, String> {
        let home = self.home.clone();
        let repo = self.repo.clone();
        let vm_factory =
            Arc::new(move || FlowVm::new(build_action_registry(&home, None), Some(repo.clone())));
        let mut adapters = AdapterRegistry::default();
        adapters.register(Arc::new(FlowAdapter::new(vm_factory.clone())));
        adapters.register(Arc::new(SkillAdapter::new(
            load_skill_registry(&self.home, None),
            vm_factory,
        )));
        let resolver = Arc::new(DesktopMcpProfileResolver {
            repo: self.repo.clone(),
            home: self.home.clone(),
        });
        adapters.register(Arc::new(McpAdapter::new(Arc::new(McpClientInvoker::new(
            resolver,
        )))));
        Ok(adapters)
    }

    async fn plan(
        &self,
        utterance: &str,
        catalog: &CapabilityCatalog,
        profile: &AgentProfile,
    ) -> Result<AgentPlan, String> {
        match Router::new(
            catalog.clone(),
            profile.clone(),
            Arc::new(NoFallbackPlanner),
        )
        .route(utterance)
        .await?
        {
            RouteOutcome::Plan(plan) => Ok(plan),
            RouteOutcome::Clarify {
                question,
                candidate_ids,
            } => Err(format!("{question} candidates={candidate_ids:?}")),
            RouteOutcome::Control(control) => Err(format!(
                "system control `{control:?}` cannot start a new agent run"
            )),
        }
    }
}

struct NoFallbackPlanner;

#[async_trait]
impl PlannerBackend for NoFallbackPlanner {
    async fn plan(
        &self,
        _utterance: &str,
        _candidates: Vec<RankedCandidate>,
    ) -> Result<AgentPlan, String> {
        Err("no deterministic capability match; configure an AI planner or supply a plan".into())
    }
}

struct DesktopMcpProfileResolver {
    repo: Repo,
    home: PathBuf,
}

impl McpProfileResolver for DesktopMcpProfileResolver {
    fn resolve(&self, server: &str) -> Result<McpConnectionProfile, InvocationError> {
        let row = self
            .repo
            .get_mcp_server(server)
            .map_err(|error| InvocationError::Unavailable(error.to_string()))?
            .ok_or_else(|| {
                InvocationError::Unavailable(format!("unknown MCP server `{server}`"))
            })?;
        if !row.enabled {
            return Err(InvocationError::Unavailable(format!(
                "MCP server `{server}` is disabled"
            )));
        }
        let identity =
            mcp_commands::load_vault_identity(&self.home).map_err(InvocationError::Unavailable)?;
        let transport = mcp_commands::runtime_transport(&self.repo, identity.as_ref(), &row)
            .map_err(InvocationError::Unavailable)?;
        Ok(McpConnectionProfile { transport })
    }
}

fn parse_risk(value: &str) -> Result<RiskLevel, String> {
    match value.trim().to_ascii_uppercase().as_str() {
        "L0" => Ok(RiskLevel::L0),
        "L1" => Ok(RiskLevel::L1),
        "L2" => Ok(RiskLevel::L2),
        "L3" => Ok(RiskLevel::L3),
        _ => Err(format!("invalid MCP risk level `{value}`")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use lumo_agent::{
        AdapterRegistry, AgentPlan, AgentProfile, AgentProfileDraft, CapabilityCatalog,
        CapabilityDescriptor, CapabilityKind, CapabilitySource, InvocationAdapter,
        InvocationContext, InvocationError, InvocationRequest, InvocationResult, PlanNode,
        RiskLevel,
    };
    use lumo_storage::Repo;
    use serde_json::json;
    use std::{collections::BTreeMap, sync::Arc};

    fn capability(id: &str, alias: &str) -> CapabilityDescriptor {
        let mut descriptor = CapabilityDescriptor {
            id: id.into(),
            source: CapabilitySource::Flow {
                path: format!("/{id}.lumoflow.yaml"),
            },
            name: id.into(),
            description: String::new(),
            input_schema: json!({"type": "object"}),
            output_schema: None,
            aliases: vec![alias.into()],
            examples: Vec::new(),
            risk: RiskLevel::L0,
            enabled: true,
            version_hash: String::new(),
        };
        descriptor.refresh_version_hash();
        descriptor
    }

    struct FakeAdapter;

    #[async_trait]
    impl InvocationAdapter for FakeAdapter {
        fn source_kind(&self) -> CapabilityKind {
            CapabilityKind::Flow
        }

        async fn invoke(
            &self,
            _request: InvocationRequest,
            context: InvocationContext,
        ) -> Result<InvocationResult, InvocationError> {
            Ok(InvocationResult {
                output: json!({"node": context.node_id}),
                duration_ms: 1,
                tokens_used: 0,
                cost_usd_micro: 0,
                metadata: BTreeMap::new(),
            })
        }
    }

    struct FakeFactory {
        catalog: CapabilityCatalog,
        profile: AgentProfile,
    }

    impl FakeFactory {
        fn new() -> Self {
            let catalog = CapabilityCatalog::new(vec![
                capability("flow:daily-report", "运行日报"),
                capability("flow:archive-report", "归档日报"),
            ])
            .unwrap();
            let profile = AgentProfile::validate(AgentProfileDraft::default(), &catalog).unwrap();
            Self { catalog, profile }
        }
    }

    #[async_trait]
    impl DesktopAgentFactory for FakeFactory {
        async fn catalog(&self, _profile_id: Option<&str>) -> Result<CapabilityCatalog, String> {
            Ok(self.catalog.clone())
        }

        async fn profile(&self, _profile_id: Option<&str>) -> Result<AgentProfile, String> {
            Ok(self.profile.clone())
        }

        async fn adapters(&self, _profile: &AgentProfile) -> Result<AdapterRegistry, String> {
            let mut adapters = AdapterRegistry::default();
            adapters.register(Arc::new(FakeAdapter));
            Ok(adapters)
        }

        async fn plan(
            &self,
            utterance: &str,
            catalog: &CapabilityCatalog,
            _profile: &AgentProfile,
        ) -> Result<AgentPlan, String> {
            let exact = catalog.exact_alias(utterance);
            assert_eq!(exact.len(), 1);
            assert_eq!(exact[0].id, "flow:daily-report");
            Ok(AgentPlan::new(
                "factory-plan",
                utterance,
                vec![
                    PlanNode {
                        id: "generate".into(),
                        depends_on: Vec::new(),
                        capability_id: "flow:daily-report".into(),
                        arguments: json!({}),
                        risk: RiskLevel::L0,
                        timeout_ms: 1_000,
                        retry_limit: 0,
                        expected_output_schema: None,
                    },
                    PlanNode {
                        id: "archive".into(),
                        depends_on: vec!["generate".into()],
                        capability_id: "flow:archive-report".into(),
                        arguments: json!({}),
                        risk: RiskLevel::L0,
                        timeout_ms: 1_000,
                        retry_limit: 0,
                        expected_output_schema: None,
                    },
                ],
            ))
        }
    }

    #[tokio::test]
    async fn exact_alias_executes_two_node_plan_and_persists_completion_after_node_events() {
        let repo = Repo::open_in_memory().unwrap();
        let service = DesktopAgentService::new(
            Arc::new(FakeFactory::new()),
            repo.clone(),
            Arc::new(NoopDesktopAgentEventEmitter),
        );

        let execution = service
            .start(AgentStartInput {
                utterance: "运行日报".into(),
                profile_id: None,
                supplied_plan: None,
            })
            .await
            .unwrap();
        let run_id = execution.run_id().to_string();
        let report = execution.wait().await.unwrap();

        assert_eq!(report.status, lumo_agent::LoopStatus::Completed);
        let events = repo.list_agent_events(&run_id, 0).unwrap();
        let kinds = events
            .iter()
            .map(|event| event.kind.as_str())
            .collect::<Vec<_>>();
        let last_node = kinds
            .iter()
            .rposition(|kind| *kind == "node.completed")
            .unwrap();
        let terminal = kinds
            .iter()
            .position(|kind| *kind == "run.completed")
            .unwrap();
        assert!(last_node < terminal);
        assert_eq!(
            kinds,
            [
                "session.started",
                "plan.created",
                "run.started",
                "node.queued",
                "node.queued",
                "node.started",
                "node.completed",
                "node.started",
                "node.completed",
                "run.completed",
            ]
        );
        let (state, finished_at, error): (String, Option<i64>, Option<String>) = repo
            .with_raw(|connection| {
                connection.query_row(
                    "SELECT state, finished_at, error FROM agent_runs WHERE id=?",
                    [&run_id],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                )
            })
            .unwrap();
        assert_eq!(state, "completed");
        assert!(finished_at.is_some());
        assert!(error.is_none());
    }
}
