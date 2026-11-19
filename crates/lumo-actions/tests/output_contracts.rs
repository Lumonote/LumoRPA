//! Studio's port types must describe real action output, including null and
//! multi-valued headers. Dynamic JSON results intentionally stay untyped.
mod common;

use common::{fs_caps, ok, ok_with, Capabilities};
use lumo_core::ActionRegistry;
use serde_json::{json, Value};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn matches(schema: &Value, value: &Value) -> bool {
    if schema == &json!(false) {
        return false;
    }
    if let Some(variants) = schema.get("anyOf").and_then(Value::as_array) {
        if !variants.iter().any(|variant| matches(variant, value)) {
            return false;
        }
    }
    let type_matches = |kind: &str| match kind {
        "null" => value.is_null(),
        "string" => value.is_string(),
        "boolean" => value.is_boolean(),
        "integer" => value.is_i64() || value.is_u64(),
        "number" => value.is_number(),
        "array" => value.is_array(),
        "object" => value.is_object(),
        _ => false,
    };
    match schema.get("type") {
        Some(Value::String(kind)) if !type_matches(kind) => return false,
        Some(Value::Array(types))
            if !types
                .iter()
                .any(|kind| kind.as_str().is_some_and(type_matches)) =>
        {
            return false;
        }
        _ => {}
    }
    if let Some(variants) = schema.get("enum").and_then(Value::as_array) {
        if !variants.contains(value) {
            return false;
        }
    }
    if let Some(object) = value.as_object() {
        if schema["required"].as_array().is_some_and(|keys| {
            keys.iter()
                .any(|key| !object.contains_key(key.as_str().unwrap()))
        }) {
            return false;
        }
        for (key, value) in object {
            if let Some(property) = schema
                .get("properties")
                .and_then(|properties| properties.get(key))
            {
                if !matches(property, value) {
                    return false;
                }
            } else if let Some(extra) = schema.get("additionalProperties") {
                if !matches(extra, value) {
                    return false;
                }
            }
        }
    }
    if let (Some(items), Some(array)) = (schema.get("items"), value.as_array()) {
        if !array.iter().all(|value| matches(items, value)) {
            return false;
        }
    }
    true
}

fn assert_contract(id: &str, output: &Value) {
    let mut registry = ActionRegistry::new();
    lumo_actions::register_all(&mut registry);
    let action = registry.get(id).unwrap();
    let schema = action
        .output_schema()
        .unwrap_or_else(|| panic!("{id} has no output contract"));
    assert!(
        matches(schema, output),
        "{id} output {output} violates {schema}"
    );
}

#[tokio::test]
async fn scalar_collection_and_dynamic_json_outputs_match_declared_types() {
    for (id, input) in [
        ("string.upper", json!({"text": "你好 studio"})),
        ("string.length", json!({"text": "你好"})),
        ("string.split", json!({"text": "one,two", "sep": ","})),
        ("string.contains", json!({"text": "studio", "needle": "io"})),
        ("data.json_format", json!({"value": {"ready": true}})),
        ("json.keys", json!({"value": {"one": 1}})),
        ("json.values", json!({"value": {"one": 1, "two": "hello"}})),
        ("list.length", json!({"items": [1, 2]})),
        ("list.append", json!({"items": [1], "value": "text"})),
        (
            "dictionary.pop",
            json!({"dictionary": {"one": 1}, "key": "one"}),
        ),
        ("math.min", json!({"items": []})),
        ("math.sum", json!({"items": [1, 2]})),
        (
            "text.extract_content_from_text",
            json!({"text": "a1 a2", "pattern": "a(\\d)", "all": true}),
        ),
        (
            "text.extract_content_from_text",
            json!({"text": "none", "pattern": "a(\\d)"}),
        ),
    ] {
        assert_contract(id, &ok(id, input).await);
    }
    let mut registry = ActionRegistry::new();
    lumo_actions::register_all(&mut registry);
    for text in ["null", "false", "7", "\"string\"", "[1]", "{\"a\":1}"] {
        let output = ok("data.json_parse", json!({"text": text})).await;
        assert_contract("data.json_parse", &output);
        assert_eq!(
            registry.get("data.json_parse").unwrap().output_schema(),
            Some(&json!({}))
        );
    }
    for id in [
        "json.get",
        "json.set",
        "json.delete",
        "list.get",
        "dictionary.get_value",
    ] {
        assert_eq!(
            registry.get(id).unwrap().output_schema(),
            Some(&json!({})),
            "{id} must not guess a dynamic type"
        );
    }
}

#[tokio::test]
async fn file_outputs_cover_management_metadata_and_nullable_timestamps() {
    let directory = tempfile::tempdir().unwrap();
    let first = directory.path().join("note.txt");
    let copy = directory.path().join("copy.txt");
    let caps = fs_caps(directory.path());
    for (id, input) in [
        ("file.write", json!({"path": first, "content": "hello"})),
        ("file.append", json!({"path": first, "content": " world"})),
        ("file.read", json!({"path": first})),
        ("file.exists", json!({"path": first})),
        ("file.metadata", json!({"path": first})),
        ("file.list", json!({"path": directory.path()})),
        ("file.wait", json!({"path": first, "stable_ms": 0})),
        ("file.copy", json!({"from": first, "to": copy})),
        ("file.delete", json!({"path": copy, "dry_run": true})),
        ("file.delete", json!({"path": copy})),
        ("file.delete", json!({"path": copy, "missing_ok": true})),
    ] {
        assert_contract(id, &ok_with(id, input, caps.clone()).await);
    }
}

#[tokio::test]
async fn http_outputs_keep_repeated_headers_and_arbitrary_json_body_shapes() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/response"))
        .respond_with(
            ResponseTemplate::new(200)
                .append_header("set-cookie", "one=1")
                .append_header("set-cookie", "two=2")
                .set_body_json(json!(["hello", 7])),
        )
        .mount(&server)
        .await;
    let output = ok_with(
        "http.request",
        json!({"url": format!("{}/response", server.uri())}),
        Capabilities {
            network: vec!["127.0.0.1".into()],
            ..Default::default()
        },
    )
    .await;
    assert!(output["headers"]["set-cookie"].is_array());
    assert_contract("http.request", &output);
    assert_eq!(output["json"], json!(["hello", 7]));
}
