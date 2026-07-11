use lumo_agent::{
    AgentProfile, AgentProfileDraft, CapabilityCatalog, CapabilityCatalogBuilder,
    CapabilityCatalogError, CapabilityDescriptor, CapabilitySource, RiskLevel,
};

fn capability(id: &str, alias: &str, risk: RiskLevel, enabled: bool) -> CapabilityDescriptor {
    let mut descriptor =
        CapabilityDescriptor::mcp("test", id, serde_json::json!({"type": "object"}));
    descriptor.id = id.into();
    descriptor.name = id.into();
    descriptor.aliases = vec![alias.into()];
    descriptor.risk = risk;
    descriptor.enabled = enabled;
    descriptor.refresh_version_hash();
    descriptor
}

fn profile(visible_capabilities: impl IntoIterator<Item = &'static str>) -> AgentProfile {
    AgentProfile::validate(
        AgentProfileDraft {
            visible_capabilities: visible_capabilities
                .into_iter()
                .map(str::to_owned)
                .collect(),
            ..AgentProfileDraft::default()
        },
        &CapabilityCatalog::new(vec![
            capability("allowed", "allowed", RiskLevel::L1, true),
            capability("disabled", "disabled", RiskLevel::L0, false),
            capability("risky", "risky", RiskLevel::L2, true),
            capability("hidden", "hidden", RiskLevel::L0, true),
        ])
        .unwrap(),
    )
    .unwrap()
}

#[test]
fn constructor_rejects_duplicate_ids() {
    let descriptor = capability("duplicate", "run", RiskLevel::L0, true);

    let error = CapabilityCatalog::new(vec![descriptor.clone(), descriptor]).unwrap_err();

    assert_eq!(
        error,
        CapabilityCatalogError::DuplicateId("duplicate".into())
    );
}

#[test]
fn constructor_rejects_invalid_version_hashes() {
    let mut descriptor = capability("changed", "run", RiskLevel::L0, true);
    descriptor.description = "changed after hashing".into();

    let error = CapabilityCatalog::new(vec![descriptor]).unwrap_err();

    assert_eq!(
        error,
        CapabilityCatalogError::InvalidVersionHash("changed".into())
    );
}

#[test]
fn alias_collisions_return_unicode_normalized_candidates_in_id_order() {
    let catalog = CapabilityCatalog::new(vec![
        capability("zeta", "  İŞi BAŞLAT  ", RiskLevel::L0, true),
        capability("alpha", "i̇şi başlat", RiskLevel::L0, true),
    ])
    .unwrap();

    let ids = catalog
        .exact_alias("  İşi Başlat ")
        .into_iter()
        .map(|descriptor| descriptor.id.clone())
        .collect::<Vec<_>>();

    assert_eq!(ids, ["alpha", "zeta"]);
    assert_eq!(
        catalog
            .all()
            .into_iter()
            .map(|descriptor| descriptor.id.clone())
            .collect::<Vec<_>>(),
        ["alpha", "zeta"]
    );
}

#[test]
fn visibility_filters_disabled_risky_and_profile_hidden_capabilities() {
    let catalog = CapabilityCatalog::new(vec![
        capability("allowed", "allowed", RiskLevel::L1, true),
        capability("disabled", "disabled", RiskLevel::L0, false),
        capability("risky", "risky", RiskLevel::L2, true),
        capability("hidden", "hidden", RiskLevel::L0, true),
    ])
    .unwrap();
    let profile = profile(["allowed", "disabled", "risky"]);

    let visible = catalog
        .visible_for(&profile)
        .into_iter()
        .map(|descriptor| descriptor.id.clone())
        .collect::<Vec<_>>();

    assert_eq!(visible, ["allowed"]);
}

#[test]
fn source_builder_accepts_normalized_flow_skill_and_mcp_descriptors() {
    let mut flow = capability("flow:daily", "daily", RiskLevel::L0, true);
    flow.source = CapabilitySource::Flow {
        path: "flows/daily.yaml".into(),
    };
    flow.refresh_version_hash();
    let mut skill = capability("skill:greet", "greet", RiskLevel::L0, true);
    skill.source = CapabilitySource::Skill {
        name: "greet".into(),
        source: "local".into(),
    };
    skill.refresh_version_hash();
    let mcp = capability("mcp:erp/query", "query", RiskLevel::L0, true);

    let catalog = CapabilityCatalogBuilder::new()
        .flows([flow])
        .skills([skill])
        .mcp([mcp])
        .build()
        .unwrap();

    assert_eq!(catalog.all().len(), 3);
    assert!(catalog.get("skill:greet").is_some());
}
