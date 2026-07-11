use chrono::{Duration, TimeZone, Utc};
use lumo_storage::{JobNodeCheckpoint, NewAgentJob, Repo};
use serde_json::json;

fn at(seconds: i64) -> chrono::DateTime<Utc> {
    Utc.timestamp_opt(1_750_000_000 + seconds, 0).unwrap()
}

fn job(id: &str, key: &str, available_at: chrono::DateTime<Utc>) -> NewAgentJob {
    NewAgentJob {
        id: id.into(),
        idempotency_key: key.into(),
        payload: json!({"planId": format!("plan-{id}")}),
        schedule_kind: "one_shot".into(),
        schedule_spec: json!({"runAt": available_at}),
        priority: 5,
        available_at,
        max_attempts: 3,
        created_at: at(0),
    }
}

#[test]
fn enqueue_is_idempotent_and_preserves_original_payload() {
    let repo = Repo::open_in_memory().unwrap();
    let first = repo.enqueue_job(&job("job-1", "same-key", at(10))).unwrap();
    let second = repo.enqueue_job(&job("job-2", "same-key", at(20))).unwrap();

    assert!(first.inserted);
    assert!(!second.inserted);
    assert_eq!(second.job.id, "job-1");
    assert_eq!(repo.list_jobs(10).unwrap().len(), 1);
    assert_eq!(second.job.payload, json!({"planId": "plan-job-1"}));
}

#[test]
fn lease_is_exclusive_and_heartbeat_requires_the_owner() {
    let repo = Repo::open_in_memory().unwrap();
    repo.enqueue_job(&job("job-1", "lease-key", at(0))).unwrap();

    let leased = repo
        .acquire_job_lease("worker-a", at(1), Duration::seconds(30))
        .unwrap()
        .unwrap();
    assert_eq!(leased.state, "running");
    assert_eq!(leased.worker_id.as_deref(), Some("worker-a"));
    assert_eq!(leased.attempts, 1);
    assert!(repo
        .acquire_job_lease("worker-b", at(2), Duration::seconds(30))
        .unwrap()
        .is_none());

    assert!(!repo
        .heartbeat_job("job-1", "worker-b", at(5), Duration::seconds(30))
        .unwrap());
    assert!(repo
        .heartbeat_job("job-1", "worker-a", at(5), Duration::seconds(30))
        .unwrap());
    assert_eq!(
        repo.get_job("job-1").unwrap().unwrap().lease_until,
        Some(at(35))
    );
}

#[test]
fn retry_waits_until_due_and_stops_at_attempt_limit() {
    let repo = Repo::open_in_memory().unwrap();
    let mut limited = job("job-1", "retry-key", at(0));
    limited.max_attempts = 2;
    repo.enqueue_job(&limited).unwrap();

    repo.acquire_job_lease("worker", at(1), Duration::seconds(10))
        .unwrap();
    assert!(repo
        .retry_job("job-1", "worker", at(2), at(20), "temporary")
        .unwrap());
    assert_eq!(repo.get_job("job-1").unwrap().unwrap().state, "waiting");
    assert!(repo
        .acquire_job_lease("worker", at(19), Duration::seconds(10))
        .unwrap()
        .is_none());

    repo.acquire_job_lease("worker", at(20), Duration::seconds(10))
        .unwrap();
    assert!(repo
        .retry_job("job-1", "worker", at(21), at(40), "permanent")
        .unwrap());
    let failed = repo.get_job("job-1").unwrap().unwrap();
    assert_eq!(failed.state, "failed");
    assert_eq!(failed.last_error.as_deref(), Some("permanent"));
}

#[test]
fn pause_resume_cancel_and_recurring_completion_are_durable() {
    let repo = Repo::open_in_memory().unwrap();
    let mut recurring = job("job-1", "cron-key", at(0));
    recurring.schedule_kind = "cron".into();
    recurring.schedule_spec = json!({"expression": "0 0 * * * *"});
    repo.enqueue_job(&recurring).unwrap();

    assert!(repo.pause_job("job-1", at(1)).unwrap());
    assert_eq!(repo.get_job("job-1").unwrap().unwrap().state, "paused");
    assert!(repo.resume_job("job-1", at(2)).unwrap());
    repo.acquire_job_lease("worker", at(2), Duration::seconds(10))
        .unwrap();
    assert!(repo
        .complete_job("job-1", "worker", at(3), Some(at(100)))
        .unwrap());
    let rescheduled = repo.get_job("job-1").unwrap().unwrap();
    assert_eq!(rescheduled.state, "queued");
    assert_eq!(rescheduled.available_at, at(100));
    assert_eq!(rescheduled.attempts, 0);

    assert!(repo.cancel_job("job-1", at(4)).unwrap());
    let cancelled = repo.get_job("job-1").unwrap().unwrap();
    assert_eq!(cancelled.state, "failed");
    assert!(cancelled.cancelled_at.is_some());
    assert!(!repo.resume_job("job-1", at(5)).unwrap());
}

#[test]
fn crash_recovery_requeues_idempotent_work_and_marks_uncertain_l3_unknown() {
    let repo = Repo::open_in_memory().unwrap();
    repo.enqueue_job(&job("safe", "safe-key", at(0))).unwrap();
    repo.acquire_job_lease("dead-a", at(1), Duration::seconds(5))
        .unwrap();
    repo.record_job_node(&JobNodeCheckpoint {
        job_id: "safe".into(),
        node_id: "read".into(),
        state: "running".into(),
        risk: "L1".into(),
        idempotent: true,
        attempt: 1,
        updated_at: at(2),
    })
    .unwrap();

    repo.enqueue_job(&job("unsafe", "unsafe-key", at(0)))
        .unwrap();
    repo.acquire_job_lease("dead-b", at(1), Duration::seconds(5))
        .unwrap();
    repo.record_job_node(&JobNodeCheckpoint {
        job_id: "unsafe".into(),
        node_id: "send-payment".into(),
        state: "running".into(),
        risk: "L3".into(),
        idempotent: false,
        attempt: 1,
        updated_at: at(2),
    })
    .unwrap();

    let recovered = repo.recover_expired_jobs(at(7)).unwrap();
    assert_eq!(recovered.len(), 2);
    assert_eq!(repo.get_job("safe").unwrap().unwrap().state, "queued");
    assert_eq!(repo.get_job("unsafe").unwrap().unwrap().state, "unknown");
    assert_eq!(repo.list_job_nodes("unsafe").unwrap()[0].state, "unknown");
}
