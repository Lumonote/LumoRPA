//! Shared output contracts for actions whose result shape is fixed. An empty
//! schema deliberately means arbitrary JSON, rather than an inferred object.
use once_cell::sync::Lazy;
use serde_json::{json, Value};

macro_rules! output_schema {
    ($name:ident, $schema:expr) => {
        pub(crate) fn $name() -> &'static Value {
            static SCHEMA: Lazy<Value> = Lazy::new(|| $schema);
            &SCHEMA
        }
    };
}

output_schema!(any, json!({}));
output_schema!(string, json!({ "type": "string" }));
output_schema!(boolean, json!({ "type": "boolean" }));
output_schema!(integer, json!({ "type": "integer" }));
output_schema!(array, json!({ "type": "array", "items": {} }));
output_schema!(
    strings,
    json!({ "type": "array", "items": { "type": "string" } })
);
output_schema!(object, json!({ "type": "object" }));
output_schema!(nullable_number, json!({ "type": ["number", "null"] }));
output_schema!(
    path,
    json!({
        "type": "object", "required": ["path"],
        "properties": { "path": { "type": "string" } }, "additionalProperties": false
    })
);
output_schema!(
    moved,
    json!({
        "type": "object", "required": ["from", "to"],
        "properties": { "from": { "type": "string" }, "to": { "type": "string" } },
        "additionalProperties": false
    })
);
output_schema!(
    http_response,
    json!({
        "type": "object", "required": ["status", "headers", "text", "json"],
        "properties": {
            "status": { "type": "integer" },
            "headers": { "type": "object", "additionalProperties": { "anyOf": [
                { "type": "string" }, { "type": "array", "items": { "type": "string" } }
            ] } },
            "text": { "type": "string" }, "json": {}
        }, "additionalProperties": false
    })
);

pub(crate) fn file_metadata() -> &'static Value {
    static SCHEMA: Lazy<Value> = Lazy::new(|| {
        json!({
            "type": "object",
            "required": ["path", "name", "type", "len", "readonly", "modified_ms", "created_ms"],
            "properties": {
                "path": { "type": "string" }, "name": { "type": "string" },
                "type": { "type": "string", "enum": ["symlink", "directory", "file", "other"] },
                "len": { "type": "integer" }, "readonly": { "type": "boolean" },
                "modified_ms": { "type": ["integer", "null"] },
                "created_ms": { "type": ["integer", "null"] }
            }, "additionalProperties": false
        })
    });
    &SCHEMA
}
