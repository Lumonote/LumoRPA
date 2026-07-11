use lumo_agent::{
    ContentOrigin, ImprovementTarget, ProposalBuilder, TraceMiner, TraceRecord,
};
use serde_json::json;

#[test]
fn miner_aggregates_failures_retries_corrections_and_replacements_without_secrets() {
    let records = vec![
        TraceRecord {
            run_id: "run-1".into(),
            capability_id: "old-tool".into(),
            completed: true,
            success: false,
            latency_ms: 900,
            cost_usd_micro: 40,
            retry_count: 2,
            manual_correction: Some("use new-tool with Bearer top-secret".into()),
            replacement_capability: Some("new-tool".into()),
            payload: json!({"password": "secret-1", "input": "safe"}),
            origin: ContentOrigin::ToolResult,
        },
        TraceRecord {
            run_id: "run-2".into(),
            capability_id: "old-tool".into(),
            completed: true,
            success: true,
            latency_ms: 300,
            cost_usd_micro: 10,
            retry_count: 1,
            manual_correction: Some("use new-tool".into()),
            replacement_capability: Some("new-tool".into()),
            payload: json!({"authorization": "Bearer secret-2"}),
            origin: ContentOrigin::McpTool,
        },
        TraceRecord {
            run_id: "active-run".into(),
            capability_id: "old-tool".into(),
            completed: false,
            success: false,
            latency_ms: 99_000,
            cost_usd_micro: 99_000,
            retry_count: 99,
            manual_correction: None,
            replacement_capability: None,
            payload: json!({"token": "must-not-appear"}),
            origin: ContentOrigin::Web,
        },
    ];

    let summary = TraceMiner::mine(&records);
    let aggregate = &summary.by_capability["old-tool"];
    assert_eq!(aggregate.runs, 2);
    assert_eq!(aggregate.successes, 1);
    assert_eq!(aggregate.retries, 3);
    assert_eq!(aggregate.total_latency_ms, 1_200);
    assert_eq!(aggregate.total_cost_usd_micro, 50);
    assert_eq!(aggregate.replacements["new-tool"], 2);
    assert_eq!(summary.source_run_ids, ["run-1", "run-2"]);

    let encoded = serde_json::to_string(&summary).unwrap();
    for secret in ["top-secret", "secret-1", "secret-2", "must-not-appear"] {
        assert!(!encoded.contains(secret), "secret leaked: {encoded}");
    }
}

#[test]
fn deterministic_builder_proposes_repeated_replacement_without_raw_payloads() {
    let summary = TraceMiner::mine(&[
        TraceRecord::completed_failure("r1", "old").with_replacement("new"),
        TraceRecord::completed_failure("r2", "old").with_replacement("new"),
    ]);
    let proposals = ProposalBuilder::deterministic(&summary, "base-v1").unwrap();

    assert_eq!(proposals.len(), 1);
    assert_eq!(
        proposals[0].target,
        ImprovementTarget::RouterExample {
            capability_id: "old".into()
        }
    );
    assert_eq!(proposals[0].patch["preferredCapability"], json!("new"));
    assert_eq!(proposals[0].origin, ContentOrigin::Trace);
}
