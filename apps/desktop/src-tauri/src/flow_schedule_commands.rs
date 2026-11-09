//! Deterministic scheduling of a saved flow, using the same catalog and
//! permission checks as interactive Agent execution.
use super::{
    agent_service::{DesktopAgentFactory, ProductionDesktopAgentFactory},
    job_commands::{prepare_new_job, JobScheduleRequest},
    open_repo,
    paths::{flow_read_roots, resolve_within},
    AppHandle,
};
use chrono::{DateTime, Utc};
use lumo_agent::{AgentPlan, CapabilityDescriptor, CapabilitySource, JobSchedule, PlanNode};
use lumo_storage::{AgentJobRow, EnqueueJobResult};
use serde::Deserialize;
use serde_json::{json, Value};
use tauri::Emitter;

fn latest_job_run(repo: &lumo_storage::Repo, job_id: &str) -> Result<Option<String>, String> {
    use rusqlite::OptionalExtension;
    let job = repo
        .get_job(job_id)
        .map_err(|error| error.to_string())?
        .ok_or("任务不存在")?;
    if job.run_id.is_some() {
        return Ok(job.run_id);
    }
    repo.with_raw(|connection| connection.query_row(
        "SELECT id FROM agent_runs WHERE json_extract(plan_json,'$.metadata.jobId')=? ORDER BY started_at DESC,id DESC LIMIT 1",[job_id],|row|row.get(0),
    ).optional()).map_err(|error|error.to_string())
}

#[tauri::command]
pub(super) fn job_latest_run(app: AppHandle, job_id: String) -> Result<Option<String>, String> {
    latest_job_run(&open_repo(&app)?, &job_id)
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct FlowScheduleRequest {
    path: String,
    inputs: Value,
    schedule: JobSchedule,
    #[serde(default)]
    profile_id: Option<String>,
    #[serde(default = "default_attempts")]
    max_attempts: i64,
    #[serde(default = "default_timeout")]
    timeout_ms: u64,
    #[serde(default = "default_priority")]
    priority: i64,
    #[serde(default)]
    idempotency_key: Option<String>,
}
fn default_attempts() -> i64 {
    3
}
fn default_timeout() -> u64 {
    300_000
}
fn default_priority() -> i64 {
    5
}

fn flow_request(
    request: FlowScheduleRequest,
    capability: &CapabilityDescriptor,
    now: DateTime<Utc>,
) -> Result<JobScheduleRequest, String> {
    let CapabilitySource::Flow { path } = &capability.source else {
        return Err("only saved flows can be scheduled here".into());
    };
    if path != &request.path || !capability.enabled {
        return Err("flow is unavailable in the current catalog".into());
    }
    if !request.inputs.is_object() {
        return Err("flow inputs must be a JSON object".into());
    }
    lumo_core::schema::validate_input(&capability.input_schema, &request.inputs)?;
    if !(1..=20).contains(&request.max_attempts)
        || !(1_000..=3_600_000).contains(&request.timeout_ms)
        || !(-100..=100).contains(&request.priority)
    {
        return Err(
            "attempts must be 1..20, timeout 1..3600 seconds and priority -100..100".into(),
        );
    }
    match &request.schedule {
        JobSchedule::OneShot { run_at } if *run_at <= now => {
            return Err("one-shot run time must be in the future".into())
        }
        JobSchedule::Cron { .. }
            if request
                .schedule
                .next_after(now)
                .map_err(|error| error.to_string())?
                .is_none() =>
        {
            return Err("cron schedule has no future occurrence".into())
        }
        _ => {}
    }
    let objective = format!("运行流程 {}", capability.name);
    let mut plan = AgentPlan::new(
        format!("scheduled-{}", ulid::Ulid::new()),
        &objective,
        vec![PlanNode {
            id: "flow".into(),
            depends_on: Vec::new(),
            capability_id: capability.id.clone(),
            arguments: request.inputs,
            risk: capability.risk,
            timeout_ms: request.timeout_ms,
            // Uncertain side effects are reviewed by the durable Job layer.
            retry_limit: 0,
            expected_output_schema: capability.output_schema.clone(),
        }],
    );
    plan.metadata.insert("flowPath".into(), json!(path));
    plan.metadata
        .insert("flowName".into(), json!(capability.name));
    plan.metadata.insert(
        "capabilityVersions".into(),
        json!({&capability.id: &capability.version_hash}),
    );
    Ok(JobScheduleRequest {
        utterance: objective,
        profile_id: request.profile_id,
        plan,
        schedule: request.schedule,
        idempotency_key: request.idempotency_key,
        priority: request.priority,
        max_attempts: request.max_attempts,
    })
}

fn flow_request_with_source(
    mut request: FlowScheduleRequest,
    capability: &CapabilityDescriptor,
    source: &str,
    now: DateTime<Utc>,
) -> Result<JobScheduleRequest, String> {
    if !capability.source_content_matches(source.as_bytes()) {
        return Err("流程内容已变化，请刷新后重新设置定时任务".into());
    }
    let flow = lumo_dsl::parse_str(source).map_err(|error| error.to_string())?;
    lumo_dsl::validate(&flow).map_err(|error| error.to_string())?;
    if !request.inputs.is_object() {
        return Err("flow inputs must be a JSON object".into());
    }
    request.inputs = lumo_core::vm::merge_input_defaults(&flow.spec.inputs, request.inputs)
        .map_err(|error| error.to_string())?;
    flow_request(request, capability, now)
}

async fn prepare_flow(
    app: &AppHandle,
    mut request: FlowScheduleRequest,
) -> Result<lumo_storage::NewAgentJob, String> {
    let path = resolve_within(&request.path, &flow_read_roots(app))?;
    request.path = path.to_string_lossy().into_owned();
    let factory = ProductionDesktopAgentFactory::new(app.clone(), open_repo(app)?)?;
    let profile = factory.profile(request.profile_id.as_deref()).await?;
    let catalog = factory.catalog(request.profile_id.as_deref()).await?;
    let capability = catalog.all().into_iter().find(|capability| matches!(&capability.source, CapabilitySource::Flow { path } if path == &request.path))
        .ok_or("saved flow is unavailable or has a duplicate flow ID; use a unique ID")?;
    if !profile.visible_capabilities.is_empty()
        && !profile.visible_capabilities.contains(&capability.id)
    {
        return Err("flow is not visible to this Agent profile".into());
    }
    let now = Utc::now();
    let source = std::fs::read_to_string(&request.path).map_err(|error| error.to_string())?;
    let request = flow_request_with_source(request, &capability, &source, now)?;
    lumo_agent::PlanValidator::new(&catalog, &profile)
        .validate(request.plan.clone())
        .map_err(|error| error.to_string())?;
    prepare_new_job(request, now)
}

#[tauri::command]
pub(super) async fn job_schedule_flow(
    app: AppHandle,
    request: FlowScheduleRequest,
) -> Result<EnqueueJobResult, String> {
    let job = prepare_flow(&app, request).await?;
    let result = open_repo(&app)?
        .enqueue_job(&job)
        .map_err(|error| error.to_string())?;
    let _ = app.emit("lumo://job-updated", &result.job);
    Ok(result)
}

#[tauri::command]
pub(super) async fn job_update_schedule(
    app: AppHandle,
    job_id: String,
    expected_updated_at: DateTime<Utc>,
    request: FlowScheduleRequest,
) -> Result<AgentJobRow, String> {
    let replacement = prepare_flow(&app, request).await?;
    let repo = open_repo(&app)?;
    if !repo
        .update_job_schedule(&job_id, expected_updated_at, &replacement, Utc::now())
        .map_err(|error| error.to_string())?
    {
        return Err("任务已变化或已开始执行，请刷新；仅可编辑尚未开始的排队/暂停任务".into());
    }
    let job = repo
        .get_job(&job_id)
        .map_err(|error| error.to_string())?
        .ok_or("job disappeared")?;
    let _ = app.emit("lumo://job-updated", &job);
    Ok(job)
}

#[cfg(test)]
mod tests {
    use super::*;
    use lumo_agent::RiskLevel;
    fn fixture() -> (FlowScheduleRequest, CapabilityDescriptor) {
        let mut capability = CapabilityDescriptor {
            id: "flow:daily".into(),
            source: CapabilitySource::Flow {
                path: "/flows/daily.yaml".into(),
            },
            name: "Daily".into(),
            description: String::new(),
            input_schema: json!({"type":"object","required":["day"],"properties":{"day":{"type":"string"}}}),
            output_schema: None,
            aliases: Vec::new(),
            examples: Vec::new(),
            risk: RiskLevel::L2,
            enabled: true,
            version_hash: String::new(),
        };
        capability.refresh_version_hash();
        let request = FlowScheduleRequest {
            path: "/flows/daily.yaml".into(),
            inputs: json!({"day":"today"}),
            schedule: JobSchedule::cron_in(
                "0 0 9 * * *",
                Some("Asia/Shanghai".into()),
                Default::default(),
            )
            .unwrap(),
            profile_id: None,
            max_attempts: 4,
            timeout_ms: 60_000,
            priority: 5,
            idempotency_key: None,
        };
        (request, capability)
    }
    #[test]
    fn scheduled_flow_keeps_inputs_risk_timezone_and_retries() {
        let (request, capability) = fixture();
        let now = Utc::now();
        let prepared =
            prepare_new_job(flow_request(request, &capability, now).unwrap(), now).unwrap();
        assert_eq!(prepared.max_attempts, 4);
        assert_eq!(prepared.schedule_spec["timezone"], "Asia/Shanghai");
        assert_eq!(
            prepared.payload["suppliedPlan"]["nodes"][0]["arguments"],
            json!({"day":"today"})
        );
        assert_eq!(prepared.payload["suppliedPlan"]["nodes"][0]["risk"], "L2");
        assert_eq!(
            prepared.payload["suppliedPlan"]["metadata"]["flowPath"],
            "/flows/daily.yaml"
        );
    }
    #[test]
    fn malformed_inputs_invalid_schedule_and_unsafe_retry_settings_are_rejected() {
        let (mut request, capability) = fixture();
        request.inputs = json!({"day":42});
        assert!(flow_request(request.clone(), &capability, Utc::now()).is_err());
        request.inputs = json!({"day":"today"});
        request.max_attempts = 21;
        assert!(flow_request(request.clone(), &capability, Utc::now()).is_err());
        request.max_attempts = 3;
        request.schedule = JobSchedule::one_shot(Utc::now() - chrono::Duration::seconds(1));
        assert!(flow_request(request, &capability, Utc::now()).is_err());
    }
    #[test]
    fn saved_flow_inputs_are_checked_and_defaults_frozen_before_enqueue() {
        let (mut request, mut capability) = fixture();
        capability.input_schema = json!({"type":"object"});
        let source = "apiVersion: lumorpa.io/v1\nkind: Flow\nmetadata: {id: daily}\nspec:\n  inputs:\n    - {name: day, type: string, required: true}\n    - {name: count, type: integer, default: 3}\n  steps: []\n";
        capability.bind_source_content(source.as_bytes());
        request.inputs = json!({});
        assert!(
            flow_request_with_source(request.clone(), &capability, source, Utc::now()).is_err()
        );
        request.inputs = json!({"day":42});
        assert!(
            flow_request_with_source(request.clone(), &capability, source, Utc::now()).is_err()
        );
        request.inputs = json!({"day":"today"});
        let prepared = flow_request_with_source(request, &capability, source, Utc::now()).unwrap();
        assert_eq!(
            prepared.plan.nodes[0].arguments,
            json!({"day":"today","count":3})
        );
        assert_eq!(
            prepared.plan.metadata["capabilityVersions"]["flow:daily"],
            capability.version_hash
        );
    }
    #[test]
    fn recurring_result_link_survives_next_occurrence_and_is_scoped_to_the_job() {
        let repo = lumo_storage::Repo::open_in_memory().unwrap();
        let (request, capability) = fixture();
        let now = Utc::now();
        let new = prepare_new_job(flow_request(request, &capability, now).unwrap(), now).unwrap();
        repo.enqueue_job(&new).unwrap();
        assert_eq!(latest_job_run(&repo, &new.id).unwrap(), None);
        repo.create_agent_run(&lumo_storage::AgentRunRow {
            id: "previous".into(),
            profile_id: None,
            utterance: None,
            plan_json: Some(json!({"metadata":{"jobId":new.id}})),
            approval_json: None,
            state: "completed".into(),
            started_at: now,
            finished_at: Some(now),
            error: None,
        })
        .unwrap();
        assert_eq!(
            latest_job_run(&repo, &new.id).unwrap(),
            Some("previous".into())
        );
        assert!(latest_job_run(&repo, "unrelated").is_err());
    }
}
