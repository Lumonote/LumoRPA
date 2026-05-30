use clap::Args as ClapArgs;
use lumo_core::ActionRegistry;
use lumo_dsl::Step;
use std::path::PathBuf;
use std::sync::Arc;

use super::{build_action_registry, load_skill_registry};

#[derive(Debug, ClapArgs)]
pub struct Args {
    /// Flow YAML file
    pub flow: PathBuf,
}

pub async fn run(home: PathBuf, args: Args) -> anyhow::Result<()> {
    let flow = lumo_dsl::parse_file(&args.flow)?;
    lumo_dsl::validate(&flow)?;
    let registry = build_action_registry(&home, Some(&args.flow));
    let skills = load_skill_registry(&home, Some(&args.flow));
    validate_steps(
        &flow.spec.steps,
        &flow.spec.capabilities,
        &registry,
        &skills,
    )?;
    println!(
        "OK  flow id={} version={} steps={}",
        flow.metadata.id,
        flow.metadata.version,
        flow.spec.steps.len()
    );
    Ok(())
}

fn validate_steps(
    steps: &[Step],
    capabilities: &lumo_dsl::Capabilities,
    registry: &ActionRegistry,
    skills: &Arc<lumo_skills::SkillRegistry>,
) -> anyhow::Result<()> {
    for step in steps {
        let action = registry.get(&step.action).ok_or_else(|| {
            anyhow::anyhow!("unknown action `{}` in step `{}`", step.action, step.id)
        })?;
        validate_capability_declaration(step, capabilities)?;
        let input = serde_json::to_value(&step.with).unwrap_or(serde_json::Value::Null);
        validate_schema(&step.id, &step.action, &input, action.schema())?;
        validate_skill_reference(step, &input, skills)?;
        for children in step.children() {
            validate_steps(children, capabilities, registry, skills)?;
        }
    }
    Ok(())
}

fn validate_capability_declaration(
    step: &Step,
    capabilities: &lumo_dsl::Capabilities,
) -> anyhow::Result<()> {
    let missing = match step.action.as_str() {
        "file.read" | "file.exists" | "excel.read_rows" if capabilities.fs_read.is_empty() => {
            Some("fs.read")
        }
        "file.write" | "excel.write_row" if capabilities.fs_write.is_empty() => Some("fs.write"),
        "http.request" | "browser.open" if capabilities.network.is_empty() => Some("network"),
        "ai.chat" if capabilities.llm.is_empty() => Some("llm"),
        _ => None,
    };
    if let Some(kind) = missing {
        anyhow::bail!(
            "step `{}` action `{}` requires spec.capabilities.{kind}",
            step.id,
            step.action
        );
    }
    Ok(())
}

fn validate_skill_reference(
    step: &Step,
    input: &serde_json::Value,
    skills: &Arc<lumo_skills::SkillRegistry>,
) -> anyhow::Result<()> {
    if step.action != "skill.invoke" {
        return Ok(());
    }
    let Some(name) = input.get("name").and_then(serde_json::Value::as_str) else {
        return Ok(());
    };
    if is_template_string(name) {
        return Ok(());
    }
    if skills.get(name).is_none() {
        anyhow::bail!("step `{}` invokes unknown skill `{name}`", step.id);
    }
    Ok(())
}

fn validate_schema(
    step_id: &str,
    action_id: &str,
    input: &serde_json::Value,
    schema: &serde_json::Value,
) -> anyhow::Result<()> {
    if schema.get("type").and_then(serde_json::Value::as_str) == Some("object") {
        // An absent `with:` (e.g. control-flow steps like `control.parallel`
        // that carry their config in `branches:`) deserializes to Null. Treat
        // it as an empty object so it validates cleanly when the schema has no
        // required properties; a genuinely non-object `with` still errors.
        let empty = serde_json::Map::new();
        let input_obj = match input {
            serde_json::Value::Null => &empty,
            serde_json::Value::Object(map) => map,
            _ => anyhow::bail!("step `{step_id}` action `{action_id}` with: must be an object"),
        };
        if let Some(required) = schema.get("required").and_then(serde_json::Value::as_array) {
            for key in required.iter().filter_map(serde_json::Value::as_str) {
                if !input_obj.contains_key(key) {
                    anyhow::bail!(
                        "step `{step_id}` action `{action_id}` missing required with.{key}"
                    );
                }
            }
        }
        let properties = schema
            .get("properties")
            .and_then(serde_json::Value::as_object);
        if schema
            .get("additionalProperties")
            .and_then(serde_json::Value::as_bool)
            == Some(false)
        {
            for key in input_obj.keys() {
                if !properties
                    .map(|props| props.contains_key(key))
                    .unwrap_or(false)
                {
                    anyhow::bail!("step `{step_id}` action `{action_id}` has unknown with.{key}");
                }
            }
        }
        if let Some(properties) = properties {
            for (key, value) in input_obj {
                if let Some(prop_schema) = properties.get(key) {
                    validate_value_type(step_id, action_id, key, value, prop_schema)?;
                }
            }
        }
    }
    Ok(())
}

fn validate_value_type(
    step_id: &str,
    action_id: &str,
    key: &str,
    value: &serde_json::Value,
    schema: &serde_json::Value,
) -> anyhow::Result<()> {
    if value.as_str().map(is_template_string).unwrap_or(false) {
        return Ok(());
    }
    let Some(expected) = schema.get("type") else {
        return Ok(());
    };
    let ok = match expected {
        serde_json::Value::String(s) => json_type_matches(s, value),
        serde_json::Value::Array(types) => types
            .iter()
            .filter_map(serde_json::Value::as_str)
            .any(|s| json_type_matches(s, value)),
        _ => true,
    };
    if !ok {
        anyhow::bail!(
            "step `{step_id}` action `{action_id}` with.{key} expected {}, got {}",
            expected,
            json_kind(value)
        );
    }
    Ok(())
}

fn json_type_matches(expected: &str, value: &serde_json::Value) -> bool {
    match expected {
        "string" => value.is_string(),
        "number" => value.is_number(),
        "integer" => value.as_i64().is_some() || value.as_u64().is_some(),
        "boolean" => value.is_boolean(),
        "array" => value.is_array(),
        "object" => value.is_object(),
        "null" => value.is_null(),
        _ => true,
    }
}

fn json_kind(value: &serde_json::Value) -> &'static str {
    match value {
        serde_json::Value::Null => "null",
        serde_json::Value::Bool(_) => "boolean",
        serde_json::Value::Number(_) => "number",
        serde_json::Value::String(_) => "string",
        serde_json::Value::Array(_) => "array",
        serde_json::Value::Object(_) => "object",
    }
}

fn is_template_string(s: &str) -> bool {
    s.contains("{{") || s.contains("{%")
}

#[cfg(test)]
mod tests {
    use super::validate_schema;
    use serde_json::json;

    fn object_schema() -> serde_json::Value {
        json!({ "type": "object", "properties": {}, "additionalProperties": true })
    }

    #[test]
    fn absent_with_is_valid_against_object_schema() {
        // control.parallel & friends carry config in `branches:`, so `with`
        // deserializes to Null — that must validate cleanly.
        assert!(validate_schema("s", "control.parallel", &json!(null), &object_schema()).is_ok());
    }

    #[test]
    fn empty_object_with_is_valid() {
        assert!(validate_schema("s", "control.close", &json!({}), &object_schema()).is_ok());
    }

    #[test]
    fn non_object_with_still_errors() {
        // A scalar `with:` is still a real authoring mistake and must be caught.
        let err = validate_schema("s", "browser.open", &json!("oops"), &object_schema());
        assert!(err.is_err(), "scalar with should be rejected");
        let err2 = validate_schema("s", "browser.open", &json!(["a"]), &object_schema());
        assert!(err2.is_err(), "array with should be rejected");
    }

    #[test]
    fn missing_required_key_errors_even_when_absent() {
        let schema = json!({ "type": "object", "properties": { "url": {} }, "required": ["url"] });
        assert!(validate_schema("s", "browser.open", &json!(null), &schema).is_err());
    }
}
