use lumo_agent::{
    AgentProfile, AgentProfileDraft, CapabilityCatalog, CapabilityDescriptor, ProfileError,
    RiskLevel,
};

fn catalog() -> CapabilityCatalog {
    CapabilityCatalog::new(vec![CapabilityDescriptor::mcp(
        "erp",
        "query_orders",
        serde_json::json!({"type": "object"}),
    )])
    .unwrap()
}

#[test]
fn validation_rejects_invalid_execution_bounds() {
    let cases = [
        (
            AgentProfileDraft {
                max_steps: 0,
                ..AgentProfileDraft::default()
            },
            ProfileError::MaxStepsOutOfRange(0),
        ),
        (
            AgentProfileDraft {
                max_steps: 101,
                ..AgentProfileDraft::default()
            },
            ProfileError::MaxStepsOutOfRange(101),
        ),
        (
            AgentProfileDraft {
                max_parallel: 0,
                ..AgentProfileDraft::default()
            },
            ProfileError::MaxParallelOutOfRange(0),
        ),
        (
            AgentProfileDraft {
                max_parallel: 17,
                ..AgentProfileDraft::default()
            },
            ProfileError::MaxParallelOutOfRange(17),
        ),
        (
            AgentProfileDraft {
                max_runtime_ms: 0,
                ..AgentProfileDraft::default()
            },
            ProfileError::MaxRuntimeMustBePositive,
        ),
        (
            AgentProfileDraft {
                max_tokens: 0,
                ..AgentProfileDraft::default()
            },
            ProfileError::MaxTokensMustBePositive,
        ),
    ];

    for (draft, expected) in cases {
        assert_eq!(
            AgentProfile::validate(draft, &catalog()).unwrap_err(),
            expected
        );
    }
}

#[test]
fn validation_rejects_unknown_visible_capability() {
    let error = AgentProfile::validate(
        AgentProfileDraft {
            visible_capabilities: vec!["missing:tool".into()],
            ..AgentProfileDraft::default()
        },
        &catalog(),
    )
    .unwrap_err();

    assert_eq!(
        error,
        ProfileError::UnknownCapability("missing:tool".into())
    );
}

#[test]
fn safe_defaults_are_bounded_and_low_risk() {
    let draft = AgentProfileDraft::default();

    assert_eq!(draft.max_steps, 20);
    assert_eq!(draft.max_parallel, 4);
    assert_eq!(draft.max_auto_risk, RiskLevel::L1);
    assert!(draft.max_runtime_ms > 0);
    assert!(draft.max_tokens > 0);
}

#[test]
fn profile_serde_roundtrip_uses_camel_case() {
    let profile = AgentProfile::validate(
        AgentProfileDraft {
            visible_capabilities: vec!["mcp:erp/query_orders".into()],
            ..AgentProfileDraft::default()
        },
        &catalog(),
    )
    .unwrap();

    let json = serde_json::to_value(&profile).unwrap();
    assert!(json.get("plannerProvider").is_some());
    assert!(json.get("visibleCapabilities").is_some());
    assert!(json.get("maxAutoRisk").is_some());
    assert_eq!(
        serde_json::from_value::<AgentProfile>(json).unwrap(),
        profile
    );
}
