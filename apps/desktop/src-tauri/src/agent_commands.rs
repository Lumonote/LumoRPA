use super::agent_service::{
    AgentStartInput, DesktopAgentService, ProductionDesktopAgentFactory,
    TauriDesktopAgentEventEmitter,
};
use super::{open_repo, DesktopState};
use lumo_agent::{AgentEventKind, AgentPlan, RiskLevel};
use lumo_core::CancelToken;
use lumo_storage::{AgentEventInsert, AgentEventRow, Repo};
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
    Paused,
    Cancelled,
    Completed,
    Failed,
}

struct AgentSession {
    _plan: AgentPlan,
    state: AgentRuntimeState,
    cancel: CancelToken,
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

#[tauri::command]
pub(super) async fn agent_start(
    state: State<'_, DesktopState>,
    input: AgentStartInput,
) -> Result<AgentSessionDto, String> {
    start_agent_request(&state, input).await
}

pub(super) fn setup_agent_service(app: &AppHandle) -> Result<(), String> {
    let repo = open_repo(app)?;
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
    let (run_id, plan, cancel) = execution.into_session_parts();
    let mut runtime = lock_runtime(state)?;
    start_session(&mut runtime, run_id, plan, cancel)
}

#[tauri::command]
pub(super) fn agent_pause(
    app: AppHandle,
    state: State<'_, DesktopState>,
    run_id: String,
) -> Result<AgentSessionDto, String> {
    let mut runtime = lock_runtime(&state)?;
    let result = pause_session(&mut runtime, &run_id)?;
    append_event(
        &app,
        &open_repo(&app)?,
        &run_id,
        AgentEventKind::RunPaused,
        None,
        json!({}),
    )?;
    Ok(result)
}

#[tauri::command]
pub(super) fn agent_resume(
    app: AppHandle,
    state: State<'_, DesktopState>,
    run_id: String,
) -> Result<AgentSessionDto, String> {
    let mut runtime = lock_runtime(&state)?;
    let result = resume_session(&mut runtime, &run_id)?;
    append_event(
        &app,
        &open_repo(&app)?,
        &run_id,
        AgentEventKind::RunResumed,
        None,
        json!({}),
    )?;
    Ok(result)
}

#[tauri::command]
pub(super) fn agent_cancel(
    app: AppHandle,
    state: State<'_, DesktopState>,
    run_id: String,
) -> Result<AgentSessionDto, String> {
    let mut runtime = lock_runtime(&state)?;
    let result = cancel_session(&mut runtime, &run_id)?;
    append_event(
        &app,
        &open_repo(&app)?,
        &run_id,
        AgentEventKind::RunCancelled,
        None,
        json!({}),
    )?;
    Ok(result)
}

#[tauri::command]
pub(super) fn agent_approve(
    app: AppHandle,
    state: State<'_, DesktopState>,
    run_id: String,
    approved: bool,
    node_ids: Vec<String>,
) -> Result<AgentSessionDto, String> {
    let mut runtime = lock_runtime(&state)?;
    let result = approve_session(&mut runtime, &run_id, approved, node_ids.clone())?;
    append_event(
        &app,
        &open_repo(&app)?,
        &run_id,
        AgentEventKind::PermissionResolved,
        None,
        json!({ "approved": approved, "nodeIds": node_ids }),
    )?;
    Ok(result)
}

#[tauri::command]
pub(super) fn agent_events(
    app: AppHandle,
    run_id: String,
    after_seq: Option<i64>,
) -> Result<Vec<AgentEventRow>, String> {
    open_repo(&app)?
        .list_agent_events(&run_id, after_seq.unwrap_or(0))
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub(super) fn agent_restore(
    app: AppHandle,
    state: State<'_, DesktopState>,
    run_id: String,
) -> Result<AgentSessionDto, String> {
    let repo = open_repo(&app)?;
    let events = repo
        .list_agent_events(&run_id, 0)
        .map_err(|error| error.to_string())?;
    let restored = restore_from_events(&run_id, &events)?;
    for node_id in &restored.unknown_nodes {
        append_event(
            &app,
            &repo,
            &run_id,
            AgentEventKind::NodeFailed,
            Some(node_id),
            json!({
                "state": "unknown",
                "reason": "desktop host restarted during an external side-effect node"
            }),
        )?;
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
) -> Result<AgentSessionDto, String> {
    if runtime.sessions.contains_key(&run_id) {
        return Err(format!("agent run `{run_id}` already exists"));
    }
    let session = AgentSession {
        _plan: plan,
        state: AgentRuntimeState::Running,
        cancel,
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
    if session.state != AgentRuntimeState::Running {
        return Err(format!("agent run `{run_id}` is not running"));
    }
    session.state = AgentRuntimeState::Paused;
    Ok(session_dto(run_id, session))
}

fn resume_session(
    runtime: &mut DesktopAgentRuntime,
    run_id: &str,
) -> Result<AgentSessionDto, String> {
    let session = require_session(runtime, run_id)?;
    if session.state != AgentRuntimeState::Paused {
        return Err(format!("agent run `{run_id}` is not paused"));
    }
    session.state = AgentRuntimeState::Running;
    Ok(session_dto(run_id, session))
}

fn cancel_session(
    runtime: &mut DesktopAgentRuntime,
    run_id: &str,
) -> Result<AgentSessionDto, String> {
    let session = require_session(runtime, run_id)?;
    session.cancel.cancel();
    session.state = AgentRuntimeState::Cancelled;
    Ok(session_dto(run_id, session))
}

fn approve_session(
    runtime: &mut DesktopAgentRuntime,
    run_id: &str,
    approved: bool,
    node_ids: Vec<String>,
) -> Result<AgentSessionDto, String> {
    let session = require_session(runtime, run_id)?;
    session.approval = Some(json!({ "approved": approved, "nodeIds": node_ids }));
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

fn session_dto(run_id: &str, session: &AgentSession) -> AgentSessionDto {
    AgentSessionDto {
        run_id: run_id.to_string(),
        state: session.state,
        paused: session.state == AgentRuntimeState::Paused,
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

fn append_event(
    app: &AppHandle,
    repo: &Repo,
    run_id: &str,
    kind: AgentEventKind,
    node_id: Option<&str>,
    payload: Value,
) -> Result<(), String> {
    let seq = repo
        .list_agent_events(run_id, 0)
        .map_err(|error| error.to_string())?
        .last()
        .map_or(1, |event| event.seq + 1);
    repo.append_agent_event(AgentEventInsert {
        run_id,
        seq,
        kind: kind.as_str(),
        node_id,
        parent_node_id: None,
        payload: &payload,
    })
    .map_err(|error| error.to_string())?;
    let _ = app.emit(
        "lumo://agent-event",
        json!({ "runId": run_id, "seq": seq, "kind": kind.as_str(), "nodeId": node_id, "payload": payload }),
    );
    Ok(())
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
        let started = start_session(
            &mut runtime,
            "run-1".into(),
            plan(RiskLevel::L1),
            CancelToken::new(),
        )
        .unwrap();
        assert_eq!(started.state, AgentRuntimeState::Running);

        pause_session(&mut runtime, "run-1").unwrap();
        assert_eq!(runtime.sessions["run-1"].state, AgentRuntimeState::Paused);
        approve_session(&mut runtime, "run-1", true, vec!["send".into()]).unwrap();
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
}
