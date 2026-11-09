use chrono::{Duration, TimeZone, Utc};
use lumo_storage::{NewAgentJob, Repo};
use serde_json::json;

fn job() -> NewAgentJob {
    let now = Utc.timestamp_opt(1_760_000_000, 0).unwrap();
    NewAgentJob {
        id: "scheduled".into(),
        idempotency_key: "create-once".into(),
        payload: json!({"original":true}),
        schedule_kind: "one_shot".into(),
        schedule_spec: json!({"runAt":now + Duration::hours(1)}),
        priority: 5,
        available_at: now + Duration::hours(1),
        max_attempts: 3,
        created_at: now,
    }
}

#[test]
fn schedule_edit_is_atomic_and_rejects_stale_or_started_jobs() {
    let repo = Repo::open_in_memory().unwrap();
    let original = job();
    repo.enqueue_job(&original).unwrap();
    let mut replacement = original.clone();
    replacement.payload = json!({"inputs":{"date":"today"}});
    replacement.max_attempts = 7;
    replacement.available_at += Duration::hours(1);
    let now = original.created_at + Duration::seconds(1);
    assert!(repo
        .update_job_schedule("scheduled", original.created_at, &replacement, now)
        .unwrap());
    assert!(!repo
        .update_job_schedule("scheduled", original.created_at, &original, now)
        .unwrap());
    let stored = repo.get_job("scheduled").unwrap().unwrap();
    assert_eq!(stored.payload, replacement.payload);
    assert_eq!(stored.max_attempts, 7);
    assert_eq!(stored.idempotency_key, "create-once");
    repo.acquire_job_lease("worker", replacement.available_at, Duration::seconds(30))
        .unwrap()
        .unwrap();
    assert!(!repo
        .update_job_schedule("scheduled", stored.updated_at, &original, now)
        .unwrap());
    repo.pause_job("scheduled", replacement.available_at + Duration::seconds(1))
        .unwrap();
    let paused = repo.get_job("scheduled").unwrap().unwrap();
    assert!(!repo
        .update_job_schedule("scheduled", paused.updated_at, &original, now)
        .unwrap());
}

#[test]
fn paused_unstarted_job_can_be_edited_without_resuming_it() {
    let repo = Repo::open_in_memory().unwrap();
    let original = job();
    repo.enqueue_job(&original).unwrap();
    let now = original.created_at + Duration::seconds(1);
    repo.pause_job("scheduled", now).unwrap();
    assert!(repo
        .update_job_schedule("scheduled", now, &original, now + Duration::seconds(1))
        .unwrap());
    assert_eq!(repo.get_job("scheduled").unwrap().unwrap().state, "paused");
}
