use lumo_agent::{CapabilityDescriptor, CapabilitySource, RiskLevel};

fn descriptor() -> CapabilityDescriptor {
    CapabilityDescriptor::mcp("erp", "query_orders", serde_json::json!({"type": "object"}))
}

#[test]
fn mcp_capability_id_is_stable() {
    let c = descriptor();
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

#[test]
fn same_descriptor_has_same_hash() {
    assert_eq!(descriptor(), descriptor());
}

#[test]
fn mcp_source_changes_affect_hash() {
    let original = descriptor();
    let different_server = CapabilityDescriptor::mcp(
        "warehouse",
        "query_orders",
        serde_json::json!({"type": "object"}),
    );
    let different_tool = CapabilityDescriptor::mcp(
        "erp",
        "query_customers",
        serde_json::json!({"type": "object"}),
    );

    assert_ne!(original.version_hash, different_server.version_hash);
    assert_ne!(original.version_hash, different_tool.version_hash);
}

#[test]
fn input_schema_changes_affect_hash() {
    let original = descriptor();
    let changed =
        CapabilityDescriptor::mcp("erp", "query_orders", serde_json::json!({"type": "array"}));

    assert_ne!(original.version_hash, changed.version_hash);
}

#[test]
fn nested_object_key_order_does_not_affect_hash() {
    let mut nested_left = serde_json::Map::new();
    nested_left.insert("type".into(), serde_json::json!("string"));
    nested_left.insert("description".into(), serde_json::json!("Order ID"));
    let mut left = serde_json::Map::new();
    left.insert("type".into(), serde_json::json!("object"));
    left.insert("properties".into(), serde_json::Value::Object(nested_left));

    let mut nested_right = serde_json::Map::new();
    nested_right.insert("description".into(), serde_json::json!("Order ID"));
    nested_right.insert("type".into(), serde_json::json!("string"));
    let mut right = serde_json::Map::new();
    right.insert("properties".into(), serde_json::Value::Object(nested_right));
    right.insert("type".into(), serde_json::json!("object"));

    let left = CapabilityDescriptor::mcp("erp", "query_orders", left.into());
    let right = CapabilityDescriptor::mcp("erp", "query_orders", right.into());

    assert_eq!(left.version_hash, right.version_hash);
}

#[test]
fn mutation_invalidates_hash_until_refreshed() {
    let mut capability = descriptor();
    let original_hash = capability.version_hash.clone();
    capability.input_schema = serde_json::json!({"type": "array"});

    assert!(!capability.has_valid_version_hash());
    capability.refresh_version_hash();
    assert!(capability.has_valid_version_hash());
    assert_ne!(capability.version_hash, original_hash);
}

#[test]
fn serde_uses_camel_case_keys_and_mcp_kind() {
    let json = serde_json::to_value(descriptor()).unwrap();

    assert!(json.get("versionHash").is_some());
    assert!(json.get("inputSchema").is_some());
    assert_eq!(json["source"]["kind"], "mcp");
}
