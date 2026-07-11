use lumo_agent::{CapabilityDescriptor, CapabilitySource, RiskLevel};

#[test]
fn mcp_capability_id_is_stable() {
    let c = CapabilityDescriptor::mcp("erp", "query_orders", serde_json::json!({"type":"object"}));
    assert_eq!(c.id, "mcp:erp/query_orders");
    assert_eq!(
        c.source,
        CapabilitySource::Mcp {
            server: "erp".into(),
            tool: "query_orders".into()
        }
    );
    assert_eq!(c.risk, RiskLevel::L0);
    let json = serde_json::to_value(&c).unwrap();
    assert_eq!(json["versionHash"].as_str().unwrap().len(), 64);
}
