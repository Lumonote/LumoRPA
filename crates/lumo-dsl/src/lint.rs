//! D-19 Flow Lint: structural & best-practice checks beyond `validate`.
//!
//! `validate` rejects malformed flows (collisions, control-flow misuse).
//! Lint is *advisory*: it surfaces issues that won't stop the VM from
//! trying, but will probably bite the user at runtime — undeclared variable
//! references, missing capability grants, dead retry policies, references
//! to unknown actions, etc.
//!
//! Each finding carries a stable `code` so Studio can wire actionable
//! "+ add capability" / "+ declare input" buttons per kind.

use crate::ast::{Flow, Step};
use serde::Serialize;
use std::collections::BTreeSet;

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LintSeverity {
    Error,
    Warn,
    Info,
}

#[derive(Debug, Clone, Serialize)]
pub struct LintIssue {
    pub severity: LintSeverity,
    pub code: &'static str,
    pub step: Option<String>,
    pub message: String,
}

impl LintIssue {
    fn at(severity: LintSeverity, code: &'static str, step: Option<&str>, msg: String) -> Self {
        Self {
            severity,
            code,
            step: step.map(str::to_string),
            message: msg,
        }
    }
}

/// Run all lint checks. `known_actions` lets the linter complain about
/// references to undefined actions; pass an empty slice to skip that check.
pub fn lint_flow(flow: &Flow, known_actions: &[&str]) -> Vec<LintIssue> {
    let mut issues = Vec::new();
    if flow.spec.steps.is_empty() {
        issues.push(LintIssue::at(
            LintSeverity::Info,
            "flow.empty",
            None,
            "Flow has no steps in spec.steps".into(),
        ));
    }

    let mut step_ids = BTreeSet::new();
    let mut all_step_ids = BTreeSet::new();
    collect_step_ids(&flow.spec.steps, &mut all_step_ids);

    let input_names: BTreeSet<&str> = flow.spec.inputs.iter().map(|i| i.name.as_str()).collect();
    let known: BTreeSet<&str> = known_actions.iter().copied().collect();
    let has_llm_cap = !flow.spec.capabilities.llm.is_empty();
    let has_net_cap = !flow.spec.capabilities.network.is_empty();
    let has_fs_read = !flow.spec.capabilities.fs_read.is_empty();
    let has_fs_write = !flow.spec.capabilities.fs_write.is_empty();
    let has_mcp_cap = !flow.spec.capabilities.mcp.is_empty();

    walk(
        &flow.spec.steps,
        &mut step_ids,
        &all_step_ids,
        &input_names,
        &known,
        Ctx {
            has_llm_cap,
            has_net_cap,
            has_fs_read,
            has_fs_write,
            has_mcp_cap,
        },
        &mut issues,
    );

    issues
}

#[derive(Clone, Copy)]
struct Ctx {
    has_llm_cap: bool,
    has_net_cap: bool,
    has_fs_read: bool,
    has_fs_write: bool,
    has_mcp_cap: bool,
}

fn collect_step_ids(steps: &[Step], out: &mut BTreeSet<String>) {
    for s in steps {
        out.insert(s.id.clone());
        for child in s.children() {
            collect_step_ids(child, out);
        }
    }
}

fn walk(
    steps: &[Step],
    seen: &mut BTreeSet<String>,
    all_step_ids: &BTreeSet<String>,
    inputs: &BTreeSet<&str>,
    known: &BTreeSet<&str>,
    ctx: Ctx,
    out: &mut Vec<LintIssue>,
) {
    for s in steps {
        if s.id.trim().is_empty() {
            out.push(LintIssue::at(
                LintSeverity::Error,
                "step.empty_id",
                None,
                "Step has empty id".into(),
            ));
        } else if s.id.chars().any(char::is_whitespace) {
            out.push(LintIssue::at(
                LintSeverity::Warn,
                "step.bad_id",
                Some(&s.id),
                format!("Step id `{}` contains whitespace; prefer dashes", s.id),
            ));
        }
        seen.insert(s.id.clone());

        if !known.is_empty() && !s.action.trim().is_empty() && !known.contains(s.action.as_str()) {
            out.push(LintIssue::at(
                LintSeverity::Warn,
                "action.unknown",
                Some(&s.id),
                format!("Unknown action `{}` (not in registry)", s.action),
            ));
        }

        // Capability hints (best-effort by action prefix).
        let action = s.action.as_str();
        if action.starts_with("http.") && !ctx.has_net_cap {
            out.push(LintIssue::at(
                LintSeverity::Warn,
                "capability.network",
                Some(&s.id),
                format!(
                    "Action `{action}` will hit the network but spec.capabilities.network is empty"
                ),
            ));
        }
        if (action.starts_with("file.")
            && matches!(action, "file.read" | "file.exists" | "csv.read")
            || matches!(action, "csv.read"))
            && !ctx.has_fs_read
        {
            out.push(LintIssue::at(
                LintSeverity::Warn,
                "capability.fs_read",
                Some(&s.id),
                format!("Action `{action}` reads files but spec.capabilities.\"fs.read\" is empty"),
            ));
        }
        if matches!(
            action,
            "file.write" | "csv.write" | "excel.write_row" | "db.sqlite_exec"
        ) && !ctx.has_fs_write
        {
            out.push(LintIssue::at(
                LintSeverity::Warn,
                "capability.fs_write",
                Some(&s.id),
                format!(
                    "Action `{action}` writes to disk but spec.capabilities.\"fs.write\" is empty"
                ),
            ));
        }
        if action.starts_with("ai.")
            && !ctx.has_llm_cap
            && s.ai
                .as_ref()
                .map_or(action == "ai.chat", |a| a.is_enabled())
        {
            out.push(LintIssue::at(
                LintSeverity::Warn,
                "capability.llm",
                Some(&s.id),
                format!("Action `{action}` calls an LLM but spec.capabilities.llm is empty"),
            ));
        }
        if action.starts_with("mcp.") && !ctx.has_mcp_cap {
            out.push(LintIssue::at(
                LintSeverity::Warn,
                "capability.mcp",
                Some(&s.id),
                format!("Action `{action}` calls an MCP tool but spec.capabilities.mcp is empty"),
            ));
        }
        if let Some(ai) = &s.ai {
            if ai.is_enabled() && !ctx.has_llm_cap {
                out.push(LintIssue::at(
                    LintSeverity::Warn,
                    "capability.llm",
                    Some(&s.id),
                    "step.ai is enabled but spec.capabilities.llm is empty".into(),
                ));
            }
        }

        // Retry dead policy.
        if let Some(retry) = &s.retry {
            if retry.times == 0 && !retry.on.is_empty() {
                out.push(LintIssue::at(
                    LintSeverity::Warn,
                    "retry.dead",
                    Some(&s.id),
                    "retry.on is set but retry.times is 0 — the policy will never fire".into(),
                ));
            }
        }

        // Template variable references.
        let yaml_text = serde_yaml::to_string(&s.with).unwrap_or_default();
        for r in scan_refs(&yaml_text) {
            check_ref(&r, inputs, all_step_ids, &s.id, out);
        }
        if let Some(w) = &s.when {
            for r in scan_refs(w) {
                check_ref(&r, inputs, all_step_ids, &s.id, out);
            }
        }

        for child in s.children() {
            walk(child, seen, all_step_ids, inputs, known, ctx, out);
        }
    }
}

#[derive(Debug)]
struct TemplateRef {
    root: String,
    next: Option<String>,
    raw: String,
}

/// Naive scan for `{{ ... }}` references — pulls the first dotted identifier
/// chain after the opening braces. Handles minijinja syntax well enough for
/// `inputs.x`, `vars.y`, `steps.id.result`, `env.HOME`, `vault.smtp.password`.
fn scan_refs(text: &str) -> Vec<TemplateRef> {
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(start) = rest.find("{{") {
        let after = &rest[start + 2..];
        let Some(end) = after.find("}}") else { break };
        let expr = after[..end].trim();
        if let Some(t) = first_token_chain(expr) {
            let raw = format!("{{{{ {expr} }}}}");
            out.push(TemplateRef {
                root: t.0,
                next: t.1,
                raw,
            });
        }
        rest = &after[end + 2..];
    }
    out
}

fn first_token_chain(expr: &str) -> Option<(String, Option<String>)> {
    let mut buf = String::new();
    for c in expr.chars() {
        if c.is_alphanumeric() || c == '_' || c == '.' {
            buf.push(c);
        } else if buf.is_empty() {
            continue;
        } else {
            break;
        }
    }
    if buf.is_empty() {
        return None;
    }
    let mut parts = buf.split('.');
    let root = parts.next()?.to_string();
    let next = parts.next().map(|s| s.to_string());
    Some((root, next))
}

fn check_ref(
    r: &TemplateRef,
    inputs: &BTreeSet<&str>,
    step_ids: &BTreeSet<String>,
    cur_step: &str,
    out: &mut Vec<LintIssue>,
) {
    match r.root.as_str() {
        "inputs" => {
            if let Some(name) = &r.next {
                if !inputs.contains(name.as_str()) {
                    out.push(LintIssue::at(
                        LintSeverity::Warn,
                        "template.undeclared_input",
                        Some(cur_step),
                        format!("`{}` references input `{}` not in spec.inputs", r.raw, name),
                    ));
                }
            }
        }
        "steps" => {
            if let Some(name) = &r.next {
                if !step_ids.contains(name) {
                    out.push(LintIssue::at(
                        LintSeverity::Warn,
                        "template.unknown_step",
                        Some(cur_step),
                        format!(
                            "`{}` references step id `{}` that does not exist",
                            r.raw, name
                        ),
                    ));
                }
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse_str;

    fn lint(yaml: &str) -> Vec<LintIssue> {
        let flow = parse_str(yaml).expect("parse");
        lint_flow(
            &flow,
            &[
                "browser.open",
                "browser.click",
                "browser.extract",
                "http.request",
                "file.read",
                "file.write",
                "ai.chat",
                "control.log",
                "csv.read",
            ],
        )
    }

    fn has(issues: &[LintIssue], code: &str) -> bool {
        issues.iter().any(|i| i.code == code)
    }

    #[test]
    fn warns_on_unknown_action() {
        let y = r#"
apiVersion: lumorpa.io/v1
kind: Flow
metadata: { id: t, version: 0.1.0 }
spec:
  steps:
    - id: a
      action: totally.fake
"#;
        let r = lint(y);
        assert!(has(&r, "action.unknown"), "issues: {r:?}");
    }

    #[test]
    fn warns_on_missing_network_capability() {
        let y = r#"
apiVersion: lumorpa.io/v1
kind: Flow
metadata: { id: t, version: 0.1.0 }
spec:
  steps:
    - id: get
      action: http.request
      with: { url: "https://example.com" }
"#;
        let r = lint(y);
        assert!(has(&r, "capability.network"), "issues: {r:?}");
    }

    #[test]
    fn warns_on_undeclared_input_ref() {
        let y = r#"
apiVersion: lumorpa.io/v1
kind: Flow
metadata: { id: t, version: 0.1.0 }
spec:
  steps:
    - id: log
      action: control.log
      with: { message: "hi {{ inputs.missing }}" }
"#;
        let r = lint(y);
        assert!(has(&r, "template.undeclared_input"), "issues: {r:?}");
    }

    #[test]
    fn warns_on_dead_retry() {
        let y = r#"
apiVersion: lumorpa.io/v1
kind: Flow
metadata: { id: t, version: 0.1.0 }
spec:
  steps:
    - id: a
      action: control.log
      with: { message: "x" }
      retry: { times: 0, on: ["timeout"] }
"#;
        let r = lint(y);
        assert!(has(&r, "retry.dead"), "issues: {r:?}");
    }

    #[test]
    fn clean_flow_has_no_warnings() {
        let y = r#"
apiVersion: lumorpa.io/v1
kind: Flow
metadata: { id: t, version: 0.1.0 }
spec:
  inputs:
    - { name: url }
  capabilities:
    network: ["*"]
  steps:
    - id: open
      action: browser.open
      with: { url: "{{ inputs.url }}" }
    - id: log
      action: control.log
      with: { message: "opened {{ steps.open.result }}" }
"#;
        let issues = lint(y);
        assert!(
            !issues
                .iter()
                .any(|i| matches!(i.severity, LintSeverity::Warn | LintSeverity::Error)),
            "expected clean flow, got: {issues:?}"
        );
    }
}
