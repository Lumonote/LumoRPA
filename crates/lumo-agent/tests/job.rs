use chrono::{TimeZone, Utc};
use lumo_agent::{
    crash_recovery_disposition, AgentJob, AgentPlan, JobSchedule, JobState, PlanNode,
    RecoveryDisposition, RecoveryNode, RiskLevel,
};
use serde_json::json;

fn plan() -> AgentPlan {
    AgentPlan::new(
        "plan-1",
        "prepare the report",
        vec![PlanNode {
            id: "collect".into(),
            depends_on: vec![],
            capability_id: "skill:collect".into(),
            arguments: json!({}),
            risk: RiskLevel::L1,
            timeout_ms: 30_000,
            retry_limit: 2,
            expected_output_schema: None,
        }],
    )
}

#[test]
fn job_states_use_stable_wire_names() {
    let cases = [
        (JobState::Queued, "queued"),
        (JobState::Running, "running"),
        (JobState::Waiting, "waiting"),
        (JobState::Paused, "paused"),
        (JobState::Completed, "completed"),
        (JobState::Failed, "failed"),
        (JobState::Unknown, "unknown"),
    ];

    for (state, expected) in cases {
        assert_eq!(state.as_str(), expected);
        assert_eq!(serde_json::to_value(state).unwrap(), json!(expected));
        assert_eq!(
            serde_json::from_value::<JobState>(json!(expected)).unwrap(),
            state
        );
    }
}

#[test]
fn one_shot_and_cron_schedules_calculate_next_run() {
    let noon = Utc.with_ymd_and_hms(2026, 7, 11, 12, 0, 0).unwrap();
    let one_shot = JobSchedule::one_shot(noon);
    assert_eq!(
        one_shot
            .next_after(noon - chrono::Duration::seconds(1))
            .unwrap(),
        Some(noon)
    );
    assert_eq!(one_shot.next_after(noon).unwrap(), None);

    let cron = JobSchedule::cron("0 */5 * * * *").unwrap();
    assert_eq!(
        cron.next_after(Utc.with_ymd_and_hms(2026, 7, 11, 12, 1, 0).unwrap())
            .unwrap(),
        Some(Utc.with_ymd_and_hms(2026, 7, 11, 12, 5, 0).unwrap())
    );
}

#[test]
fn invalid_cron_and_empty_idempotency_keys_are_rejected() {
    assert!(JobSchedule::cron("not cron").is_err());
    let now = Utc.with_ymd_and_hms(2026, 7, 11, 12, 0, 0).unwrap();
    assert!(AgentJob::new("job-1", "  ", plan(), JobSchedule::one_shot(now), now).is_err());
}

#[test]
fn new_job_is_queued_and_carries_idempotency_identity() {
    let now = Utc.with_ymd_and_hms(2026, 7, 11, 12, 0, 0).unwrap();
    let job = AgentJob::new(
        "job-1",
        "daily-report:2026-07-11",
        plan(),
        JobSchedule::one_shot(now),
        now,
    )
    .unwrap();

    assert_eq!(job.state, JobState::Queued);
    assert_eq!(job.idempotency_key, "daily-report:2026-07-11");
    assert_eq!(job.next_run_at, Some(now));
    assert_eq!(job.attempts, 0);
}

#[test]
fn crash_recovery_only_replays_idempotent_nodes() {
    let idempotent = RecoveryNode::new("read", RiskLevel::L1, true);
    assert_eq!(
        crash_recovery_disposition(&[idempotent]),
        RecoveryDisposition::Resume
    );

    let low_risk_side_effect = RecoveryNode::new("cache", RiskLevel::L1, false);
    assert_eq!(
        crash_recovery_disposition(&[low_risk_side_effect]),
        RecoveryDisposition::Fail
    );

    let uncertain_payment = RecoveryNode::new("pay", RiskLevel::L3, false);
    assert_eq!(
        crash_recovery_disposition(&[uncertain_payment]),
        RecoveryDisposition::Unknown
    );
}
