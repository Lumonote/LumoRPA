use super::agent_service::{
    AgentStartInput, DesktopAgentService, ProductionDesktopAgentFactory,
    TauriDesktopAgentEventEmitter,
};
use super::{open_repo, DesktopState};
use lumo_agent::{
    AgentControlState, AgentEventDraft, AgentEventKind, AgentPlan, AgentRunControl, LoopStatus,
    RiskLevel,
};
use lumo_core::CancelToken;
use lumo_storage::{AgentEventRow, Repo};
use serde::Serialize;
use serde_json::{json, Value};
use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};
use tauri::{Emitter, Listener, Manager, State, Wry};

type AppHandle = tauri::AppHandle<Wry>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) enum AgentRuntimeState {
    Running,
    WaitingApproval,
    Paused,
    Cancelled,
    Completed,
    Failed,
}

struct AgentSession {
    _plan: AgentPlan,
    state: AgentRuntimeState,
    cancel: CancelToken,
    control: Option<AgentRunControl>,
    approval: Option<Value>,
    unknown_nodes: Vec<String>,
}

#[derive(Default)]
pub(super) struct DesktopAgentRuntime {
    sessions: HashMap<String, AgentSession>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct AgentSessionDto {
    run_id: String,
    state: AgentRuntimeState,
    paused: bool,
    approval: Option<Value>,
    unknown_nodes: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct AgentRunSummaryDto {
    id: String,
    profile_id: Option<String>,
    utterance: Option<String>,
    state: String,
    started_at: i64,
    finished_at: Option<i64>,
    error: Option<String>,
}

#[tauri::command]
pub(super) async fn agent_start(
    state: State<'_, DesktopState>,
    input: AgentStartInput,
) -> Result<AgentSessionDto, String> {
    start_agent_request(&state, input).await
}

pub(super) fn setup_agent_service(app: &AppHandle) -> Result<(), String> {
    let repo = open_repo(app)?;
    // Called once before this process has accepted any Agent sessions.
    repo.recover_interrupted_invocations()
        .map_err(|e| e.to_string())?;
    let factory = Arc::new(ProductionDesktopAgentFactory::new(
        app.clone(),
        repo.clone(),
    )?);
    let emitter = Arc::new(TauriDesktopAgentEventEmitter::new(app.clone()));
    let service = Arc::new(DesktopAgentService::new(factory, repo, emitter));
    let state = app.state::<DesktopState>();
    *state
        .agent_service
        .lock()
        .map_err(|_| "desktop agent service is unavailable".to_string())? = Some(service);

    let listener_app = app.clone();
    app.listen("lumo://agent-start-request", move |event| {
        let input = serde_json::from_str::<AgentStartInput>(event.payload());
        let app = listener_app.clone();
        tauri::async_runtime::spawn(async move {
            let result = match input {
                Ok(input) => {
                    let state = app.state::<DesktopState>();
                    start_agent_request(&state, input).await.map(|_| ())
                }
                Err(error) => Err(format!("invalid agent start request: {error}")),
            };
            if let Err(error) = result {
                super::voice_commands::report_agent_start_failure(&app, &error);
                let _ = app.emit("lumo://agent-start-failed", json!({ "error": error }));
            }
        });
    });
    Ok(())
}

async fn start_agent_request(
    state: &DesktopState,
    input: AgentStartInput,
) -> Result<AgentSessionDto, String> {
    let service = state
        .agent_service
        .lock()
        .map_err(|_| "desktop agent service is unavailable".to_string())?
        .clone()
        .ok_or_else(|| "desktop agent service is not initialized".to_string())?;
    let execution = service.start(input).await?;
    let (run_id, plan, cancel, control) = execution.into_session_parts();
    let mut runtime = lock_runtime(state)?;
    start_session(&mut runtime, run_id, plan, cancel, control)
}

#[tauri::command]
pub(super) async fn agent_pause(
    app: AppHandle,
    state: State<'_, DesktopState>,
    run_id: String,
) -> Result<AgentSessionDto, String> {
    if let Some(result) = control_background_run(&app, &state, &run_id, "pause").await? {
        return Ok(result);
    }
    let result = {
        let mut runtime = lock_runtime(&state)?;
        pause_session(&mut runtime, &run_id)?
    };
    append_event(
        &state,
        &run_id,
        AgentEventKind::RunPaused,
        None,
        json!({"scope":"subsequent_nodes"}),
    )
    .await?;
    Ok(result)
}

#[tauri::command]
pub(super) async fn agent_resume(
    app: AppHandle,
    state: State<'_, DesktopState>,
    run_id: String,
) -> Result<AgentSessionDto, String> {
    if let Some(result) = control_background_run(&app, &state, &run_id, "resume").await? {
        return Ok(result);
    }
    let result = {
        let mut runtime = lock_runtime(&state)?;
        resume_session(&mut runtime, &run_id)?
    };
    append_event(
        &state,
        &run_id,
        AgentEventKind::RunResumed,
        None,
        json!({"state":result.state}),
    )
    .await?;
    Ok(result)
}

#[tauri::command]
pub(super) async fn agent_cancel(
    app: AppHandle,
    state: State<'_, DesktopState>,
    run_id: String,
) -> Result<AgentSessionDto, String> {
    if let Some(result) = control_background_run(&app, &state, &run_id, "cancel").await? {
        return Ok(result);
    }
    let (result, restored) = {
        let mut runtime = lock_runtime(&state)?;
        let restored = runtime
            .sessions
            .get(&run_id)
            .is_some_and(|session| session.control.is_none());
        (cancel_session(&mut runtime, &run_id)?, restored)
    };
    // Live execution publishes its terminal event after cancellation/cleanup.
    if restored {
        append_event(
            &state,
            &run_id,
            AgentEventKind::RunCancelled,
            None,
            json!({}),
        )
        .await?;
    }
    Ok(result)
}

async fn control_background_run(
    app: &AppHandle,
    state: &DesktopState,
    run_id: &str,
    action: &str,
) -> Result<Option<AgentSessionDto>, String> {
    let service = state
        .agent_service
        .lock()
        .map_err(|_| "desktop agent service is unavailable")?
        .clone()
        .ok_or("desktop agent service is not initialized")?;
    if !service.is_background_job(run_id)? {
        return Ok(None);
    }
    let job = service
        .control_background_job(state, run_id, action)
        .await?
        .ok_or("后台任务已变化、已结束或仍待审批，请刷新；待审批任务须先完成审批")?;
    let _ = app.emit("lumo://job-updated", &job);
    let state = match action {
        "resume" => AgentRuntimeState::Running,
        "cancel" => AgentRuntimeState::Cancelled,
        _ => AgentRuntimeState::Paused,
    };
    Ok(Some(AgentSessionDto {
        run_id: run_id.into(),
        state,
        paused: state == AgentRuntimeState::Paused,
        approval: None,
        unknown_nodes: Vec::new(),
    }))
}

/// 语音状态播报用：正在运行/暂停中的 agent 会话数。
pub(super) fn active_agent_session_count(state: &DesktopState) -> usize {
    lock_runtime(state)
        .map(|runtime| {
            runtime
                .sessions
                .values()
                .filter(|session| {
                    matches!(
                        session_state(session),
                        AgentRuntimeState::Running
                            | AgentRuntimeState::Paused
                            | AgentRuntimeState::WaitingApproval
                    )
                })
                .count()
        })
        .unwrap_or(0)
}

pub(super) fn cancel_active_agent_sessions(
    _app: &AppHandle,
    state: &DesktopState,
) -> Result<Vec<String>, String> {
    let (ids, restored_ids) = {
        let mut runtime = lock_runtime(state)?;
        let ids = runtime
            .sessions
            .iter()
            .filter_map(|(id, session)| {
                matches!(
                    session_state(session),
                    AgentRuntimeState::Running
                        | AgentRuntimeState::Paused
                        | AgentRuntimeState::WaitingApproval
                )
                .then_some(id.clone())
            })
            .collect::<Vec<_>>();
        let restored_ids = ids
            .iter()
            .filter(|id| runtime.sessions[*id].control.is_none())
            .cloned()
            .collect::<Vec<_>>();
        for id in &ids {
            cancel_session(&mut runtime, id)?;
        }
        (ids, restored_ids)
    };
    if !restored_ids.is_empty() {
        let service = state
            .agent_service
            .lock()
            .map_err(|_| "desktop agent service is unavailable")?
            .clone()
            .ok_or("desktop agent service is not initialized")?;
        tauri::async_runtime::spawn(async move {
            for run_id in restored_ids {
                if let Err(error) = service
                    .publish_event(
                        AgentEventDraft::new(run_id, AgentEventKind::RunCancelled)
                            .payload(json!({"source":"voice_stop"})),
                    )
                    .await
                {
                    tracing::warn!(%error, "failed to persist cancellation of restored agent run");
                }
            }
        });
    }
    Ok(ids)
}

#[tauri::command]
pub(super) async fn agent_approve(
    state: State<'_, DesktopState>,
    run_id: String,
    approved: bool,
    node_ids: Vec<String>,
    plan_hash: String,
    strengthened: Option<bool>,
) -> Result<AgentSessionDto, String> {
    let service = state
        .agent_service
        .lock()
        .map_err(|_| "desktop agent service is unavailable")?
        .clone()
        .ok_or("desktop agent service is not initialized")?;
    if service.is_background_job(&run_id)? {
        service
            .resolve_job_approval(
                &run_id,
                &plan_hash,
                &node_ids,
                approved,
                strengthened.unwrap_or(false),
            )
            .await?;
        return Ok(AgentSessionDto {
            run_id,
            state: if approved {
                AgentRuntimeState::WaitingApproval
            } else {
                AgentRuntimeState::Cancelled
            },
            paused: false,
            approval: Some(json!({"approved":approved,"nodeIds":node_ids,"planHash":plan_hash})),
            unknown_nodes: vec![],
        });
    }
    let control = {
        let mut runtime = lock_runtime(&state)?;
        let session = require_session(&mut runtime, &run_id)?;
        session
            .control
            .clone()
            .ok_or("restored runs cannot resume automatically; start a new run")?
    };
    let service = state
        .agent_service
        .lock()
        .map_err(|_| "desktop agent service is unavailable")?
        .clone()
        .ok_or("desktop agent service is not initialized")?;
    service
        .resolve_approval(
            &run_id,
            &control,
            &plan_hash,
            &node_ids,
            approved,
            strengthened.unwrap_or(false),
        )
        .await?;
    let mut runtime = lock_runtime(&state)?;
    let session = require_session(&mut runtime, &run_id)?;
    if !approved {
        session.cancel.cancel();
    }
    session.approval = Some(
        json!({"approved":approved,"nodeIds":node_ids,"planHash":plan_hash,"strengthened":strengthened.unwrap_or(false)}),
    );
    Ok(session_dto(&run_id, session))
}

#[tauri::command]
pub(super) fn agent_events(
    app: AppHandle,
    run_id: String,
    after_seq: Option<i64>,
) -> Result<Vec<AgentEventRow>, String> {
    let repo = open_repo(&app)?;
    let run_id = if run_id == "latest" {
        latest_agent_run_id(&repo)?.unwrap_or(run_id)
    } else {
        run_id
    };
    repo.list_agent_events(&run_id, after_seq.unwrap_or(0))
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub(super) fn agent_usage(
    app: AppHandle,
    run_id: String,
) -> Result<Vec<lumo_storage::InvocationAttemptRow>, String> {
    let repo = open_repo(&app)?;
    repo.reconcile_invocation_usage(&run_id)
        .map_err(|e| e.to_string())?;
    repo.list_invocation_attempts(&run_id)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub(super) fn agent_resolve_usage(
    app: AppHandle,
    resolution: lumo_storage::InvocationUsageResolution,
) -> Result<(), String> {
    let repo = open_repo(&app)?;
    repo.reconcile_invocation_usage(&resolution.run_id)
        .map_err(|e| e.to_string())?;
    if !repo
        .resolve_invocation_usage(&resolution)
        .map_err(|e| e.to_string())?
    {
        return Err("用量已更新、仍在执行，或填写的总量小于已记录用量，请刷新后重试".into());
    }
    Ok(())
}

#[tauri::command]
pub(super) fn agent_run_list(
    app: AppHandle,
    limit: Option<u32>,
) -> Result<Vec<AgentRunSummaryDto>, String> {
    let limit = i64::from(limit.unwrap_or(50).clamp(1, 200));
    open_repo(&app)?.with_raw(|connection| {
        let mut statement = connection.prepare("SELECT id,profile_id,utterance,state,started_at,finished_at,error FROM agent_runs ORDER BY started_at DESC LIMIT ?")?;
        let rows = statement.query_map([limit], |row| Ok(AgentRunSummaryDto { id: row.get(0)?, profile_id: row.get(1)?, utterance: row.get(2)?, state: row.get(3)?, started_at: row.get(4)?, finished_at: row.get(5)?, error: row.get(6)? }))?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
    }).map_err(|error| error.to_string())
}

fn latest_agent_run_id(repo: &Repo) -> Result<Option<String>, String> {
    use rusqlite::OptionalExtension;
    repo.with_raw(|connection| {
        connection
            .query_row(
                "SELECT id FROM agent_runs ORDER BY started_at DESC LIMIT 1",
                [],
                |row| row.get(0),
            )
            .optional()
    })
    .map_err(|error| error.to_string())
}

#[tauri::command]
pub(super) async fn agent_restore(
    app: AppHandle,
    state: State<'_, DesktopState>,
    run_id: String,
) -> Result<AgentSessionDto, String> {
    if let Some(session) = lock_runtime(&state)?.sessions.get(&run_id) {
        return Ok(session_dto(&run_id, session));
    }
    let repo = open_repo(&app)?;
    let events = repo
        .list_agent_events(&run_id, 0)
        .map_err(|error| error.to_string())?;
    let restored = restore_from_events(&run_id, &events)?;
    for node_id in &restored.unknown_nodes {
        append_event(
            &state,
            &run_id,
            AgentEventKind::NodeFailed,
            Some(node_id),
            json!({
                "state": "unknown",
                "reason": "desktop host restarted during an external side-effect node"
            }),
        )
        .await?;
    }
    let dto = session_dto(&run_id, &restored);
    lock_runtime(&state)?.sessions.insert(run_id, restored);
    Ok(dto)
}

fn start_session(
    runtime: &mut DesktopAgentRuntime,
    run_id: String,
    plan: AgentPlan,
    cancel: CancelToken,
    control: Option<AgentRunControl>,
) -> Result<AgentSessionDto, String> {
    if runtime.sessions.contains_key(&run_id) {
        return Err(format!("agent run `{run_id}` already exists"));
    }
    let session = AgentSession {
        _plan: plan,
        state: AgentRuntimeState::Running,
        cancel,
        control,
        approval: None,
        unknown_nodes: Vec::new(),
    };
    let dto = session_dto(&run_id, &session);
    runtime.sessions.insert(run_id, session);
    Ok(dto)
}

fn pause_session(
    runtime: &mut DesktopAgentRuntime,
    run_id: &str,
) -> Result<AgentSessionDto, String> {
    let session = require_session(runtime, run_id)?;
    session
        .control
        .as_ref()
        .ok_or("restored runs cannot resume automatically; start a new run")?
        .pause()?;
    Ok(session_dto(run_id, session))
}

fn resume_session(
    runtime: &mut DesktopAgentRuntime,
    run_id: &str,
) -> Result<AgentSessionDto, String> {
    let session = require_session(runtime, run_id)?;
    session
        .control
        .as_ref()
        .ok_or("restored runs cannot resume automatically; start a new run")?
        .resume()?;
    Ok(session_dto(run_id, session))
}

fn cancel_session(
    runtime: &mut DesktopAgentRuntime,
    run_id: &str,
) -> Result<AgentSessionDto, String> {
    let session = require_session(runtime, run_id)?;
    if matches!(
        session_state(session),
        AgentRuntimeState::Completed | AgentRuntimeState::Failed | AgentRuntimeState::Cancelled
    ) {
        return Err(format!("agent run `{run_id}` has already ended"));
    }
    session.cancel.cancel();
    session.state = AgentRuntimeState::Cancelled;
    if let Some(control) = &session.control {
        control.finish(LoopStatus::Cancelled);
    }
    Ok(session_dto(run_id, session))
}

#[cfg(test)]
fn approve_session(
    runtime: &mut DesktopAgentRuntime,
    run_id: &str,
    approved: bool,
    node_ids: Vec<String>,
    plan_hash: &str,
    strengthened: bool,
) -> Result<AgentSessionDto, String> {
    let session = require_session(runtime, run_id)?;
    let control = session
        .control
        .as_ref()
        .ok_or("restored runs cannot resume automatically; start a new run")?;
    control.resolve_approval(plan_hash, &node_ids, approved, strengthened)?;
    if !approved {
        session.cancel.cancel();
    }
    session.approval = Some(
        json!({"approved":approved,"nodeIds":node_ids,"planHash":plan_hash,"strengthened":strengthened}),
    );
    Ok(session_dto(run_id, session))
}

fn restore_from_events(run_id: &str, events: &[AgentEventRow]) -> Result<AgentSession, String> {
    let plan = events
        .iter()
        .find(|event| event.kind == AgentEventKind::PlanCreated.as_str())
        .and_then(|event| event.payload.get("plan"))
        .cloned()
        .ok_or_else(|| format!("agent run `{run_id}` has no persisted plan"))?;
    let plan: AgentPlan = serde_json::from_value(plan).map_err(|error| error.to_string())?;
    let terminal = events
        .iter()
        .rev()
        .find_map(|event| match event.kind.as_str() {
            "run.completed" => Some(AgentRuntimeState::Completed),
            "run.failed" => Some(AgentRuntimeState::Failed),
            "run.cancelled" => Some(AgentRuntimeState::Cancelled),
            _ => None,
        });
    let explicitly_paused = events
        .iter()
        .rev()
        .find(|event| matches!(event.kind.as_str(), "run.paused" | "run.resumed"))
        .is_some_and(|event| event.kind == "run.paused");
    let state = terminal.unwrap_or(if explicitly_paused {
        AgentRuntimeState::Paused
    } else {
        // A process restart cannot safely continue in-flight work without a
        // durable harness session, so recovered active runs pause by default.
        AgentRuntimeState::Paused
    });

    let mut started = HashSet::new();
    let mut finished = HashSet::new();
    for event in events {
        let Some(node_id) = event.node_id.as_ref() else {
            continue;
        };
        match event.kind.as_str() {
            "node.started" => {
                started.insert(node_id.clone());
            }
            "node.completed" | "node.failed" | "node.cancelled" => {
                finished.insert(node_id.clone());
            }
            _ => {}
        }
    }
    let mut unknown_nodes = started
        .difference(&finished)
        .filter(|node_id| {
            plan.node(node_id)
                .is_some_and(|node| node.risk >= RiskLevel::L2)
        })
        .cloned()
        .collect::<Vec<_>>();
    unknown_nodes.sort();
    let approval = events
        .iter()
        .rev()
        .find(|event| event.kind == AgentEventKind::PermissionResolved.as_str())
        .map(|event| event.payload.clone());
    Ok(AgentSession {
        _plan: plan,
        state,
        cancel: CancelToken::new(),
        control: None,
        approval,
        unknown_nodes,
    })
}

fn require_session<'a>(
    runtime: &'a mut DesktopAgentRuntime,
    run_id: &str,
) -> Result<&'a mut AgentSession, String> {
    runtime
        .sessions
        .get_mut(run_id)
        .ok_or_else(|| format!("agent run `{run_id}` is not active; restore it first"))
}

fn session_state(session: &AgentSession) -> AgentRuntimeState {
    match session.control.as_ref().map(AgentRunControl::state) {
        Some(AgentControlState::Running) => AgentRuntimeState::Running,
        Some(AgentControlState::WaitingApproval) => AgentRuntimeState::WaitingApproval,
        Some(AgentControlState::Paused) => AgentRuntimeState::Paused,
        Some(AgentControlState::Completed) => AgentRuntimeState::Completed,
        Some(AgentControlState::Failed) => AgentRuntimeState::Failed,
        Some(AgentControlState::Cancelled) => AgentRuntimeState::Cancelled,
        None => session.state,
    }
}

fn session_dto(run_id: &str, session: &AgentSession) -> AgentSessionDto {
    AgentSessionDto {
        run_id: run_id.to_string(),
        state: session_state(session),
        paused: session_state(session) == AgentRuntimeState::Paused,
        approval: session.approval.clone(),
        unknown_nodes: session.unknown_nodes.clone(),
    }
}

fn lock_runtime(
    state: &DesktopState,
) -> Result<std::sync::MutexGuard<'_, DesktopAgentRuntime>, String> {
    state
        .agent
        .lock()
        .map_err(|_| "desktop agent runtime is unavailable".to_string())
}

async fn append_event(
    state: &DesktopState,
    run_id: &str,
    kind: AgentEventKind,
    node_id: Option<&str>,
    payload: Value,
) -> Result<(), String> {
    let service = state
        .agent_service
        .lock()
        .map_err(|_| "desktop agent service is unavailable")?
        .clone()
        .ok_or("desktop agent service is not initialized")?;
    let mut draft = AgentEventDraft::new(run_id, kind).payload(payload);
    if let Some(id) = node_id {
        draft = draft.node_id(id);
    }
    service.publish_event(draft).await
}

#[cfg(test)]
fn event_row(seq: i64, kind: &str, node_id: Option<&str>, payload: Value) -> AgentEventRow {
    AgentEventRow {
        run_id: "run-1".into(),
        seq,
        kind: kind.into(),
        node_id: node_id.map(str::to_string),
        parent_node_id: None,
        payload,
        created_at: chrono::Utc::now(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lumo_agent::{AgentPlan, PlanNode, RiskLevel};
    use serde_json::json;

    fn plan(risk: RiskLevel) -> AgentPlan {
        AgentPlan::new(
            "plan-1",
            "test objective",
            vec![PlanNode {
                id: "send".into(),
                depends_on: Vec::new(),
                capability_id: "email.send".into(),
                arguments: json!({}),
                risk,
                timeout_ms: 1_000,
                retry_limit: 0,
                expected_output_schema: None,
            }],
        )
    }

    #[test]
    fn runtime_start_pause_resume_approve_and_cancel() {
        let mut runtime = DesktopAgentRuntime::default();
        let control = AgentRunControl::default();
        let approval_plan = plan(RiskLevel::L1);
        control.prepare(
            &approval_plan,
            &[],
            [("send".into(), RiskLevel::L1)].into_iter().collect(),
        );
        let started = start_session(
            &mut runtime,
            "run-1".into(),
            plan(RiskLevel::L1),
            CancelToken::new(),
            Some(control.clone()),
        )
        .unwrap();
        assert_eq!(started.state, AgentRuntimeState::WaitingApproval);

        pause_session(&mut runtime, "run-1").unwrap();
        assert_eq!(
            session_state(&runtime.sessions["run-1"]),
            AgentRuntimeState::Paused
        );
        approve_session(
            &mut runtime,
            "run-1",
            true,
            vec!["send".into()],
            &approval_plan.stable_hash(),
            false,
        )
        .unwrap();
        assert!(runtime.sessions["run-1"].approval.as_ref().unwrap()["approved"] == true);
        resume_session(&mut runtime, "run-1").unwrap();
        let cancel = runtime.sessions["run-1"].cancel.clone();
        cancel_session(&mut runtime, "run-1").unwrap();
        assert!(cancel.is_cancelled());
        assert_eq!(
            runtime.sessions["run-1"].state,
            AgentRuntimeState::Cancelled
        );
    }

    #[test]
    fn restore_marks_unfinished_external_node_unknown() {
        let events = vec![
            event_row(
                1,
                "plan.created",
                None,
                json!({"plan": plan(RiskLevel::L2)}),
            ),
            event_row(2, "run.started", None, json!({})),
            event_row(3, "node.started", Some("send"), json!({})),
        ];
        let restored = restore_from_events("run-1", &events).unwrap();
        assert_eq!(restored.state, AgentRuntimeState::Paused);
        assert_eq!(restored.unknown_nodes, ["send"]);
    }

    #[test]
    fn live_session_reads_execution_terminal_state_and_rejects_further_controls() {
        for status in [
            LoopStatus::Completed,
            LoopStatus::Failed,
            LoopStatus::BudgetExceeded,
            LoopStatus::Cancelled,
        ] {
            let mut runtime = DesktopAgentRuntime::default();
            let control = AgentRunControl::default();
            start_session(
                &mut runtime,
                "run-1".into(),
                plan(RiskLevel::L0),
                CancelToken::new(),
                Some(control.clone()),
            )
            .unwrap();
            control.finish(status);
            let expected = match status {
                LoopStatus::Completed => AgentRuntimeState::Completed,
                LoopStatus::Cancelled => AgentRuntimeState::Cancelled,
                _ => AgentRuntimeState::Failed,
            };
            assert_eq!(
                session_dto("run-1", &runtime.sessions["run-1"]).state,
                expected
            );
            assert!(pause_session(&mut runtime, "run-1").is_err());
            assert!(resume_session(&mut runtime, "run-1").is_err());
            assert!(cancel_session(&mut runtime, "run-1").is_err());
        }
    }

    #[test]
    fn restored_runs_reject_resume_and_approval_without_a_live_executor() {
        let restored = restore_from_events(
            "run-1",
            &[event_row(
                1,
                "plan.created",
                None,
                json!({"plan":plan(RiskLevel::L2)}),
            )],
        )
        .unwrap();
        let mut runtime = DesktopAgentRuntime::default();
        runtime.sessions.insert("run-1".into(), restored);
        assert!(resume_session(&mut runtime, "run-1").is_err());
        assert!(approve_session(
            &mut runtime,
            "run-1",
            true,
            vec!["send".into()],
            "hash",
            false
        )
        .is_err());
        assert_eq!(
            cancel_session(&mut runtime, "run-1").unwrap().state,
            AgentRuntimeState::Cancelled
        );
    }
}
