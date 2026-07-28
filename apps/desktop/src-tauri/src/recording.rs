//! Recorder-output sanitizing: wrap the recorder YAML fragment into a
//! complete LumoFlow doc, with schema-aware step/with sanitization.
//! Pure move out of `lib.rs`; semantics unchanged.

use super::*;

/// Wrap the recorder's `events_to_yaml_patch` fragment into a complete
/// LumoFlow doc so the user can hit ▶ on the result without hand-editing.
/// The fragment is parsed and sanitized instead of text-spliced so empty list
/// entries and schema-unknown recorder notes cannot leak into saved flows.
pub(crate) fn wrap_recording_fragment(name: &str, fragment: &str) -> Result<String, String> {
    let id = sanitize_flow_name(name);
    let id = if id.is_empty() {
        "recording".into()
    } else {
        id
    };
    let steps = sanitize_recording_fragment_steps(fragment)?;
    let doc = recording_flow_doc(&id, steps);
    let _: Flow = serde_yaml::from_value(doc.clone())
        .map_err(|e| format!("recording flow invalid after sanitize: {e}"))?;
    serde_yaml::to_string(&doc).map_err(|e| format!("yaml serialize: {e}"))
}

pub(crate) fn sanitize_recording_fragment_steps(fragment: &str) -> Result<Vec<YamlValue>, String> {
    let parsed: YamlValue =
        serde_yaml::from_str(fragment).map_err(|e| format!("yaml parse: {e}"))?;
    let seq = match parsed {
        YamlValue::Sequence(seq) => seq,
        YamlValue::Null => Vec::new(),
        _ => return Ok(Vec::new()),
    };
    let mut seen = std::collections::BTreeSet::new();
    Ok(seq
        .into_iter()
        .enumerate()
        .filter_map(|(idx, step)| sanitize_recording_step(step, idx, &mut seen))
        .collect())
}

pub(crate) fn sanitize_recording_step(
    value: YamlValue,
    idx: usize,
    seen: &mut std::collections::BTreeSet<String>,
) -> Option<YamlValue> {
    let raw = value.as_mapping()?;
    let action = yaml_string(raw, "action")?;
    let with = sanitize_recording_with(&action, raw.get(yaml_key("with")))?;
    let fallback_id = format!(
        "{}_{}",
        action.replace(|c: char| !c.is_ascii_alphanumeric(), "_"),
        idx + 1
    );
    let id = unique_recording_step_id(yaml_string(raw, "id").unwrap_or(fallback_id), seen);

    let mut clean = YamlMapping::new();
    clean.insert(yaml_key("id"), YamlValue::String(id));
    clean.insert(yaml_key("action"), YamlValue::String(action));
    if let Some(when) = yaml_string(raw, "when") {
        clean.insert(yaml_key("when"), YamlValue::String(when));
    }
    if let Some(bind) = yaml_string(raw, "bind") {
        clean.insert(yaml_key("bind"), YamlValue::String(bind));
    }
    if let Some(retry) = raw.get(yaml_key("retry")).and_then(prune_yaml_value) {
        clean.insert(yaml_key("retry"), retry);
    }
    if let Some(ai) = raw.get(yaml_key("ai")).and_then(prune_yaml_value) {
        clean.insert(yaml_key("ai"), ai);
    }
    if !with.is_empty() {
        clean.insert(yaml_key("with"), YamlValue::Mapping(with));
    }

    let clean_value = YamlValue::Mapping(clean);
    serde_yaml::from_value::<Step>(clean_value.clone()).ok()?;
    Some(clean_value)
}

pub(crate) fn sanitize_recording_with(action: &str, raw: Option<&YamlValue>) -> Option<YamlMapping> {
    let empty = YamlMapping::new();
    let with = raw.and_then(YamlValue::as_mapping).unwrap_or(&empty);
    match action {
        "browser.open" => {
            let url = yaml_string(with, "url")?;
            let mut out = YamlMapping::new();
            out.insert(yaml_key("url"), YamlValue::String(url));
            copy_yaml_bool(&mut out, with, "headless");
            copy_yaml_string(&mut out, with, "wait_for");
            copy_yaml_number(&mut out, with, "timeout_ms");
            Some(out)
        }
        "browser.click" => {
            let mut out = selector_with(with)?;
            copy_yaml_string(&mut out, with, "prompt");
            copy_yaml_string(&mut out, with, "model");
            copy_yaml_number(&mut out, with, "timeout_ms");
            Some(out)
        }
        "browser.type" => {
            let mut out = selector_with(with)?;
            let text = yaml_string_preserve(with, "text")?;
            if text.is_empty() {
                return None;
            }
            out.insert(yaml_key("text"), YamlValue::String(text));
            copy_yaml_bool(&mut out, with, "clear");
            copy_yaml_string(&mut out, with, "prompt");
            copy_yaml_string(&mut out, with, "model");
            copy_yaml_number(&mut out, with, "timeout_ms");
            Some(out)
        }
        "browser.extract" => {
            let selector = yaml_string(with, "selector")?;
            let mut out = YamlMapping::new();
            out.insert(yaml_key("selector"), YamlValue::String(selector));
            copy_yaml_bool(&mut out, with, "all");
            copy_yaml_string(&mut out, with, "attr");
            copy_yaml_number(&mut out, with, "timeout_ms");
            if let Some(map) = with.get(yaml_key("map")).and_then(prune_yaml_value) {
                out.insert(yaml_key("map"), map);
            }
            if let Some(frame) = with.get(yaml_key("frame")).and_then(prune_yaml_value) {
                out.insert(yaml_key("frame"), frame);
            }
            Some(out)
        }
        _ => raw
            .and_then(prune_yaml_value)
            .and_then(|v| match v {
                YamlValue::Mapping(map) => Some(map),
                _ => None,
            })
            .or_else(|| Some(YamlMapping::new())),
    }
}

pub(crate) fn selector_with(with: &YamlMapping) -> Option<YamlMapping> {
    let mut out = YamlMapping::new();
    copy_yaml_string(&mut out, with, "selector");
    if let Some(selectors) = with
        .get(yaml_key("selectors"))
        .and_then(YamlValue::as_mapping)
        .map(sanitize_selectors)
        .filter(|m| !m.is_empty())
    {
        out.insert(yaml_key("selectors"), YamlValue::Mapping(selectors));
    }
    if out.is_empty() {
        None
    } else {
        Some(out)
    }
}

pub(crate) fn sanitize_selectors(selectors: &YamlMapping) -> YamlMapping {
    let mut out = YamlMapping::new();
    for key in [
        "id",
        "data_testid",
        "css",
        "aria_label",
        "text_includes",
        "xpath",
    ] {
        copy_yaml_string(&mut out, selectors, key);
    }
    out
}

pub(crate) fn prune_yaml_value(value: &YamlValue) -> Option<YamlValue> {
    match value {
        YamlValue::Null => None,
        YamlValue::String(s) => {
            if s.trim().is_empty() {
                None
            } else {
                Some(YamlValue::String(s.clone()))
            }
        }
        YamlValue::Sequence(seq) => {
            let pruned: Vec<_> = seq.iter().filter_map(prune_yaml_value).collect();
            if pruned.is_empty() {
                None
            } else {
                Some(YamlValue::Sequence(pruned))
            }
        }
        YamlValue::Mapping(map) => {
            let mut pruned = YamlMapping::new();
            for (key, value) in map {
                if let Some(value) = prune_yaml_value(value) {
                    pruned.insert(key.clone(), value);
                }
            }
            if pruned.is_empty() {
                None
            } else {
                Some(YamlValue::Mapping(pruned))
            }
        }
        _ => Some(value.clone()),
    }
}

pub(crate) fn recording_flow_doc(id: &str, steps: Vec<YamlValue>) -> YamlValue {
    let mut metadata = YamlMapping::new();
    metadata.insert(yaml_key("id"), YamlValue::String(id.to_string()));
    metadata.insert(yaml_key("version"), YamlValue::String("0.1.0".into()));
    metadata.insert(yaml_key("name"), YamlValue::String(format!("录制 · {id}")));
    metadata.insert(
        yaml_key("tags"),
        YamlValue::Sequence(vec![YamlValue::String("recording".into())]),
    );

    let mut capabilities = YamlMapping::new();
    capabilities.insert(
        yaml_key("network"),
        YamlValue::Sequence(vec![YamlValue::String("*".into())]),
    );

    let mut spec = YamlMapping::new();
    spec.insert(yaml_key("capabilities"), YamlValue::Mapping(capabilities));
    spec.insert(yaml_key("steps"), YamlValue::Sequence(steps));

    let mut doc = YamlMapping::new();
    doc.insert(
        yaml_key("apiVersion"),
        YamlValue::String("lumorpa.io/v1".into()),
    );
    doc.insert(yaml_key("kind"), YamlValue::String("Flow".into()));
    doc.insert(yaml_key("metadata"), YamlValue::Mapping(metadata));
    doc.insert(yaml_key("spec"), YamlValue::Mapping(spec));
    YamlValue::Mapping(doc)
}

pub(crate) fn yaml_key(key: &str) -> YamlValue {
    YamlValue::String(key.to_string())
}

pub(crate) fn yaml_string(map: &YamlMapping, key: &str) -> Option<String> {
    yaml_string_preserve(map, key).and_then(|s| {
        let trimmed = s.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        }
    })
}

pub(crate) fn yaml_string_preserve(map: &YamlMapping, key: &str) -> Option<String> {
    map.get(yaml_key(key))
        .and_then(YamlValue::as_str)
        .map(str::to_string)
}

pub(crate) fn copy_yaml_string(out: &mut YamlMapping, src: &YamlMapping, key: &str) {
    if let Some(value) = yaml_string(src, key) {
        out.insert(yaml_key(key), YamlValue::String(value));
    }
}

pub(crate) fn copy_yaml_bool(out: &mut YamlMapping, src: &YamlMapping, key: &str) {
    if let Some(value) = src.get(yaml_key(key)).and_then(YamlValue::as_bool) {
        out.insert(yaml_key(key), YamlValue::Bool(value));
    }
}

pub(crate) fn copy_yaml_number(out: &mut YamlMapping, src: &YamlMapping, key: &str) {
    if let Some(value) = src.get(yaml_key(key)) {
        if matches!(value, YamlValue::Number(_)) {
            out.insert(yaml_key(key), value.clone());
        }
    }
}

pub(crate) fn unique_recording_step_id(raw: String, seen: &mut std::collections::BTreeSet<String>) -> String {
    let mut clean: String = raw
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '.' {
                c
            } else {
                '_'
            }
        })
        .collect();
    clean = clean.trim_matches('_').to_string();
    if clean.is_empty() {
        clean = "recorded_step".into();
    }
    let base = clean.clone();
    let mut n = 2;
    while seen.contains(&clean) {
        clean = format!("{base}_{n}");
        n += 1;
    }
    seen.insert(clean.clone());
    clean
}

#[cfg(test)]
mod recording_flow_tests {
    use super::wrap_recording_fragment;

    #[test]
    fn wrap_recording_fragment_drops_empty_and_schema_unknown_steps() {
        let fragment = r##"
# Recorder YAML patch
- id: click_1
  action: browser.click
  "# note": recorder spotted 12 similar items
  with:
    selectors:
      css: button.login
      xpath: //button[1]
- id: empty_1
  with: {}
- id: type_1
  action: browser.type
  with:
    selectors:
      id: user
    text: alice
    clear: true
"##;
        let source = wrap_recording_fragment("rec-smoke", fragment).expect("wrap recording");
        assert!(!source.contains("# note"), "{source}");
        assert!(!source.contains("empty_1"), "{source}");

        let flow = lumo_dsl::parse_str(&source).expect("recording flow parses");
        assert_eq!(flow.metadata.id, "rec-smoke");
        assert_eq!(flow.spec.steps.len(), 2);
        assert_eq!(flow.spec.steps[0].id, "click_1");
        assert_eq!(flow.spec.steps[0].action, "browser.click");
        assert_eq!(flow.spec.steps[1].id, "type_1");
        assert_eq!(flow.spec.steps[1].action, "browser.type");
    }
}
