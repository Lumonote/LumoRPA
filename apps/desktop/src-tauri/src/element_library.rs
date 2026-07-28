//! Element-library persistence and R-02 recorded-element ingestion
//! (DOM + desktop AX events -> element-library entries).
//! Pure move out of `lib.rs`; semantics unchanged.

use super::*;

pub(crate) fn element_library_path(app: &AppHandle) -> Result<PathBuf, String> {
    let home = app_home(app)?;
    std::fs::create_dir_all(&home).map_err(|e| format!("create {}: {e}", home.display()))?;
    Ok(home.join("element-library.json"))
}

pub(crate) fn empty_element_library() -> Value {
    serde_json::json!({
        "version": 1,
        "elements": [],
        "images": [],
        "datatables": [],
    })
}

pub(crate) fn normalize_element_library(value: &mut Value) {
    if !value.is_object() {
        *value = empty_element_library();
        return;
    }
    let obj = value.as_object_mut().expect("checked object");
    obj.entry("version").or_insert(Value::from(1));
    for key in ["elements", "images", "datatables"] {
        if !matches!(obj.get(key), Some(Value::Array(_))) {
            obj.insert(key.into(), Value::Array(Vec::new()));
        }
    }
}

pub(crate) fn load_element_library_value(app: &AppHandle) -> Result<Value, String> {
    let path = element_library_path(app)?;
    if !path.exists() {
        return Ok(empty_element_library());
    }
    let text =
        std::fs::read_to_string(&path).map_err(|e| format!("read {}: {e}", path.display()))?;
    let mut library: Value =
        serde_json::from_str(&text).map_err(|e| format!("parse {}: {e}", path.display()))?;
    normalize_element_library(&mut library);
    Ok(library)
}

pub(crate) fn save_element_library_value(app: &AppHandle, library: &Value) -> Result<(), String> {
    let path = element_library_path(app)?;
    let text = serde_json::to_string_pretty(library).map_err(|e| e.to_string())?;
    std::fs::write(&path, text).map_err(|e| format!("write {}: {e}", path.display()))
}

pub(crate) fn upsert_recorded_elements(app: &AppHandle, events: &[RawEvent]) -> Result<usize, String> {
    let mut library = load_element_library_value(app)?;
    normalize_element_library(&mut library);
    let Some(elements) = library.get_mut("elements").and_then(Value::as_array_mut) else {
        return Ok(0);
    };
    let mut changed = 0usize;
    for event in events {
        if let Some(element) = recorded_element_from_event(event) {
            upsert_element(elements, element);
            changed += 1;
        }
    }
    if changed > 0 {
        if let Some(obj) = library.as_object_mut() {
            obj.insert(
                "updatedAt".into(),
                chrono::Utc::now()
                    .to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
                    .into(),
            );
        }
        save_element_library_value(app, &library)?;
    }
    Ok(changed)
}

pub(crate) fn upsert_element(elements: &mut Vec<Value>, incoming: Value) {
    let Some(id) = incoming
        .get("id")
        .and_then(Value::as_str)
        .map(str::to_string)
    else {
        return;
    };
    if let Some(existing) = elements
        .iter_mut()
        .find(|el| el.get("id").and_then(Value::as_str) == Some(id.as_str()))
    {
        merge_recorded_element(existing, incoming);
    } else {
        elements.push(incoming);
    }
}

pub(crate) fn merge_recorded_element(existing: &mut Value, incoming: Value) {
    let preserved_label = existing.get("label").cloned();
    let preserved_group = existing.get("group").cloned();
    let preserved_used_in = existing.get("usedIn").cloned();
    *existing = incoming;
    if let Some(label) =
        preserved_label.filter(|v| v.as_str().map(|s| !s.trim().is_empty()).unwrap_or(false))
    {
        existing["label"] = label;
    }
    if let Some(group) =
        preserved_group.filter(|v| v.as_str().map(|s| !s.trim().is_empty()).unwrap_or(false))
    {
        existing["group"] = group;
    }
    if let Some(old_used) = preserved_used_in.and_then(|v| v.as_array().cloned()) {
        let mut merged = existing
            .get("usedIn")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        for item in old_used {
            if !merged.iter().any(|v| v == &item) {
                merged.push(item);
            }
        }
        existing["usedIn"] = Value::Array(merged);
    }
}

pub(crate) fn recorded_element_from_event(event: &RawEvent) -> Option<Value> {
    match event.source.as_str() {
        "dom" => recorded_dom_element(event),
        "desktop" => recorded_desktop_element(event),
        _ => None,
    }
}

/// R-02 desktop element ingestion. A `focus_field` / `focus_changed` event
/// carries a [`FocusSnapshot`] (app / window_title / focused_role / name /
/// value). When the focused control is identifiable we mint a desktop
/// element-library entry whose fingerprints describe the AX target so the
/// user can reuse it in `desktop.*` steps. We deliberately skip
/// `app_changed` / `launched` / `heartbeat` (no actionable control there).
pub(crate) fn recorded_desktop_element(event: &RawEvent) -> Option<Value> {
    if !matches!(event.kind.as_str(), "focus_field" | "focus_changed") {
        return None;
    }
    let payload = &event.payload;
    let str_field = |key: &str| {
        payload
            .get(key)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
    };
    let role = str_field("focused_role");
    let name = str_field("focused_name");
    let app = str_field("app");
    let window = str_field("window_title");
    // Need at least a named/typed control to be worth saving.
    if role.is_none() && name.is_none() {
        return None;
    }
    let source_label = window.or(app).unwrap_or("桌面应用");
    // Reuse the same id hashing as DOM: (source | css(=role) | xpath(unused) | label(=name)).
    let id = recorded_element_id(source_label, role, None, name);
    let captured_at = format_event_time(event.at_ms);
    let mut fingerprints = Map::new();
    if let Some(v) = role {
        fingerprints.insert("ax_role".into(), v.into());
    }
    if let Some(v) = name {
        fingerprints.insert("ax_name".into(), v.into());
        fingerprints.insert("text_includes".into(), v.into());
    }
    if let Some(v) = app {
        fingerprints.insert("app".into(), v.into());
    }
    if let Some(v) = window {
        fingerprints.insert("window_title".into(), v.into());
    }
    let display_label = name
        .map(str::to_string)
        .or_else(|| role.map(|r| format!("桌面控件 · {r}")))
        .unwrap_or_else(|| "桌面控件".into());
    let element = serde_json::json!({
        "id": id,
        "label": display_label,
        "group": format!("录制 · {source_label}"),
        "automation": "desktop",
        "scope": "local",
        "syncState": "local",
        "owner": "Recorder",
        "source": source_label,
        "tag": role.unwrap_or("control"),
        "role": role.unwrap_or("element"),
        "capturedAt": captured_at,
        "lastValidated": captured_at,
        "usedIn": ["desktop.focus"],
        "fingerprints": Value::Object(fingerprints),
    });
    Some(element)
}

pub(crate) fn recorded_dom_element(event: &RawEvent) -> Option<Value> {
    if event.source != "dom" {
        return None;
    }
    if !matches!(
        event.kind.as_str(),
        "click" | "input" | "change" | "keydown" | "similar_grab"
    ) {
        return None;
    }
    let payload = &event.payload;
    let css = if event.kind == "similar_grab" {
        payload.get("generalized_selector").and_then(Value::as_str)
    } else {
        payload.get("selector").and_then(Value::as_str)
    }
    .map(str::trim)
    .filter(|s| !s.is_empty());
    let xpath = payload
        .get("xpath")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let label = payload
        .get("label")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty());
    if css.is_none() && xpath.is_none() && label.is_none() {
        return None;
    }
    let url = payload
        .get("url")
        .and_then(Value::as_str)
        .unwrap_or("录制页面");
    let tag = payload
        .get("tag")
        .and_then(Value::as_str)
        .unwrap_or("element");
    let id = recorded_element_id(url, css, xpath, label);
    let captured_at = format_event_time(event.at_ms);
    let mut fingerprints = Map::new();
    if let Some(v) = css {
        fingerprints.insert("css".into(), v.into());
    }
    if let Some(v) = xpath {
        fingerprints.insert("xpath".into(), v.into());
    }
    if let Some(v) = label {
        fingerprints.insert("aria_label".into(), v.into());
        if v.len() < 32 {
            fingerprints.insert("text_includes".into(), v.into());
        }
    }
    let display_label = label
        .map(str::to_string)
        .unwrap_or_else(|| format!("录制元素 · {tag}"));
    let used_in = match event.kind.as_str() {
        "input" | "change" => "browser.type",
        "similar_grab" => "browser.extract",
        _ => "browser.click",
    };
    let sibling_count = payload
        .get("sibling_count")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let mut element = serde_json::json!({
        "id": id,
        "label": display_label,
        "group": format!("录制 · {}", source_group(url)),
        "automation": "web",
        "scope": "local",
        "syncState": "local",
        "owner": "Recorder",
        "source": url,
        "tag": tag,
        "role": role_for_tag(tag),
        "capturedAt": captured_at,
        "lastValidated": captured_at,
        "usedIn": [used_in],
        "fingerprints": Value::Object(fingerprints),
    });
    if sibling_count > 0 {
        element["siblingCount"] = Value::from(sibling_count);
        element["similar"] = Value::Array(vec![Value::from("同款元素")]);
    }
    Some(element)
}

pub(crate) fn recorded_element_id(
    source: &str,
    css: Option<&str>,
    xpath: Option<&str>,
    label: Option<&str>,
) -> String {
    let key = format!(
        "{}|{}|{}|{}",
        source,
        css.unwrap_or_default(),
        xpath.unwrap_or_default(),
        label.unwrap_or_default()
    );
    let mut hasher = DefaultHasher::new();
    key.hash(&mut hasher);
    format!("el_rec_{:016x}", hasher.finish())
}

pub(crate) fn format_event_time(ms: i64) -> String {
    chrono::DateTime::<chrono::Utc>::from_timestamp_millis(ms)
        .unwrap_or_else(chrono::Utc::now)
        .format("%Y-%m-%d %H:%M")
        .to_string()
}

pub(crate) fn source_group(source: &str) -> String {
    let without_scheme = source.split("://").nth(1).unwrap_or(source);
    without_scheme
        .split('/')
        .next()
        .filter(|s| !s.is_empty())
        .unwrap_or("页面")
        .to_string()
}

pub(crate) fn role_for_tag(tag: &str) -> &'static str {
    match tag {
        "input" | "textarea" => "textbox",
        "button" => "button",
        "select" => "combobox",
        "a" => "link",
        "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => "heading",
        _ => "element",
    }
}
