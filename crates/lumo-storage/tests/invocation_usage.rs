use chrono::Utc;
use lumo_storage::{InvocationAttemptRow, InvocationFlowRunRow, Repo};

fn attempt(number: u32) -> InvocationAttemptRow {
    InvocationAttemptRow {
        run_id: "agent".into(),
        node_id: "node".into(),
        attempt: number,
        capability_id: "flow:test".into(),
        source_kind: "flow".into(),
        flow_run_id: Some(format!("flow-{number}")),
        state: "running".into(),
        usage_state: "pending".into(),
        budget_sensitive: true,
        tokens_used: None,
        cost_usd_micro: None,
        duration_ms: 0,
        started_at: Utc::now(),
        finished_at: None,
        error: None,
        revision: 0,
        resolved_by: None,
        resolution_reason: None,
        resolved_at: None,
    }
}

#[test]
fn startup_recovery_exposes_crashed_call_charges_without_claiming_completeness() {
    use lumo_storage::{AiCallInsert, FlowRunRow};
    let repo = Repo::open_in_memory().unwrap();
    repo.begin_invocation_attempt(&attempt(1)).unwrap();
    repo.record_invocation_flow_run(&flow("flow-1", None, None, true))
        .unwrap();
    repo.create_run(&FlowRunRow {
        id: "flow-1".into(),
        flow_id: "test".into(),
        flow_version: "1".into(),
        trigger_kind: "agent:flow".into(),
        inputs: serde_json::json!({}),
        outputs: None,
        state: "running".into(),
        worker_id: None,
        started_at: Some(Utc::now()),
        finished_at: None,
        cost_token: 0,
        cost_usd_micro: 0,
        trace_id: None,
    })
    .unwrap();
    repo.record_ai_call(AiCallInsert {
        flow_run_id: "flow-1",
        step_id: None,
        helper: "chat",
        provider: "test",
        model: "test",
        input_tokens: 3,
        output_tokens: 2,
        latency_ms: 1,
        cost_usd_micro: 7,
    })
    .unwrap();
    assert_eq!(repo.recover_interrupted_invocations().unwrap(), 1);
    repo.reconcile_invocation_usage("agent").unwrap();
    let row = repo
        .get_invocation_attempt("agent", "node", 1)
        .unwrap()
        .unwrap();
    assert_eq!(row.state, "dropped");
    assert_eq!(row.usage_state, "unknown");
    assert_eq!(row.tokens_used, Some(5));
    assert_eq!(row.cost_usd_micro, Some(7));
    assert_eq!(
        repo.list_invocation_flow_runs("agent").unwrap()[0].state,
        "interrupted"
    );
}
fn flow(
    id: &str,
    parent: Option<&str>,
    tokens: Option<u64>,
    unknown: bool,
) -> InvocationFlowRunRow {
    InvocationFlowRunRow {
        flow_run_id: id.into(),
        run_id: "agent".into(),
        node_id: "node".into(),
        attempt: 1,
        parent_flow_run_id: parent.map(str::to_string),
        state: if tokens.is_some() {
            "failed"
        } else {
            "running"
        }
        .into(),
        usage_unknown: unknown,
        tokens_used: tokens,
        cost_usd_micro: tokens.map(|n| n * 10),
        duration_ms: 5,
    }
}

#[test]
fn nested_flow_usage_is_monotonic_and_aggregated_once_after_failure_and_late_cancel() {
    let repo = Repo::open_in_memory().unwrap();
    let mut row = attempt(1);
    repo.begin_invocation_attempt(&row).unwrap();
    assert!(repo.begin_invocation_attempt(&row).is_err());
    repo.record_invocation_flow_run(&flow("flow-1", None, None, true))
        .unwrap();
    repo.record_invocation_flow_run(&flow("child", Some("flow-1"), Some(7), false))
        .unwrap();
    row.state = "timed_out".into();
    row.usage_state = "unknown".into();
    row.duration_ms = 20;
    row.finished_at = Some(Utc::now());
    repo.finish_invocation_attempt(&row).unwrap();
    let saved = repo
        .get_invocation_attempt("agent", "node", 1)
        .unwrap()
        .unwrap();
    assert_eq!(saved.tokens_used, Some(7));
    assert_eq!(saved.usage_state, "unknown");
    repo.record_invocation_flow_run(&flow("flow-1", None, Some(3), false))
        .unwrap();
    repo.record_invocation_flow_run(&flow("child", Some("flow-1"), Some(7), false))
        .unwrap();
    let saved = repo
        .get_invocation_attempt("agent", "node", 1)
        .unwrap()
        .unwrap();
    assert_eq!(saved.tokens_used, Some(10));
    assert_eq!(saved.cost_usd_micro, Some(100));
    assert_eq!(saved.usage_state, "recorded");
    assert_eq!(saved.state, "timed_out");
    assert_eq!(saved.duration_ms, 20);
    // A late Started notification cannot reopen a terminal VM association.
    repo.record_invocation_flow_run(&flow("child", Some("flow-1"), None, true))
        .unwrap();
    assert_eq!(
        repo.get_invocation_attempt("agent", "node", 1)
            .unwrap()
            .unwrap()
            .tokens_used,
        Some(10)
    );
}

#[test]
fn unknown_attempts_remain_distinct_from_measured_zero_and_retry_costs_survive_reopen() {
    let temporary = tempfile::tempdir().unwrap();
    let path = temporary.path().join("ledger.sqlite");
    let repo = Repo::open(&path).unwrap();
    let mut failed = attempt(1);
    repo.begin_invocation_attempt(&failed).unwrap();
    failed.state = "dropped".into();
    failed.usage_state = "unknown".into();
    failed.finished_at = Some(Utc::now());
    repo.finish_invocation_attempt(&failed).unwrap();
    let mut retried = attempt(2);
    repo.begin_invocation_attempt(&retried).unwrap();
    retried.state = "completed".into();
    retried.usage_state = "recorded".into();
    retried.tokens_used = Some(0);
    retried.cost_usd_micro = Some(0);
    retried.finished_at = Some(Utc::now());
    repo.finish_invocation_attempt(&retried).unwrap();
    let rows = Repo::open(&path)
        .unwrap()
        .list_invocation_attempts("agent")
        .unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].tokens_used, None);
    assert_eq!(rows[0].usage_state, "unknown");
    assert_eq!(rows[1].tokens_used, Some(0));
    assert_eq!(rows[1].usage_state, "recorded");
}

#[test]
fn usage_migration_from_v6_preserves_existing_runs() {
    let temporary = tempfile::tempdir().unwrap();
    let path = temporary.path().join("old.sqlite");
    let repo = Repo::open(&path).unwrap();
    repo.with_raw(|conn|conn.execute_batch("DROP TABLE agent_invocation_usage_resolutions;DROP TABLE agent_invocation_flow_runs;DROP TABLE agent_invocation_attempts;PRAGMA user_version=6;")).unwrap();
    drop(repo);
    let reopened = Repo::open(&path).unwrap();
    reopened.begin_invocation_attempt(&attempt(1)).unwrap();
    assert_eq!(reopened.list_invocation_attempts("agent").unwrap().len(), 1);
    assert_eq!(
        reopened
            .with_raw(|conn| conn.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0)))
            .unwrap(),
        7
    );
}

#[test]
fn manual_reconciliation_requires_terminal_state_audit_and_current_revision() {
    use lumo_storage::InvocationUsageResolution;
    let repo = Repo::open_in_memory().unwrap();
    let mut row = attempt(1);
    repo.begin_invocation_attempt(&row).unwrap();
    let mut resolution = InvocationUsageResolution {
        run_id: "agent".into(),
        node_id: "node".into(),
        attempt: 1,
        expected_revision: 0,
        tokens_used: 5,
        cost_usd_micro: 70,
        resolved_by: "operator".into(),
        reason: "checked provider invoice".into(),
    };
    assert!(
        !repo.resolve_invocation_usage(&resolution).unwrap(),
        "an active call cannot be reconciled"
    );
    repo.record_invocation_flow_run(&flow("flow-1", None, Some(3), true))
        .unwrap();
    row.state = "failed".into();
    row.usage_state = "unknown".into();
    row.finished_at = Some(Utc::now());
    repo.finish_invocation_attempt(&row).unwrap();
    let current = repo
        .get_invocation_attempt("agent", "node", 1)
        .unwrap()
        .unwrap();
    assert!(
        !repo.resolve_invocation_usage(&resolution).unwrap(),
        "stale revision must fail"
    );
    resolution.expected_revision = current.revision;
    resolution.tokens_used = 2;
    assert!(
        !repo.resolve_invocation_usage(&resolution).unwrap(),
        "known charges cannot be reduced"
    );
    resolution.tokens_used = 5;
    resolution.reason = " ".into();
    assert!(repo.resolve_invocation_usage(&resolution).is_err());
    resolution.reason = "checked provider invoice".into();
    assert!(repo.resolve_invocation_usage(&resolution).unwrap());
    let verified = repo
        .get_invocation_attempt("agent", "node", 1)
        .unwrap()
        .unwrap();
    assert_eq!(verified.tokens_used, Some(5));
    assert_eq!(verified.usage_state, "recorded");
    assert_eq!(verified.resolved_by.as_deref(), Some("operator"));
    assert_eq!(
        verified.resolution_reason.as_deref(),
        Some("checked provider invoice")
    );
    // Reconciliation and a duplicated late report cannot overwrite the
    // operator's larger confirmed total or invalidate an unchanged revision.
    repo.reconcile_invocation_usage("agent").unwrap();
    repo.record_invocation_flow_run(&flow("flow-1", None, Some(3), true))
        .unwrap();
    assert_eq!(
        repo.get_invocation_attempt("agent", "node", 1)
            .unwrap()
            .unwrap()
            .revision,
        verified.revision
    );
    repo.record_invocation_flow_run(&flow("flow-1", None, Some(8), true))
        .unwrap();
    let revised = repo
        .get_invocation_attempt("agent", "node", 1)
        .unwrap()
        .unwrap();
    assert_eq!(revised.tokens_used, Some(8));
    assert_eq!(revised.usage_state, "unknown");
    assert_eq!(
        repo.with_raw(|conn| conn.query_row(
            "SELECT COUNT(*) FROM agent_invocation_usage_resolutions",
            [],
            |row| row.get::<_, i64>(0)
        ))
        .unwrap(),
        1
    );
}
