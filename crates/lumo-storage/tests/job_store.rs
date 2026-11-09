use chrono::{Duration, TimeZone, Utc};
use lumo_storage::{DurableJobNode, JobNodeCheckpoint, NewAgentJob, Repo};
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

fn background_run(repo: &Repo, id: &str) {
    repo.create_agent_run(&lumo_storage::AgentRunRow {
        id: id.into(),
        profile_id: None,
        utterance: None,
        plan_json: None,
        approval_json: None,
        state: "running".into(),
        started_at: at(0),
        finished_at: None,
        error: None,
    })
    .unwrap();
}

#[test]
fn run_controls_are_atomic_bound_to_one_occurrence_and_do_not_bypass_approval() {
    let repo = Repo::open_in_memory().unwrap();
    repo.enqueue_job(&job("job-1", "controls", at(0))).unwrap();
    repo.acquire_job_lease("worker", at(1), Duration::seconds(30))
        .unwrap()
        .unwrap();
    background_run(&repo, "run-1");
    repo.bind_job_run("job-1", "worker", "run-1", at(1))
        .unwrap();
    let (paused, event) = repo
        .control_job_run("run-1", "pause", at(2))
        .unwrap()
        .unwrap();
    assert_eq!(paused.state, "paused");
    assert_eq!(event.kind, "run.paused");
    assert_eq!(
        repo.control_job_run("run-1", "resume", at(3))
            .unwrap()
            .unwrap()
            .0
            .state,
        "queued"
    );
    repo.acquire_job_lease("worker", at(4), Duration::seconds(30))
        .unwrap()
        .unwrap();
    assert!(repo.wait_job_approval("job-1", "worker", at(5)).unwrap());
    for action in ["pause", "resume"] {
        assert!(repo
            .control_job_run("run-1", action, at(6))
            .unwrap()
            .is_none());
    }
    assert_eq!(
        repo.get_job("job-1").unwrap().unwrap().state,
        "waiting_approval"
    );
    let (cancelled, event) = repo
        .control_job_run("run-1", "cancel", at(7))
        .unwrap()
        .unwrap();
    assert!(cancelled.cancelled_at.is_some());
    assert_eq!(event.kind, "run.cancelled");
    assert_eq!(repo.list_agent_events("run-1", 0).unwrap().len(), 3);

    let repo = Repo::open_in_memory().unwrap();
    repo.enqueue_job(&job("job-1", "cron", at(0))).unwrap();
    repo.acquire_job_lease("worker", at(1), Duration::seconds(30))
        .unwrap()
        .unwrap();
    background_run(&repo, "old-run");
    repo.bind_job_run("job-1", "worker", "old-run", at(1))
        .unwrap();
    repo.complete_job("job-1", "worker", at(2), Some(at(3)))
        .unwrap();
    repo.acquire_job_lease("worker", at(3), Duration::seconds(30))
        .unwrap()
        .unwrap();
    background_run(&repo, "new-run");
    repo.bind_job_run("job-1", "worker", "new-run", at(3))
        .unwrap();
    for action in ["pause", "resume", "cancel"] {
        assert!(repo
            .control_job_run("old-run", action, at(4))
            .unwrap()
            .is_none());
    }
    assert_eq!(repo.get_job("job-1").unwrap().unwrap().state, "running");
    assert!(repo.list_agent_events("old-run", 0).unwrap().is_empty());
}

#[test]
fn failed_run_control_event_rolls_back_job_and_run_changes() {
    let repo = Repo::open_in_memory().unwrap();
    repo.enqueue_job(&job("job-1", "controls", at(0))).unwrap();
    repo.acquire_job_lease("worker", at(1), Duration::seconds(30))
        .unwrap()
        .unwrap();
    background_run(&repo, "run-1");
    repo.bind_job_run("job-1", "worker", "run-1", at(1))
        .unwrap();
    repo.with_raw(|connection|connection.execute_batch("CREATE TRIGGER reject_control_event BEFORE INSERT ON agent_events BEGIN SELECT RAISE(ABORT,'disk failed'); END")).unwrap();
    assert!(repo.control_job_run("run-1", "cancel", at(2)).is_err());
    let job = repo.get_job("job-1").unwrap().unwrap();
    assert_eq!(job.state, "running");
    assert!(job.cancelled_at.is_none());
    let state: String = repo
        .with_raw(|connection| {
            connection.query_row("SELECT state FROM agent_runs WHERE id='run-1'", [], |row| {
                row.get(0)
            })
        })
        .unwrap();
    assert_eq!(state, "running");
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

fn checkpoint(job_id: &str, node_id: &str, risk: &str, idempotent: bool) -> JobNodeCheckpoint {
    JobNodeCheckpoint {
        job_id: job_id.into(),
        node_id: node_id.into(),
        state: "running".into(),
        risk: risk.into(),
        idempotent,
        attempt: 1,
        updated_at: at(2),
    }
}

#[test]
fn cron_completion_clears_the_finished_rounds_node_checkpoints() {
    let repo = Repo::open_in_memory().unwrap();
    let mut recurring = job("cron", "cron-round", at(0));
    recurring.schedule_kind = "cron".into();
    repo.enqueue_job(&recurring).unwrap();
    repo.acquire_job_lease("worker", at(1), Duration::seconds(30))
        .unwrap();
    assert!(repo
        .bind_job_run("cron", "worker", "cron-run-1", at(2))
        .unwrap());
    let mut node = checkpoint("cron", "send", "L3", false);
    node.state = "completed".into();
    repo.record_job_node(&node).unwrap();

    assert!(!repo
        .complete_job("cron", "other", at(3), Some(at(100)))
        .unwrap());
    assert_eq!(
        repo.list_job_nodes("cron").unwrap().len(),
        1,
        "a stale owner must not clear checkpoints"
    );
    assert!(repo
        .complete_job("cron", "worker", at(3), Some(at(100)))
        .unwrap());
    assert!(
        repo.list_job_nodes("cron").unwrap().is_empty(),
        "the next cron occurrence must execute every node again"
    );
    assert!(repo.get_job("cron").unwrap().unwrap().run_id.is_none());
    assert!(repo.job_for_run("cron-run-1").unwrap().is_none());
}

#[test]
fn recovery_classifies_each_running_node_without_inventing_success() {
    let repo = Repo::open_in_memory().unwrap();
    repo.enqueue_job(&job("mixed", "mixed-recovery", at(0)))
        .unwrap();
    repo.acquire_job_lease("dead", at(1), Duration::seconds(5))
        .unwrap();
    repo.record_job_node(&checkpoint("mixed", "read", "L1", true))
        .unwrap();
    repo.record_job_node(&checkpoint("mixed", "send", "L3", false))
        .unwrap();
    let mut finished = checkpoint("mixed", "done", "L2", false);
    finished.state = "completed".into();
    repo.save_durable_job_node(
        "dead",
        &DurableJobNode {
            checkpoint: finished,
            node_hash: "completed-node".into(),
            result: Some(json!({"output":"completed"})),
        },
    )
    .unwrap();

    repo.recover_expired_jobs(at(7)).unwrap();
    let nodes = repo.list_job_nodes("mixed").unwrap();
    assert_eq!(repo.get_job("mixed").unwrap().unwrap().state, "unknown");
    assert_eq!(
        nodes
            .iter()
            .find(|node| node.node_id == "read")
            .unwrap()
            .state,
        "queued"
    );
    assert_eq!(
        nodes
            .iter()
            .find(|node| node.node_id == "send")
            .unwrap()
            .state,
        "unknown"
    );
    assert_eq!(
        nodes
            .iter()
            .find(|node| node.node_id == "done")
            .unwrap()
            .state,
        "completed"
    );
}

#[test]
fn uncertain_non_idempotent_work_requires_review_even_at_low_risk() {
    let repo = Repo::open_in_memory().unwrap();
    repo.enqueue_job(&job("low", "low-risk-uncertainty", at(0)))
        .unwrap();
    repo.acquire_job_lease("dead", at(1), Duration::seconds(5))
        .unwrap();
    repo.record_job_node(&checkpoint("low", "write", "L1", false))
        .unwrap();
    repo.recover_expired_jobs(at(7)).unwrap();
    assert_eq!(repo.get_job("low").unwrap().unwrap().state, "unknown");
    assert_eq!(repo.list_job_nodes("low").unwrap()[0].state, "unknown");
    assert!(
        !repo.resume_job("low", at(8)).unwrap(),
        "resume must not bypass unknown-node review"
    );
}

#[test]
fn retries_and_pauses_cannot_requeue_an_uncertain_side_effect() {
    for action in ["retry", "pause"] {
        let repo = Repo::open_in_memory().unwrap();
        repo.enqueue_job(&job(action, action, at(0))).unwrap();
        repo.acquire_job_lease("worker", at(1), Duration::seconds(30))
            .unwrap();
        repo.record_job_node(&checkpoint(action, "write", "L1", false))
            .unwrap();
        let changed = if action == "retry" {
            repo.retry_job(action, "worker", at(3), at(10), "connection lost")
                .unwrap()
        } else {
            repo.pause_job(action, at(3)).unwrap()
        };
        assert!(changed);
        assert_eq!(repo.get_job(action).unwrap().unwrap().state, "unknown");
        assert_eq!(repo.list_job_nodes(action).unwrap()[0].state, "unknown");
        assert!(!repo.resume_job(action, at(4)).unwrap());
    }
}

#[test]
fn durable_results_and_run_binding_require_the_live_lease_owner() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("durable.db");
    let repo = Repo::open(&path).unwrap();
    repo.enqueue_job(&job("durable", "durable-node", at(0)))
        .unwrap();
    repo.acquire_job_lease("worker", at(1), Duration::seconds(10))
        .unwrap();
    assert!(!repo
        .bind_job_run("durable", "stale", "run-1", at(2))
        .unwrap());
    assert!(repo
        .bind_job_run("durable", "worker", "run-1", at(2))
        .unwrap());
    assert_eq!(repo.job_for_run("run-1").unwrap().unwrap().id, "durable");
    let mut durable = DurableJobNode {
        checkpoint: checkpoint("durable", "send", "L2", false),
        node_hash: "plan-node-capability".into(),
        result: None,
    };
    assert!(repo.save_durable_job_node("stale", &durable).is_err());
    repo.save_durable_job_node("worker", &durable).unwrap();
    durable.checkpoint.state = "completed".into();
    durable.checkpoint.updated_at = at(3);
    durable.result = Some(json!({"output":{"receipt":"r-1"},"tokensUsed":7}));
    repo.save_durable_job_node("worker", &durable).unwrap();
    drop(repo);
    let repo = Repo::open(&path).unwrap();
    let restored = repo.list_durable_job_nodes("durable").unwrap();
    assert_eq!(restored[0].node_hash, "plan-node-capability");
    assert_eq!(restored[0].result, durable.result);
    durable.checkpoint.updated_at = at(12);
    assert!(repo.save_durable_job_node("worker", &durable).is_err());
}

#[test]
fn approval_wait_is_durable_does_not_spend_attempts_and_requires_explicit_resume() {
    let repo = Repo::open_in_memory().unwrap();
    let mut limited = job("approval", "approval-wait", at(0));
    limited.max_attempts = 1;
    repo.enqueue_job(&limited).unwrap();
    repo.acquire_job_lease("worker", at(1), Duration::seconds(30))
        .unwrap();
    repo.bind_job_run("approval", "worker", "approval-run", at(2))
        .unwrap();
    assert!(!repo.wait_job_approval("approval", "stale", at(3)).unwrap());
    assert!(repo.wait_job_approval("approval", "worker", at(3)).unwrap());
    let waiting = repo.get_job("approval").unwrap().unwrap();
    assert_eq!(waiting.state, "waiting_approval");
    assert_eq!(waiting.attempts, 0);
    assert_eq!(waiting.run_id.as_deref(), Some("approval-run"));
    assert!(waiting.worker_id.is_none());
    assert!(repo
        .acquire_job_lease("other", at(100), Duration::seconds(30))
        .unwrap()
        .is_none());
    assert!(!repo.resume_job("approval", at(101)).unwrap());
    assert!(repo.resume_job_approval("approval", at(101)).unwrap());
    assert!(!repo.resume_job_approval("approval", at(102)).unwrap());
    assert_eq!(
        repo.acquire_job_lease("worker", at(102), Duration::seconds(30))
            .unwrap()
            .unwrap()
            .attempts,
        1
    );
}

#[test]
fn human_review_resolves_only_unknown_nodes_and_preserves_completed_results() {
    let repo = Repo::open_in_memory().unwrap();
    repo.enqueue_job(&job("review", "human-review", at(0)))
        .unwrap();
    repo.acquire_job_lease("dead", at(1), Duration::seconds(5))
        .unwrap();
    repo.bind_job_run("review", "dead", "review-run", at(2))
        .unwrap();
    for id in ["first", "second"] {
        repo.save_durable_job_node(
            "dead",
            &DurableJobNode {
                checkpoint: checkpoint("review", id, "L3", false),
                node_hash: format!("hash-{id}"),
                result: None,
            },
        )
        .unwrap();
    }
    repo.recover_expired_jobs(at(7)).unwrap();
    let result = json!({"output":{"receipt":"verified"}});
    assert!(repo
        .resolve_unknown_job_node("review", "first", Some(result.clone()), at(8))
        .unwrap());
    assert_eq!(repo.get_job("review").unwrap().unwrap().state, "unknown");
    assert!(!repo
        .resolve_unknown_job_node("review", "first", None, at(9))
        .unwrap());
    assert!(repo
        .resolve_unknown_job_node("review", "second", None, at(9))
        .unwrap());
    assert_eq!(repo.get_job("review").unwrap().unwrap().state, "queued");
    let nodes = repo.list_durable_job_nodes("review").unwrap();
    assert_eq!(nodes[0].checkpoint.state, "completed");
    assert_eq!(nodes[0].result, Some(result));
    assert_eq!(nodes[1].checkpoint.state, "queued");
    assert_eq!(nodes[1].node_hash, "hash-second");
}

#[test]
fn recovery_without_any_checkpoint_requires_a_bound_run_or_manual_review() {
    let repo = Repo::open_in_memory().unwrap();
    for id in ["bound", "legacy"] {
        repo.enqueue_job(&job(id, id, at(0))).unwrap();
        repo.acquire_job_lease(id, at(1), Duration::seconds(5))
            .unwrap();
    }
    repo.bind_job_run("bound", "bound", "bound-run", at(2))
        .unwrap();
    repo.recover_expired_jobs(at(7)).unwrap();
    assert_eq!(repo.get_job("bound").unwrap().unwrap().state, "queued");
    assert_eq!(repo.get_job("legacy").unwrap().unwrap().state, "unknown");
    assert!(
        repo.list_job_nodes("legacy")
            .unwrap()
            .iter()
            .any(|node| node.state == "unknown"),
        "legacy uncertainty needs a reviewable marker"
    );
    assert!(repo
        .resolve_unknown_job_node("legacy", "__legacy_execution__", None, at(8))
        .unwrap());
    assert_eq!(repo.get_job("legacy").unwrap().unwrap().state, "queued");
    assert!(
        repo.list_job_nodes("legacy").unwrap().is_empty(),
        "unbound legacy markers cannot be replayed as real plan nodes"
    );
}

#[test]
fn legacy_unhashed_unknown_results_cannot_be_invented_but_can_be_cleared_for_verified_retry() {
    let repo = Repo::open_in_memory().unwrap();
    repo.enqueue_job(&job("old", "old-checkpoint", at(0)))
        .unwrap();
    repo.acquire_job_lease("dead", at(1), Duration::seconds(5))
        .unwrap();
    repo.record_job_node(&checkpoint("old", "send", "L3", false))
        .unwrap();
    repo.recover_expired_jobs(at(7)).unwrap();
    assert!(!repo
        .resolve_unknown_job_node("old", "send", Some(json!({"output":"unbound"})), at(8))
        .unwrap());
    assert!(repo
        .resolve_unknown_job_node("old", "send", None, at(8))
        .unwrap());
    assert!(repo.list_job_nodes("old").unwrap().is_empty());
    assert_eq!(repo.get_job("old").unwrap().unwrap().state, "queued");
}

#[test]
fn legacy_completed_nodes_without_a_durable_result_require_review() {
    let repo = Repo::open_in_memory().unwrap();
    repo.enqueue_job(&job("legacy-done", "legacy-completed", at(0)))
        .unwrap();
    repo.acquire_job_lease("dead", at(1), Duration::seconds(5))
        .unwrap();
    let mut node = checkpoint("legacy-done", "send", "L3", false);
    node.state = "completed".into();
    repo.record_job_node(&node).unwrap();
    repo.recover_expired_jobs(at(7)).unwrap();
    assert_eq!(
        repo.get_job("legacy-done").unwrap().unwrap().state,
        "unknown"
    );
    assert_eq!(
        repo.list_job_nodes("legacy-done").unwrap()[0].state,
        "unknown"
    );
    assert!(repo
        .resolve_unknown_job_node("legacy-done", "send", None, at(8))
        .unwrap());
    assert!(repo.list_job_nodes("legacy-done").unwrap().is_empty());
}

#[test]
fn stale_workers_cannot_bypass_human_review_or_replace_finished_results() {
    let repo = Repo::open_in_memory().unwrap();
    repo.enqueue_job(&job("cas", "checkpoint-cas", at(0)))
        .unwrap();
    repo.acquire_job_lease("worker", at(1), Duration::seconds(5))
        .unwrap();
    let mut node = DurableJobNode {
        checkpoint: checkpoint("cas", "send", "L3", false),
        node_hash: "hash".into(),
        result: None,
    };
    repo.save_durable_job_node("worker", &node).unwrap();
    repo.recover_expired_jobs(at(7)).unwrap();
    node.checkpoint.state = "completed".into();
    node.result = Some(json!({"output":"unverified"}));
    assert!(repo.save_durable_job_node("worker", &node).is_err());
    assert_eq!(
        repo.list_durable_job_nodes("cas").unwrap()[0]
            .checkpoint
            .state,
        "unknown"
    );
    repo.resolve_unknown_job_node("cas", "send", None, at(8))
        .unwrap();
    repo.acquire_job_lease("worker", at(9), Duration::seconds(30))
        .unwrap();
    node.checkpoint.updated_at = at(10);
    repo.save_durable_job_node("worker", &node).unwrap();
    node.result = Some(json!({"output":"replacement"}));
    assert!(repo.save_durable_job_node("worker", &node).is_err());
    assert_eq!(
        repo.list_durable_job_nodes("cas").unwrap()[0].result,
        Some(json!({"output":"unverified"}))
    );
}

#[test]
fn recovery_preserves_a_preexisting_unknown_checkpoint() {
    let repo = Repo::open_in_memory().unwrap();
    repo.enqueue_job(&job("preexisting", "preexisting-unknown", at(0)))
        .unwrap();
    repo.acquire_job_lease("dead", at(1), Duration::seconds(5))
        .unwrap();
    let mut node = checkpoint("preexisting", "send", "L3", false);
    node.state = "unknown".into();
    repo.record_job_node(&node).unwrap();
    repo.recover_expired_jobs(at(7)).unwrap();
    assert_eq!(
        repo.get_job("preexisting").unwrap().unwrap().state,
        "unknown"
    );
}
