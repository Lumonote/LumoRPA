//! F-23 gate: every action whose hand-written `schema()` was replaced by
//! `crate::schema::derive::<XIn>()` must stay (a) inside the subset the two
//! validators understand and (b) at least as strict as before — no unknown
//! fields, required stays required, declared scalar types stay enforced.
//!
//! schemars makes the input struct the single source of truth; these tests make
//! a *silent loosening* impossible to ship — e.g. an `Option<T>` field decaying
//! into an array `type` the core validator can't read, a `String` enum losing
//! its variants behind an unresolved `$ref`, or a struct missing
//! `deny_unknown_fields` so `additionalProperties:false` is dropped.
//!
//! The behavioural test is schema-driven (it synthesises probes from each
//! action's own schema), so it covers every id in `DERIVED_CLOSED` with no
//! per-action boilerplate. `schema_examples.rs` is the complementary gate: real
//! shipped flows must still validate (catches over-tightening).

use lumo_actions::register_all;
use lumo_core::{schema::validate_input, ActionRegistry};
use serde_json::{json, Map, Value};

/// Action ids whose `schema()` now derives from a `#[serde(deny_unknown_fields)]`
/// input struct. Extended as each family is converted.
const DERIVED_CLOSED: &[&str] = &[
    // hash_ops
    "hash.sha256", "hash.sha512", "hash.sha1", "hash.md5",
    "util.base64_encode", "util.base64_decode", "util.uuid",
    // math_ops
    "math.round", "math.random", "math.min", "math.max", "math.sum", "math.avg", "math.abs",
    // regex_ops
    "regex.match", "regex.find_all", "regex.replace", "regex.captures",
    // archive
    "archive.zip", "archive.unzip",
    // clipboard
    "clipboard.get", "clipboard.set",
    // data
    "data.json_parse", "data.json_format",
    // db_ops
    "db.sqlite_query", "db.sqlite_exec",
    // csv_ops
    "csv.parse", "csv.stringify", "csv.read", "csv.write",
    // excel
    "excel.read_rows", "excel.write_row",
    // file
    "file.read", "file.write", "file.exists",
    // system_ops
    "system.shell", "system.env_get", "system.sleep", "system.platform",
    // date_ops (date.diff's `unit` is a derived Rust enum → keeps its enum constraint)
    "date.now", "date.parse", "date.format", "date.add", "date.diff", "date.weekday",
    // mcp — discover only; mcp.call stays hand-written (`arguments: Value` can't
    // express the hand-written `{type:object}` without loosening it).
    "mcp.discover",
    // control — VM-dispatched, but schemas still serve `lumo validate`/`actions`.
    // Held hand-written: control.log (level enum), control.for_each (struct looser
    // than its schema), control.try / control.parallel (intentionally permissive).
    "control.set_var", "control.if", "control.for", "control.fail", "control.sleep",
    // json_ops — json.merge kept hand-written (`a`/`b: Value` can't express {type:object}).
    "json.get", "json.set", "json.keys", "json.values", "json.delete",
    // http — upload's `mode` is a derived Rust enum (keeps ["multipart","body"]).
    "http.request", "http.download", "http.upload",
    // table_ops — data.join's `type` is a derived Rust enum (keeps ["inner","left"]).
    // data.filter / data.group_by stay hand-written: their op enums live on `String`
    // fields nested inside arrays / maps that the validator recurses into.
    "data.join",
    // list_ops
    "list.length", "list.append", "list.sort", "list.unique", "list.range",
    "list.contains", "list.get", "list.slice", "list.reverse", "list.pluck",
    // string_ops
    "string.upper", "string.lower", "string.trim", "string.length", "string.split",
    "string.join", "string.replace", "string.contains", "string.starts_with",
    "string.ends_with", "string.substring", "string.repeat", "string.pad_left",
    "string.pad_right", "string.format",
    // browser — wait's `condition` is a derived Rust enum (keeps the 4 conditions);
    // click/type/wait share `MultiSelector`, which now derives JsonSchema +
    // deny_unknown_fields so the nested `selectors:` object stays closed too.
    "browser.launch", "browser.close", "browser.open", "browser.click",
    "browser.type", "browser.extract", "browser.wait",
    // browser F-10 completions (scroll's `to` is a derived enum; screenshot gates fs-write).
    "browser.eval", "browser.screenshot", "browser.scroll", "browser.hover",
    "browser.select", "browser.cookies", "browser.set_cookie",
];

fn registry() -> ActionRegistry {
    let mut reg = ActionRegistry::new();
    register_all(&mut reg);
    reg
}

fn schema_of<'a>(reg: &'a ActionRegistry, id: &str) -> &'a Value {
    reg.get(id)
        .unwrap_or_else(|| panic!("`{id}` is in DERIVED_CLOSED but not registered"))
        .schema()
}

// ─── structural invariant ───────────────────────────────────────────────────

/// Recursively confirm a derived schema stays inside the validators' subset:
/// no `$ref` (inlining must have resolved every one) and no array-valued `type`
/// (which the core validator silently ignores → the field is not type-checked).
///
/// Schema-aware on purpose: the keyword checks apply to the *current* schema
/// node, then we recurse only into schema-bearing positions. `properties` (and
/// friends) are name→schema maps, so we descend into their VALUES — otherwise a
/// field literally named `type` (e.g. `data.join`'s `#[serde(rename="type")]`
/// enum) would be misread as the `type` keyword, exactly as the real validator
/// avoids by walking `properties` as a map (`validate_object`).
fn assert_validator_safe(id: &str, v: &Value) {
    let map = match v {
        Value::Object(m) => m,
        Value::Array(arr) => {
            arr.iter().for_each(|s| assert_validator_safe(id, s));
            return;
        }
        _ => return,
    };
    // ── keyword checks on THIS node ──
    assert!(
        !map.contains_key("$ref"),
        "`{id}`: derived schema contains an unresolved $ref — inline_subschemas \
         must inline every enum/nested type, else the validator can't check it"
    );
    if let Some(t) = map.get("type") {
        assert!(
            t.is_string(),
            "`{id}`: `type` is {t} (an array) — the core validator reads only a \
             single-string `type`, so this field would NOT be type-checked. An \
             `Option<T>` must derive to `\"type\":\"T\"` (option_add_null_type=false)."
        );
    }
    // ── recurse into schema-bearing positions only ──
    for (k, sub) in map {
        match k.as_str() {
            // name→schema maps: each VALUE is a schema (its KEY is a field name,
            // never a keyword — so a property called `type`/`enum` is safe here).
            "properties" | "patternProperties" | "definitions" | "$defs" => {
                if let Some(obj) = sub.as_object() {
                    obj.values().for_each(|s| assert_validator_safe(id, s));
                }
            }
            // data, not schemas — never descend (avoids false keyword hits).
            "enum" | "required" | "type" | "description" | "title" | "format"
            | "default" | "const" | "examples" => {}
            // anything else may itself be a schema (`items`, `additionalProperties`)
            // or an array of schemas (`allOf`/`anyOf`/`oneOf`).
            _ => assert_validator_safe(id, sub),
        }
    }
}

#[test]
fn derived_schemas_are_validator_safe() {
    let reg = registry();
    for id in DERIVED_CLOSED {
        let schema = schema_of(&reg, id);
        assert_eq!(schema["type"], json!("object"), "`{id}`: top-level schema must be an object");
        assert_eq!(
            schema["additionalProperties"],
            json!(false),
            "`{id}`: lost `additionalProperties:false` — its input struct needs \
             #[serde(deny_unknown_fields)]"
        );
        assert_validator_safe(id, schema);
    }
}

// ─── behavioural invariant (schema-driven) ───────────────────────────────────

/// A value that SHOULD satisfy `schema` — every required field set to a
/// type-appropriate dummy (recursing into nested objects, picking the first
/// `enum` member). Lets us prove that *breaking* one field is what flips
/// validation, i.e. the schema genuinely enforces it.
fn dummy_for(schema: &Value) -> Value {
    if let Some(Value::Array(allowed)) = schema.get("enum") {
        if let Some(first) = allowed.first() {
            return first.clone();
        }
    }
    match schema.get("type").and_then(Value::as_str) {
        Some("string") => json!("x"),
        Some("integer") | Some("number") => json!(1),
        Some("boolean") => json!(true),
        Some("array") => json!([]),
        Some("object") => Value::Object(valid_object(schema)),
        _ => json!("x"), // no/unknown type ⇒ permissive ⇒ any value satisfies it
    }
}

/// The required-fields-only object for an object schema.
fn valid_object(schema: &Value) -> Map<String, Value> {
    let mut m = Map::new();
    let props = schema.get("properties").and_then(Value::as_object);
    if let Some(req) = schema.get("required").and_then(Value::as_array) {
        for key in req.iter().filter_map(Value::as_str) {
            let sub = props.and_then(|p| p.get(key)).cloned().unwrap_or_else(|| json!({}));
            m.insert(key.to_string(), dummy_for(&sub));
        }
    }
    m
}

#[test]
fn derived_schemas_enforce_required_unknown_and_types() {
    let reg = registry();
    let mut errs = Vec::new();

    for id in DERIVED_CLOSED {
        let schema = schema_of(&reg, id);
        let base = valid_object(schema);

        // (1) the synthesised minimal object must validate — otherwise we can't
        // trust the negative probes below.
        if let Err(e) = validate_input(schema, &Value::Object(base.clone())) {
            errs.push(format!("`{id}`: synthesised minimal input {:?} was REJECTED: {e}", base));
            continue;
        }

        // (2) an unknown field must be rejected (additionalProperties:false).
        let mut unknown = base.clone();
        unknown.insert("__lumo_unknown_field__".into(), json!(1));
        if validate_input(schema, &Value::Object(unknown)).is_ok() {
            errs.push(format!("`{id}`: an unknown field was ACCEPTED (additionalProperties:false lost)"));
        }

        // (3) removing any required field must be rejected.
        if let Some(req) = schema.get("required").and_then(Value::as_array) {
            for key in req.iter().filter_map(Value::as_str) {
                let mut missing = base.clone();
                missing.remove(key);
                if validate_input(schema, &Value::Object(missing)).is_ok() {
                    errs.push(format!("`{id}`: removing required `{key}` still validated"));
                }
            }
        }

        // (4) every scalar-typed property must reject a wrong-typed literal.
        if let Some(props) = schema.get("properties").and_then(Value::as_object) {
            for (key, sub) in props {
                if sub.get("enum").is_some() {
                    continue; // covered by its declared members, not a free type
                }
                let wrong = match sub.get("type").and_then(Value::as_str) {
                    Some("string") => json!(123),
                    Some("integer") | Some("number") => json!("not-a-number"),
                    Some("boolean") => json!("not-a-bool"),
                    _ => continue, // array/object/untyped: skip (not a clear-cut scalar)
                };
                let mut bad = base.clone();
                bad.insert(key.clone(), wrong.clone());
                if validate_input(schema, &Value::Object(bad)).is_ok() {
                    errs.push(format!(
                        "`{id}`: property `{key}` accepted wrong-typed value {wrong} \
                         (declared {})",
                        sub["type"]
                    ));
                }
            }
        }
    }

    assert!(errs.is_empty(), "derived schemas under-enforce:\n{}", errs.join("\n"));
}
