//! F-23: derive an action's `with:` JSON Schema from its `#[derive(JsonSchema)]`
//! input struct, so the Rust deserialize target is the single source of truth —
//! the schema can no longer drift from what `execute` actually parses.
//!
//! The output must satisfy the project's two deliberately-minimal validators —
//! [`lumo_core::schema::validate_input`] (VM, post-render) and the `lumo
//! validate` command — which cover only `type`/`required`/`properties`/
//! `additionalProperties`/`enum`/`items`. Two generator settings keep schemars'
//! output inside that subset:
//!
//! * `option_add_null_type = false` — an `Option<T>` field emits `{"type":"T"}`
//!   rather than `{"type":["T","null"]}`. The core validator reads `type` only
//!   as a single string, so an array `type` would make it silently *stop*
//!   type-checking the field; and both validators already handle an
//!   absent/optional field by other means. A plain `T` is therefore both
//!   faithful to the previous hand-written schemas and stricter than an array.
//! * `inline_subschemas = true` — enums and nested structs are inlined instead
//!   of being emitted as a `$ref` into `definitions`. Neither validator
//!   resolves `$ref`, so a referenced `enum` (e.g. a `mode: ["multipart",
//!   "body"]`) or nested object would otherwise lose its checks entirely.
//!
//! Only the cosmetic `$schema`/`title` meta keys (and an empty `definitions`)
//! are stripped. `default`, `format`, `minimum`, and string-valued
//! `additionalProperties` are kept: they are harmless to both validators and
//! enrich `lumo actions --show` and future Studio form rendering.
//!
//! Pair every conversion with a case in `tests/schema_derive.rs`, which asserts
//! the derived schema still rejects what the hand-written one did — the gate
//! that makes a silent loosening impossible to ship.

use schemars::{gen::SchemaSettings, JsonSchema};
use serde_json::Value;

/// Build the validator-compatible JSON Schema for input struct `T`.
///
/// Typically cached in a per-action `static` and returned from `Action::schema`:
///
/// ```ignore
/// fn schema(&self) -> &'static Value {
///     static S: Lazy<Value> = Lazy::new(crate::schema::derive::<MyIn>);
///     &S
/// }
/// ```
pub fn derive<T: JsonSchema>() -> Value {
    let settings = SchemaSettings::draft07().with(|s| {
        s.option_add_null_type = false;
        s.inline_subschemas = true;
    });
    let root = settings.into_generator().into_root_schema_for::<T>();
    let mut v = serde_json::to_value(root).expect("RootSchema always serializes");
    strip_meta(&mut v);
    v
}

/// Remove cosmetic JSON-Schema meta the validators ignore, so a derived schema
/// reads as cleanly as the hand-written ones it replaces.
fn strip_meta(v: &mut Value) {
    match v {
        Value::Object(map) => {
            map.remove("$schema");
            map.remove("title");
            // `inline_subschemas` can leave an empty `definitions` map behind.
            if matches!(map.get("definitions"), Some(Value::Object(d)) if d.is_empty()) {
                map.remove("definitions");
            }
            for sub in map.values_mut() {
                strip_meta(sub);
            }
        }
        Value::Array(arr) => arr.iter_mut().for_each(strip_meta),
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;
    use serde_json::json;

    #[derive(Deserialize, JsonSchema)]
    #[serde(deny_unknown_fields)]
    #[allow(dead_code)]
    struct Sample {
        name: String,
        count: Option<u32>,
    }

    #[test]
    fn flat_struct_matches_minimal_handwritten_shape() {
        let s = derive::<Sample>();
        assert_eq!(s["type"], json!("object"));
        assert_eq!(
            s["required"],
            json!(["name"]),
            "Option field must not be required"
        );
        assert_eq!(s["properties"]["name"]["type"], json!("string"));
        // The crux: Option<u32> stays a single `"integer"`, NOT ["integer","null"]
        // — otherwise the core validator would stop type-checking the field.
        assert_eq!(s["properties"]["count"]["type"], json!("integer"));
        assert_eq!(
            s["additionalProperties"],
            json!(false),
            "deny_unknown_fields"
        );
        assert!(s.get("$schema").is_none(), "cosmetic $schema stripped");
        assert!(s.get("title").is_none(), "cosmetic title stripped");
        assert!(s.get("definitions").is_none(), "no leftover definitions");
    }

    #[test]
    fn enum_field_is_inlined_with_its_variants() {
        #[derive(Deserialize, JsonSchema)]
        #[serde(rename_all = "lowercase")]
        #[allow(dead_code)]
        enum Mode {
            Multipart,
            Body,
        }
        #[derive(Deserialize, JsonSchema)]
        #[serde(deny_unknown_fields)]
        #[allow(dead_code)]
        struct WithEnum {
            mode: Mode,
        }
        let s = derive::<WithEnum>();
        // Inlined enum the validator can actually check — no unresolved $ref.
        assert_eq!(
            s["properties"]["mode"]["enum"],
            json!(["multipart", "body"])
        );
        assert!(
            !s.to_string().contains("$ref"),
            "schema must be fully inlined, got: {s}"
        );
    }
}
