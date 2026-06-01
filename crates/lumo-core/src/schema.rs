//! B2 (F-17): minimal JSON-Schema validation of a step's rendered `with:`
//! against the action's [`crate::action::Action::schema`].
//!
//! Covers exactly the subset the hand-written action schemas use — `type`,
//! `required`, `properties`, `additionalProperties`, `enum`, and array `items`
//! — rather than pulling a full JSON-Schema engine (the project deliberately
//! hand-writes its schemas to avoid that weight).
//!
//! A value that is still an unrendered `{{ }}` template — or a `${{ vault }}`
//! placeholder — is left unchecked: its real value (and type) only exists at
//! dispatch time, so type/enum checks would be meaningless pre-render. This
//! keeps the validator sound both in the VM (post-render) and when used to
//! audit raw flow text.

use serde_json::{Map, Value};

/// Validate `input` against `schema`. `Ok(())` on success; `Err(msg)` carries a
/// human-readable `with`-rooted path and reason for the first violation.
pub fn validate_input(schema: &Value, input: &Value) -> Result<(), String> {
    validate_at(schema, input, "")
}

fn validate_at(schema: &Value, value: &Value, path: &str) -> Result<(), String> {
    let Some(obj) = schema.as_object() else {
        return Ok(()); // a non-object schema accepts anything
    };
    if obj.is_empty() {
        return Ok(()); // `{}` accepts anything
    }
    // Unrendered `{{ }}` template / `${{ vault }}` placeholder ⇒ defer to runtime.
    if let Value::String(s) = value {
        if s.contains("{{") {
            return Ok(());
        }
    }
    if let Some(Value::Array(allowed)) = obj.get("enum") {
        if !allowed.iter().any(|a| a == value) {
            return Err(format!(
                "{}: {} is not one of {}",
                at(path),
                short(value),
                Value::Array(allowed.clone())
            ));
        }
    }
    match obj.get("type").and_then(|t| t.as_str()) {
        Some(ty) => {
            // `null` is leniently accepted for any declared type (optional field).
            if !value.is_null() && !type_matches(ty, value) {
                return Err(format!("{}: expected {ty}, got {}", at(path), short(value)));
            }
            match ty {
                "object" => validate_object(obj, value, path)?,
                "array" => validate_array(obj, value, path)?,
                _ => {}
            }
        }
        None => {
            // No explicit `type` but object-shaped keywords present ⇒ treat as object.
            if obj.contains_key("properties")
                || obj.contains_key("required")
                || obj.contains_key("additionalProperties")
            {
                validate_object(obj, value, path)?;
            }
        }
    }
    Ok(())
}

fn validate_object(schema: &Map<String, Value>, value: &Value, path: &str) -> Result<(), String> {
    // A null/absent value is treated as an empty object so `required` still fires.
    let empty = Map::new();
    let map = match value {
        Value::Object(m) => m,
        Value::Null => &empty,
        _ => return Ok(()), // non-object (type error already reported by caller)
    };
    let props = schema.get("properties").and_then(|p| p.as_object());
    if let Some(Value::Array(req)) = schema.get("required") {
        for r in req {
            if let Some(key) = r.as_str() {
                if !map.contains_key(key) {
                    return Err(format!("{}: missing required field `{key}`", at(path)));
                }
            }
        }
    }
    if matches!(schema.get("additionalProperties"), Some(Value::Bool(false))) {
        for k in map.keys() {
            if !props.map(|p| p.contains_key(k)).unwrap_or(false) {
                return Err(format!("{}: unknown field `{k}`", at(path)));
            }
        }
    }
    if let Some(props) = props {
        for (k, sub) in props {
            if let Some(v) = map.get(k) {
                let child = if path.is_empty() {
                    k.clone()
                } else {
                    format!("{path}.{k}")
                };
                validate_at(sub, v, &child)?;
            }
        }
    }
    Ok(())
}

fn validate_array(schema: &Map<String, Value>, value: &Value, path: &str) -> Result<(), String> {
    let Value::Array(arr) = value else {
        return Ok(());
    };
    if let Some(items) = schema.get("items") {
        for (i, v) in arr.iter().enumerate() {
            validate_at(items, v, &format!("{path}[{i}]"))?;
        }
    }
    Ok(())
}

fn type_matches(ty: &str, v: &Value) -> bool {
    match ty {
        "string" => v.is_string(),
        // Accept an integral float (e.g. a template that rendered `5` to `5.0`).
        "integer" => v.is_i64() || v.is_u64() || v.as_f64().map(|f| f.fract() == 0.0).unwrap_or(false),
        "number" => v.is_number(),
        "boolean" => v.is_boolean(),
        "array" => v.is_array(),
        "object" => v.is_object(),
        "null" => v.is_null(),
        _ => true, // unknown type keyword ⇒ don't reject
    }
}

fn at(path: &str) -> String {
    if path.is_empty() {
        "with".to_string()
    } else {
        format!("with.{path}")
    }
}

fn short(v: &Value) -> String {
    match v {
        Value::String(s) => format!("\"{s}\""),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn schema() -> Value {
        json!({
            "type": "object",
            "required": ["url"],
            "properties": {
                "url": { "type": "string" },
                "timeout_ms": { "type": "integer" },
                "method": { "type": "string", "enum": ["GET", "POST"] },
                "headers": { "type": "object" }
            },
            "additionalProperties": false
        })
    }

    #[test]
    fn accepts_valid_minimal() {
        assert!(validate_input(&schema(), &json!({ "url": "x" })).is_ok());
    }

    #[test]
    fn accepts_all_known_fields() {
        let v = json!({ "url": "x", "timeout_ms": 5, "method": "GET", "headers": {} });
        assert!(validate_input(&schema(), &v).is_ok());
    }

    #[test]
    fn rejects_missing_required() {
        assert!(validate_input(&schema(), &json!({ "timeout_ms": 5 })).is_err());
    }

    #[test]
    fn rejects_unknown_field() {
        assert!(validate_input(&schema(), &json!({ "url": "x", "bogus": 1 })).is_err());
    }

    #[test]
    fn rejects_wrong_type() {
        assert!(validate_input(&schema(), &json!({ "url": "x", "timeout_ms": "soon" })).is_err());
    }

    #[test]
    fn rejects_enum_violation() {
        assert!(validate_input(&schema(), &json!({ "url": "x", "method": "PATCH" })).is_err());
    }

    #[test]
    fn skips_unrendered_template_value() {
        // A `{{ }}` value can't be type-checked pre-render → accepted.
        let v = json!({ "url": "x", "timeout_ms": "{{ inputs.t }}" });
        assert!(validate_input(&schema(), &v).is_ok());
    }

    #[test]
    fn skips_vault_placeholder_value() {
        let v = json!({ "url": "x", "timeout_ms": "${{ vault.t }}" });
        assert!(validate_input(&schema(), &v).is_ok());
    }

    #[test]
    fn empty_schema_accepts_anything() {
        assert!(validate_input(&json!({}), &json!({ "anything": [1, 2, 3] })).is_ok());
    }

    #[test]
    fn null_input_with_required_fails() {
        assert!(validate_input(&schema(), &Value::Null).is_err());
    }

    #[test]
    fn null_input_without_required_ok() {
        let s = json!({ "type": "object", "properties": { "x": { "type": "string" } } });
        assert!(validate_input(&s, &Value::Null).is_ok());
    }

    #[test]
    fn integer_accepts_integral_float() {
        assert!(validate_input(&schema(), &json!({ "url": "x", "timeout_ms": 5000.0 })).is_ok());
    }

    #[test]
    fn array_items_are_validated() {
        let s = json!({ "type": "array", "items": { "type": "integer" } });
        assert!(validate_input(&s, &json!([1, 2, 3])).is_ok());
        assert!(validate_input(&s, &json!([1, "two"])).is_err());
    }
}
