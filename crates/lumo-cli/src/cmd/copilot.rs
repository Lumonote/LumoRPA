//! `lumo copilot` — NL → Flow generator (A-13).
//!
//! Takes a natural-language description, asks the configured LLM to draft a
//! lumo/v1 YAML flow, validates the result, and writes it to disk. If the LLM
//! returns invalid YAML or the DSL validator rejects it, retries once with
//! the failure reason appended to the prompt.

use clap::Args as ClapArgs;
use lumo_ai::{AiRouter, ChatMessage, ChatRequest, ProvidersConfig, Role};
use std::path::PathBuf;
use std::sync::Arc;

use super::providers_path;

#[derive(Debug, ClapArgs)]
pub struct Args {
    /// Natural-language description of the flow to generate.
    pub prompt: String,
    /// Output YAML file. Defaults to `./flows/<slug>.lumoflow.yaml`.
    #[arg(long)]
    pub out: Option<PathBuf>,
    /// Optional model override (e.g. `anthropic/claude-sonnet-4-6`). If absent,
    /// the active provider's default model is used.
    #[arg(long)]
    pub model: Option<String>,
    /// Print the generated YAML to stdout without writing a file.
    #[arg(long)]
    pub dry_run: bool,
}

pub async fn run(home: PathBuf, args: Args) -> anyhow::Result<()> {
    let providers_cfg = ProvidersConfig::load(providers_path(&home)).unwrap_or_default();
    let router = Arc::new(AiRouter::from_config(&providers_cfg));
    if router.provider_names().is_empty() {
        anyhow::bail!(
            "no LLM provider configured. Run `lumo providers add` first or set LUMO_PROVIDERS_PATH."
        );
    }
    let model = pick_model(&router, args.model.as_deref())?;

    let mut last_err: Option<String> = None;
    let mut yaml: Option<String> = None;
    for attempt in 0..2 {
        let user = build_user_message(&args.prompt, last_err.as_deref());
        let resp = router
            .chat(ChatRequest {
                model: model.clone(),
                messages: vec![ChatMessage::text(Role::User, user)],
                temperature: Some(0.2),
                max_tokens: Some(2048),
                system: Some(system_prompt()),
            })
            .await?;
        let candidate = extract_yaml(&resp.content);
        match validate_yaml(&candidate) {
            Ok(()) => {
                yaml = Some(candidate);
                break;
            }
            Err(e) => {
                eprintln!("◉ attempt {} validate failed: {e}", attempt + 1);
                last_err = Some(e);
            }
        }
    }
    let yaml = yaml.ok_or_else(|| {
        anyhow::anyhow!(
            "copilot could not produce a valid flow after 2 attempts: {}",
            last_err.unwrap_or_else(|| "<unknown>".into())
        )
    })?;

    if args.dry_run {
        println!("{yaml}");
        return Ok(());
    }

    let out = args.out.unwrap_or_else(|| {
        let slug = slug_from_yaml(&yaml).unwrap_or_else(|| "copilot".to_string());
        PathBuf::from("./flows").join(format!("{slug}.lumoflow.yaml"))
    });
    if let Some(parent) = out.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&out, &yaml)?;
    println!("✔ wrote {} ({} bytes)", out.display(), yaml.len());
    Ok(())
}

fn pick_model(router: &AiRouter, override_model: Option<&str>) -> anyhow::Result<String> {
    if let Some(m) = override_model {
        return Ok(m.to_string());
    }
    let active = router
        .active()
        .ok_or_else(|| anyhow::anyhow!("no active provider — run `lumo providers use <name>`"))?;
    // Format expected by router.chat: `<provider>/<model>`.
    Ok(format!("{active}/default"))
}

fn system_prompt() -> String {
    r#"You are a LumoRPA flow generator. Produce ONLY a single YAML document
matching the lumo/v1 Flow schema. Wrap output in ```yaml ... ``` fences.

Schema highlights:
- top: apiVersion, kind: Flow, metadata: { id }, spec: { triggers?, steps }
- triggers: [{ kind: webhook | cron | file, with: ... }]
  - cron uses { schedule: "<cron 6-field>" }
  - file uses { path, events?: [create|modify|remove], pattern? }
- steps: list of { id, action, with, when?, retry?, do?, else?, catch?, finally? }
- action ids include: control.log, control.if, control.parallel, data.set, data.get,
  file.read, file.write, http.request, browser.open, browser.click, browser.type,
  browser.extract, excel.read, excel.write, mcp.call, mcp.discover, chat
- capabilities: declare every fs.read/fs.write/network/llm/mcp grant the steps use.

Rules:
- step ids are snake_case and unique.
- do NOT invent action ids.
- output exactly one fenced YAML block. No prose."#
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

pub(crate) fn extract_yaml(response: &str) -> String {
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

pub(crate) fn validate_yaml(yaml: &str) -> Result<(), String> {
    if yaml.is_empty() {
        return Err("empty YAML".into());
    }
    let flow = lumo_dsl::parse_str(yaml).map_err(|e| format!("parse: {e}"))?;
    lumo_dsl::validate(&flow).map_err(|e| format!("validate: {e}"))?;
    Ok(())
}

pub(crate) fn slug_from_yaml(yaml: &str) -> Option<String> {
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
}
