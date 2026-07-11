use super::{agent_service::AgentStartInput, open_repo, DesktopState};
use chrono::{DateTime, Duration, Utc};
use lumo_agent::{validate_dag, AgentJob, AgentPlan, JobSchedule, LoopStatus};
use lumo_core::CancelToken;
use lumo_storage::{AgentJobRow, EnqueueJobResult, NewAgentJob, RecoveredJob, Repo};
use serde::Deserialize;
use std::{collections::HashMap, sync::Mutex, time::Duration as StdDuration};
use tauri::{Emitter, Manager, State, Wry};
use tokio_util::sync::CancellationToken;

type AppHandle = tauri::AppHandle<Wry>;

const JOB_LEASE_SECONDS: i64 = 30;
const JOB_HEARTBEAT_SECONDS: u64 = 10;
const JOB_POLL_MILLIS: u64 = 500;

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct JobScheduleRequest {
    utterance: String,
    #[serde(default)]
    profile_id: Option<String>,
    plan: AgentPlan,
    schedule: JobSchedule,
    #[serde(default)]
    idempotency_key: Option<String>,
    #[serde(default = "default_priority")]
    priority: i64,
    #[serde(default = "default_max_attempts")]
    max_attempts: i64,
}

const fn default_priority() -> i64 {
    5
}

const fn default_max_attempts() -> i64 {
    3
}

#[derive(Debug, Clone, Copy)]
enum JobControl {
    Pause,
    Resume,
    Cancel,
}

pub(super) struct JobRuntime {
    root: CancellationToken,
    worker_started: bool,
    active: HashMap<String, CancelToken>,
}

impl Default for JobRuntime {
    fn default() -> Self {
        Self {
            root: CancellationToken::new(),
            worker_started: false,
            active: HashMap::new(),
        }
    }
}

pub(super) fn setup_job_worker(app: &AppHandle) -> Result<(), String> {
    let repo = open_repo(app)?;
    let recovered = recover_jobs(&repo, Utc::now())?;
    for job in recovered {
        let _ = app.emit("lumo://job-recovered", job);
    }
    let state = app.state::<DesktopState>();
    let root = {
        let mut runtime = lock_jobs(&state.jobs)?;
        if runtime.worker_started {
            return Err("desktop job worker is already started".into());
        }
        runtime.worker_started = true;
        runtime.root.clone()
    };
    let worker_id = format!("desktop-{}", ulid::Ulid::new());
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        run_worker(app, repo, worker_id, root).await;
    });
    Ok(())
}

#[tauri::command]
pub(super) fn job_list(app: AppHandle, limit: Option<u32>) -> Result<Vec<AgentJobRow>, String> {
    list_jobs(&open_repo(&app)?, limit.unwrap_or(100))
}

#[tauri::command]
pub(super) fn job_schedule(
    app: AppHandle,
    request: JobScheduleRequest,
) -> Result<EnqueueJobResult, String> {
    let result = schedule_job(&open_repo(&app)?, request, Utc::now())?;
    let _ = app.emit("lumo://job-updated", &result.job);
    Ok(result)
}

#[tauri::command]
pub(super) fn job_pause(
    app: AppHandle,
    state: State<'_, DesktopState>,
    job_id: String,
) -> Result<bool, String> {
    control_job(&app, &state, &job_id, JobControl::Pause)
}

#[tauri::command]
pub(super) fn job_resume(
    app: AppHandle,
    state: State<'_, DesktopState>,
    job_id: String,
) -> Result<bool, String> {
    control_job(&app, &state, &job_id, JobControl::Resume)
}

#[tauri::command]
pub(super) fn job_cancel(
    app: AppHandle,
    state: State<'_, DesktopState>,
    job_id: String,
) -> Result<bool, String> {
    control_job(&app, &state, &job_id, JobControl::Cancel)
}

fn control_job(
    app: &AppHandle,
    state: &DesktopState,
    job_id: &str,
    control: JobControl,
) -> Result<bool, String> {
    let repo = open_repo(app)?;
    let changed = transition_job(&repo, &state.jobs, job_id, control, Utc::now())?;
    if changed {
        if let Some(job) = repo.get_job(job_id).map_err(|error| error.to_string())? {
            let _ = app.emit("lumo://job-updated", job);
        }
    }
    Ok(changed)
}

fn schedule_job(
    repo: &Repo,
    request: JobScheduleRequest,
    now: DateTime<Utc>,
) -> Result<EnqueueJobResult, String> {
    if request.utterance.trim().is_empty() {
        return Err("job utterance must not be empty".into());
    }
    validate_dag(&request.plan).map_err(|error| error.to_string())?;
    if request.max_attempts <= 0 {
        return Err("job maxAttempts must be greater than zero".into());
    }
    let id = ulid::Ulid::new().to_string();
    let idempotency_key = request
        .idempotency_key
        .clone()
        .unwrap_or_else(|| id.clone());
    let job = AgentJob::new(
        &id,
        &idempotency_key,
        request.plan.clone(),
        request.schedule.clone(),
        now,
    )
    .map_err(|error| error.to_string())?;
    let (schedule_kind, schedule_spec) = match &job.schedule {
        JobSchedule::OneShot { run_at } => ("one_shot", serde_json::json!({ "runAt": run_at })),
        JobSchedule::Cron { expression } => {
            ("cron", serde_json::json!({ "expression": expression }))
        }
    };
    let payload = serde_json::to_value(AgentStartInput {
        utterance: request.utterance,
        profile_id: request.profile_id,
        supplied_plan: Some(request.plan),
    })
    .map_err(|error| error.to_string())?;
    repo.enqueue_job(&NewAgentJob {
        id,
        idempotency_key,
        payload,
        schedule_kind: schedule_kind.into(),
        schedule_spec,
        priority: request.priority.clamp(-100, 100),
        available_at: job.next_run_at.unwrap_or(now),
        max_attempts: request.max_attempts,
        created_at: now,
    })
    .map_err(|error| error.to_string())
}

fn list_jobs(repo: &Repo, limit: u32) -> Result<Vec<AgentJobRow>, String> {
    repo.list_jobs(limit.clamp(1, 1_000))
        .map_err(|error| error.to_string())
}

fn transition_job(
    repo: &Repo,
    runtime: &Mutex<JobRuntime>,
    job_id: &str,
    control: JobControl,
    now: DateTime<Utc>,
) -> Result<bool, String> {
    let changed = match control {
        JobControl::Pause => repo.pause_job(job_id, now),
        JobControl::Resume => repo.resume_job(job_id, now),
        JobControl::Cancel => repo.cancel_job(job_id, now),
    }
    .map_err(|error| error.to_string())?;
    if changed && !matches!(control, JobControl::Resume) {
        if let Some(cancel) = lock_jobs(runtime)?.active.remove(job_id) {
            cancel.cancel();
        }
    }
    Ok(changed)
}

fn recover_jobs(repo: &Repo, now: DateTime<Utc>) -> Result<Vec<RecoveredJob>, String> {
    repo.recover_expired_jobs(now)
        .map_err(|error| error.to_string())
}

fn lease_due_job(
    repo: &Repo,
    worker_id: &str,
    now: DateTime<Utc>,
    lease: Duration,
) -> Result<Option<AgentJobRow>, String> {
    repo.acquire_job_lease(worker_id, now, lease)
        .map_err(|error| error.to_string())
}

fn heartbeat_lease(
    repo: &Repo,
    job_id: &str,
    worker_id: &str,
    now: DateTime<Utc>,
    lease: Duration,
) -> Result<bool, String> {
    repo.heartbeat_job(job_id, worker_id, now, lease)
        .map_err(|error| error.to_string())
}

async fn run_worker(app: AppHandle, repo: Repo, worker_id: String, root: CancellationToken) {
    let mut poll = tokio::time::interval(StdDuration::from_millis(JOB_POLL_MILLIS));
    loop {
        tokio::select! {
            _ = root.cancelled() => break,
            _ = poll.tick() => {
                let now = Utc::now();
                let _ = recover_jobs(&repo, now);
                match lease_due_job(&repo, &worker_id, now, Duration::seconds(JOB_LEASE_SECONDS)) {
                    Ok(Some(job)) => process_leased_job(&app, &repo, &worker_id, job).await,
                    Ok(None) => {}
                    Err(error) => { let _ = app.emit("lumo://job-worker-error", serde_json::json!({ "error": error })); }
                }
            }
        }
    }
}

async fn process_leased_job(app: &AppHandle, repo: &Repo, worker_id: &str, job: AgentJobRow) {
    let input = match serde_json::from_value::<AgentStartInput>(job.payload.clone()) {
        Ok(input) => input,
        Err(error) => {
            let _ = repo.retry_job(
                &job.id,
                worker_id,
                Utc::now(),
                Utc::now() + Duration::seconds(30),
                &format!("invalid job payload: {error}"),
            );
            return;
        }
    };
    let service = match app
        .state::<DesktopState>()
        .agent_service
        .lock()
        .map_err(|_| "desktop agent service is unavailable".to_string())
        .and_then(|service| {
            service
                .clone()
                .ok_or_else(|| "desktop agent service is not initialized".to_string())
        }) {
        Ok(service) => service,
        Err(error) => {
            let _ = repo.retry_job(
                &job.id,
                worker_id,
                Utc::now(),
                Utc::now() + Duration::seconds(30),
                &error,
            );
            return;
        }
    };
    let execution = match service.start(input).await {
        Ok(execution) => execution,
        Err(error) => {
            let _ = repo.retry_job(
                &job.id,
                worker_id,
                Utc::now(),
                Utc::now() + Duration::seconds(30),
                &error,
            );
            return;
        }
    };
    let run_id = execution.run_id().to_string();
    let cancel = execution.cancel_token();
    if let Ok(mut runtime) = app.state::<DesktopState>().jobs.lock() {
        runtime.active.insert(job.id.clone(), cancel.clone());
    }
    let mut completion = Box::pin(execution.wait());
    let mut heartbeat = tokio::time::interval(StdDuration::from_secs(JOB_HEARTBEAT_SECONDS));
    let outcome = loop {
        tokio::select! {
            result = &mut completion => break result,
            _ = heartbeat.tick() => {
                match heartbeat_lease(repo, &job.id, worker_id, Utc::now(), Duration::seconds(JOB_LEASE_SECONDS)) {
                    Ok(true) => {}
                    Ok(false) => { cancel.cancel(); break Err("job lease ownership was lost".into()); }
                    Err(error) => { cancel.cancel(); break Err(error); }
                }
            }
        }
    };
    if let Ok(mut runtime) = app.state::<DesktopState>().jobs.lock() {
        runtime.active.remove(&job.id);
    }
    let now = Utc::now();
    match outcome {
        Ok(report) if report.status == LoopStatus::Completed => {
            let next = next_run_at(&job, now).unwrap_or(None);
            let _ = repo.complete_job(&job.id, worker_id, now, next);
        }
        Ok(report) => {
            let error = report
                .error
                .unwrap_or_else(|| format!("agent run {run_id} ended as {:?}", report.status));
            let _ = repo.retry_job(&job.id, worker_id, now, now + Duration::seconds(30), &error);
        }
        Err(error) => {
            let _ = repo.retry_job(&job.id, worker_id, now, now + Duration::seconds(30), &error);
        }
    }
    if let Ok(Some(updated)) = repo.get_job(&job.id) {
        let _ = app.emit("lumo://job-updated", updated);
    }
}

fn next_run_at(job: &AgentJobRow, now: DateTime<Utc>) -> Result<Option<DateTime<Utc>>, String> {
    if job.schedule_kind != "cron" {
        return Ok(None);
    }
    let expression = job
        .schedule_spec
        .get("expression")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| format!("job `{}` has an invalid cron schedule", job.id))?;
    JobSchedule::cron(expression)
        .map_err(|error| error.to_string())?
        .next_after(now)
        .map_err(|error| error.to_string())
}

fn lock_jobs(jobs: &Mutex<JobRuntime>) -> Result<std::sync::MutexGuard<'_, JobRuntime>, String> {
    jobs.lock()
        .map_err(|_| "desktop job runtime is unavailable".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{Duration, TimeZone, Utc};
    use lumo_agent::{AgentPlan, JobSchedule, PlanNode, RiskLevel};
    use lumo_storage::{JobNodeCheckpoint, Repo};
    use serde_json::json;

    fn at(seconds: i64) -> chrono::DateTime<Utc> {
        Utc.timestamp_opt(1_760_000_000 + seconds, 0).unwrap()
    }

    fn plan() -> AgentPlan {
        AgentPlan::new(
            "scheduled-plan",
            "运行日报",
            vec![PlanNode {
                id: "daily".into(),
                depends_on: Vec::new(),
                capability_id: "flow:daily".into(),
                arguments: json!({}),
                risk: RiskLevel::L0,
                timeout_ms: 1_000,
                retry_limit: 0,
                expected_output_schema: None,
            }],
        )
    }

    fn request(run_at: chrono::DateTime<Utc>) -> JobScheduleRequest {
        JobScheduleRequest {
            utterance: "运行日报".into(),
            profile_id: None,
            plan: plan(),
            schedule: JobSchedule::one_shot(run_at),
            idempotency_key: Some("daily-once".into()),
            priority: 8,
            max_attempts: 4,
        }
    }

    #[test]
    fn schedule_list_pause_resume_and_cancel_round_trip() {
        let repo = Repo::open_in_memory().unwrap();
        let runtime = Mutex::new(JobRuntime::default());
        let scheduled = schedule_job(&repo, request(at(10)), at(0)).unwrap();
        assert!(scheduled.inserted);
        assert_eq!(scheduled.job.available_at, at(10));
        assert_eq!(list_jobs(&repo, 20).unwrap().len(), 1);

        assert!(
            transition_job(&repo, &runtime, &scheduled.job.id, JobControl::Pause, at(1),).unwrap()
        );
        assert_eq!(
            repo.get_job(&scheduled.job.id).unwrap().unwrap().state,
            "paused"
        );
        assert!(transition_job(
            &repo,
            &runtime,
            &scheduled.job.id,
            JobControl::Resume,
            at(2),
        )
        .unwrap());
        assert!(transition_job(
            &repo,
            &runtime,
            &scheduled.job.id,
            JobControl::Cancel,
            at(3),
        )
        .unwrap());
        assert_eq!(
            repo.get_job(&scheduled.job.id)
                .unwrap()
                .unwrap()
                .last_error
                .as_deref(),
            Some("cancelled")
        );
    }

    #[test]
    fn recovery_marks_uncertain_side_effect_and_requeues_safe_work() {
        let repo = Repo::open_in_memory().unwrap();
        let safe = schedule_job(&repo, request(at(0)), at(0)).unwrap().job;
        let mut unsafe_request = request(at(0));
        unsafe_request.idempotency_key = Some("unsafe".into());
        let unsafe_job = schedule_job(&repo, unsafe_request, at(0)).unwrap().job;
        repo.acquire_job_lease("dead", at(1), Duration::seconds(2))
            .unwrap();
        repo.record_job_node(&JobNodeCheckpoint {
            job_id: safe.id.clone(),
            node_id: "read".into(),
            state: "running".into(),
            risk: "L1".into(),
            idempotent: true,
            attempt: 1,
            updated_at: at(1),
        })
        .unwrap();
        repo.acquire_job_lease("dead", at(1), Duration::seconds(2))
            .unwrap();
        repo.record_job_node(&JobNodeCheckpoint {
            job_id: unsafe_job.id.clone(),
            node_id: "payment".into(),
            state: "running".into(),
            risk: "L3".into(),
            idempotent: false,
            attempt: 1,
            updated_at: at(1),
        })
        .unwrap();

        let recovered = recover_jobs(&repo, at(4)).unwrap();
        assert_eq!(recovered.len(), 2);
        assert_eq!(repo.get_job(&safe.id).unwrap().unwrap().state, "queued");
        assert_eq!(
            repo.get_job(&unsafe_job.id).unwrap().unwrap().state,
            "unknown"
        );
    }

    #[test]
    fn lease_and_heartbeat_are_owned_by_one_worker() {
        let repo = Repo::open_in_memory().unwrap();
        schedule_job(&repo, request(at(0)), at(0)).unwrap();

        let leased = lease_due_job(&repo, "desktop-a", at(1), Duration::seconds(30))
            .unwrap()
            .unwrap();
        assert_eq!(leased.worker_id.as_deref(), Some("desktop-a"));
        assert!(
            heartbeat_lease(&repo, &leased.id, "desktop-a", at(2), Duration::seconds(30),).unwrap()
        );
        assert!(
            !heartbeat_lease(&repo, &leased.id, "desktop-b", at(3), Duration::seconds(30),)
                .unwrap()
        );
    }
}
