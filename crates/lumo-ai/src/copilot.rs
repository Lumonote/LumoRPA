//! NL → Flow generation — the "Magic Prompt" / copilot core (A-13 / F-18).
//!
//! Shared by `lumo copilot` (CLI) and the desktop Magic Prompt panel: asks the
//! configured LLM to draft a lumo/v1 YAML flow, unwraps the fenced block,
//! validates it against the DSL, and retries (feeding the failure reason back
//! into the prompt) up to `max_attempts`. The LLM call is the only
//! non-deterministic part; the extract / validate / retry scaffolding is pure
//! and unit-tested here.

use crate::{AiRouter, ChatMessage, ChatRequest, Role};

/// Run the NL→Flow loop: chat → unwrap YAML → DSL-validate, retrying with the
/// prior validation error appended until a flow validates or attempts run out.
/// Returns the validated YAML, or a human-readable error string.
///
/// Pure scaffolding + one LLM round-trip per attempt; callers own provider setup
/// (router) and any file I/O. `max_attempts` is clamped to at least 1.
pub async fn generate_flow(
    router: &AiRouter,
    model: &str,
    prompt: &str,
    max_attempts: u32,
) -> Result<String, String> {
    generate_flow_with_validator(router, model, prompt, max_attempts, validate_yaml).await
}

/// Same generation loop as [`generate_flow`], but lets callers supply a stricter
/// synchronous validator. The desktop app uses this to include registry/schema
/// and capability checks, not only DSL structure.
pub async fn generate_flow_with_validator<F>(
    router: &AiRouter,
    model: &str,
    prompt: &str,
    max_attempts: u32,
    validate_candidate: F,
) -> Result<String, String>
where
    F: Fn(&str) -> Result<(), String>,
{
    let attempts = max_attempts.max(1);
    let mut last_err: Option<String> = None;
    for attempt in 0..attempts {
        let user = build_user_message(prompt, last_err.as_deref());
        let resp = router
            .chat(ChatRequest {
                model: model.to_string(),
                messages: vec![ChatMessage::text(Role::User, user)],
                temperature: Some(0.2),
                max_tokens: Some(2048),
                system: Some(system_prompt()),
            })
            .await
            .map_err(|e| format!("chat: {e}"))?;
        let candidate = extract_yaml(&resp.content);
        match validate_candidate(&candidate) {
            Ok(()) => return Ok(candidate),
            Err(e) => {
                tracing::debug!("copilot attempt {} validate failed: {e}", attempt + 1);
                last_err = Some(e);
            }
        }
    }
    Err(format!(
        "copilot could not produce a valid flow after {attempts} attempts: {}",
        last_err.unwrap_or_else(|| "<unknown>".into())
    ))
}

fn system_prompt() -> String {
    r#"You are a LumoRPA flow generator. Produce ONLY one YAML document
matching the lumorpa.io/v1 Flow schema. Wrap output in ```yaml ... ``` fences.

Flow skeleton:
- top-level keys: apiVersion: lumorpa.io/v1, kind: Flow, metadata, spec.
- metadata requires id. Use snake_case ids.
- spec.steps is a list of steps. Each step requires id, action, and usually with.
- supported step fields: id, action, with, bind, when, retry, ai, do, else, catch, finally, branches.
- templates use {{ inputs.x }}, {{ vars.x }}, {{ steps.step_id.result }}, and loop bindings like {{ row.field }}.
- if a later step reuses an output, prefer bind: name and then {{ vars.name }}.

Common valid actions and input shapes:
- control.log: with { message, level? }
- control.set_var: with { name, value }
- control.if: with { cond }, nested do/else.
- control.for: with { from, to, step?, bind? }, nested do.
- control.for_each: with { in, bind? }, nested do. Use {{ row.field }} or the custom bind.
- control.try: with {}, nested do/catch/finally.
- file.read: with { path }; file.write: with { path, content, append? }; file.exists/list/mkdir/copy/move/rename/delete/metadata are also valid file.* actions.
- http.request: with { method?, url, headers?, body?, timeout_ms? }.
- browser.open: with { url, headless?, timeout_ms?, wait_for? }.
- browser.wait: with { selector? or selectors?, condition?, text?, timeout_ms? }.
- browser.click: with { selector? or selectors?, prompt?, timeout_ms? }.
- browser.type: with { selector? or selectors?, text, clear?, prompt?, timeout_ms? }.
- browser.extract: with { selector, all?, attr?, map?, timeout_ms? }.
- browser.info: with { fields?: [url|title|html|text], timeout_ms? } reads current page info.
- excel.read_rows: with { file, sheet?, header?, limit? }.
- excel.write_row: with { file, sheet?, row, headers?, replace_sheet? }.
- excel.sheet_names/read_cell/write_cell support sheet discovery and A1-style cell access.
- csv.parse/stringify/read/write, data.json_parse/json_format/filter/group_by/join,
  list.*, string.*, regex.*, date.*, math.*, json.*, hash.*, pdf.*, image.* are also valid.
- ai.chat uses action ai.chat with { prompt, model?, system?, temperature?, max_tokens? }.
- image.ocr uses { image, prompt?, model?, media_type? } for local image OCR via the configured OCR/vision model.
- skill.invoke uses { name, inputs? }; flow.call uses { path, inputs? }.

Capabilities are required for side effects:
- browser/http/email/notify/ftp/s3/mcp network calls need spec.capabilities.network grants.
- file/csv/excel/pdf/archive/image local reads need fs.read grants.
- file/csv/excel/pdf/archive/browser.screenshot/download writes need fs.write grants.
- ai.chat, image.ocr, or step ai modes need llm grants.

Rules:
- Do NOT invent action ids. Never use excel.read, excel.write, chat, data.set, or data.get.
- Keep every step id unique, including nested blocks.
- For browser scraping, open the page, wait for a selector or text, extract all matching rows,
  then use control.for_each plus excel.write_row to write rows.
- Declare the capabilities the generated steps need.
- Output exactly one fenced YAML block. No prose."#
        .to_string()
}

fn build_user_message(prompt: &str, retry_err: Option<&str>) -> String {
    match retry_err {
        Some(e) => format!(
            "Generate a LumoRPA flow for this request:\n\n{prompt}\n\n\
             Previous attempt failed validation: {e}\nPlease fix and try again."
        ),
        None => format!("Generate a LumoRPA flow for this request:\n\n{prompt}"),
    }
}

/// Unwrap a ```yaml fenced block (then a bare ``` fence, then the raw body) from
/// an LLM reply.
pub fn extract_yaml(response: &str) -> String {
    // Prefer ```yaml ... ``` fences; fall back to ``` ... ``` then to raw body.
    if let Some(after) = response.find("```yaml") {
        let rest = &response[after + 7..];
        if let Some(end) = rest.find("```") {
            return rest[..end].trim().to_string();
        }
    }
    if let Some(after) = response.find("```") {
        let rest = &response[after + 3..];
        // Skip a possible language tag on the same line.
        let rest = rest.split_once('\n').map(|(_, body)| body).unwrap_or(rest);
        if let Some(end) = rest.find("```") {
            return rest[..end].trim().to_string();
        }
    }
    response.trim().to_string()
}

/// Parse + DSL-validate a candidate flow YAML (the gate the retry loop checks).
pub fn validate_yaml(yaml: &str) -> Result<(), String> {
    if yaml.is_empty() {
        return Err("empty YAML".into());
    }
    let flow = lumo_dsl::parse_str(yaml).map_err(|e| format!("parse: {e}"))?;
    lumo_dsl::validate(&flow).map_err(|e| format!("validate: {e}"))?;
    Ok(())
}

/// Derive a filesystem-safe slug from the flow's `metadata.id`.
pub fn slug_from_yaml(yaml: &str) -> Option<String> {
    let flow = lumo_dsl::parse_str(yaml).ok()?;
    let id = flow.metadata.id.as_str();
    let slug: String = id
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect();
    if slug.is_empty() {
        None
    } else {
        Some(slug)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_yaml_unwraps_yaml_fence() {
        let resp = "Here you go:\n```yaml\napiVersion: lumorpa.io/v1\nkind: Flow\n```\n";
        let out = extract_yaml(resp);
        assert!(out.starts_with("apiVersion"));
        assert!(!out.contains("```"));
    }

    #[test]
    fn extract_yaml_unwraps_bare_fence() {
        let resp = "```\napiVersion: lumorpa.io/v1\nkind: Flow\n```";
        let out = extract_yaml(resp);
        assert!(out.starts_with("apiVersion"));
    }

    #[test]
    fn extract_yaml_returns_raw_when_no_fence() {
        let resp = "apiVersion: lumorpa.io/v1\nkind: Flow\nmetadata:\n  id: x";
        let out = extract_yaml(resp);
        assert_eq!(out, resp);
    }

    #[test]
    fn validate_yaml_rejects_garbage() {
        let res = validate_yaml(": :: not yaml ::");
        assert!(res.is_err());
    }

    #[test]
    fn validate_yaml_rejects_empty() {
        assert!(validate_yaml("").is_err());
    }

    #[test]
    fn validate_yaml_accepts_minimal_flow() {
        let yaml = r#"
apiVersion: lumorpa.io/v1
kind: Flow
metadata: { id: greet }
spec:
  steps:
    - { id: hi, action: control.log, with: { message: "hi" } }
"#;
        validate_yaml(yaml).expect("should parse + validate");
    }

    #[test]
    fn slug_from_yaml_extracts_id() {
        let yaml = "apiVersion: lumorpa.io/v1\nkind: Flow\nmetadata: { id: my-flow_1 }\nspec:\n  steps: []\n";
        assert_eq!(slug_from_yaml(yaml).as_deref(), Some("my_flow_1"));
    }

    #[test]
    fn slug_from_yaml_returns_none_on_garbage() {
        assert!(slug_from_yaml(":::").is_none());
    }

    #[test]
    fn build_user_message_includes_retry_error() {
        let m = build_user_message("do x", Some("missing metadata.id"));
        assert!(m.contains("missing metadata.id"));
        assert!(m.contains("do x"));
    }

    #[test]
    fn system_prompt_uses_current_action_ids() {
        let prompt = system_prompt();
        assert!(prompt.contains("excel.read_rows"));
        assert!(prompt.contains("excel.write_row"));
        assert!(prompt.contains("ai.chat"));
        assert!(prompt.contains("Never use excel.read, excel.write, chat, data.set, or data.get"));
    }
}
