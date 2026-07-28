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

use crate::ast::{Capabilities, Flow, Step};
use crate::caps::{self, RequiredCap};
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

    // P2-6:network grant 形状检查。带 scheme/path/port 的 grant 运行期永不匹配
    // (门禁只拿裸 host 比对),是必然引发 CapabilityDenied 的死配置 → Error 级。
    for grant in &flow.spec.capabilities.network {
        if let Some(msg) = caps::network_grant_shape_error(grant) {
            issues.push(LintIssue::at(
                LintSeverity::Error,
                "capability.network_grant",
                None,
                msg,
            ));
        }
    }

    let input_names: BTreeSet<&str> = flow.spec.inputs.iter().map(|i| i.name.as_str()).collect();
    let known: BTreeSet<&str> = known_actions.iter().copied().collect();

    walk(
        &flow.spec.steps,
        &mut step_ids,
        &all_step_ids,
        &input_names,
        &known,
        &flow.spec.capabilities,
        &mut issues,
    );

    issues
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
    capabilities: &Capabilities,
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

        // P1-2:能力提示统一从 `caps::ACTION_CAPS` 声明表派生 —— 与 lumo-core
        // validate 及运行期 ensure_* 门禁同一份真源,不再按前缀猜。
        let action = s.action.as_str();
        if let Some(spec) = caps::action_caps(action) {
            for cap in spec.required {
                if !cap.is_granted(capabilities) {
                    out.push(LintIssue::at(
                        LintSeverity::Warn,
                        cap.lint_code(),
                        Some(&s.id),
                        format!(
                            "Action `{action}` {} at runtime but spec.capabilities.{} is empty \
                             — it will fail with CapabilityDenied",
                            cap_verb(cap),
                            cap.spec_field()
                        ),
                    ));
                }
            }
            if !spec.conditional.is_empty() {
                let with_json = serde_json::to_value(&s.with).unwrap_or(serde_json::Value::Null);
                for (key, cap) in spec.conditional {
                    if caps::conditional_key_engaged(&with_json, key)
                        && !cap.is_granted(capabilities)
                    {
                        out.push(LintIssue::at(
                            LintSeverity::Warn,
                            cap.lint_code(),
                            Some(&s.id),
                            format!(
                                "Action `{action}` {} when with.{key} is set, but \
                                 spec.capabilities.{} is empty — it will fail with CapabilityDenied",
                                cap_verb(cap),
                                cap.spec_field()
                            ),
                        ));
                    }
                }
            }
        }
        // 门禁双体系提示:system.shell / system.process_kill / system.app_start
        // 走环境变量开关而非 capability,目标机不置位就一律拒绝 —— 静态检查
        // 无法验证运行机环境,只能提示(warning 级,不是 error)。
        if let Some(var) = caps::env_gate(action) {
            out.push(LintIssue::at(
                LintSeverity::Warn,
                "system.env_gate",
                Some(&s.id),
                format!(
                    "Action `{action}` is disabled unless the machine running this flow \
                     sets {var}=1 in its environment (lint cannot verify the target machine)"
                ),
            ));
        }
        if let Some(ai) = &s.ai {
            if ai.is_enabled() && capabilities.llm.is_empty() {
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
            walk(child, seen, all_step_ids, inputs, known, capabilities, out);
        }
    }
}

/// 提示文案里对每类能力用一个动词短语,报错读起来是「动作会做什么」。
fn cap_verb(cap: &RequiredCap) -> String {
    match cap {
        RequiredCap::FsRead => "reads files".into(),
        RequiredCap::FsWrite => "writes to disk".into(),
        RequiredCap::Network => "hits the network".into(),
        RequiredCap::Llm => "calls an LLM".into(),
        RequiredCap::Mcp => "calls an MCP tool".into(),
        RequiredCap::Desktop(category) => format!("drives the desktop (`{category}`)"),
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
                "file.copy",
                "ai.chat",
                "image.ocr",
                "control.log",
                "csv.read",
                "db.postgres_query",
                "notify.send",
                "email.send",
                "human.approve",
                "mcp.call",
                "desktop.click",
                "system.shell",
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
    fn warns_on_missing_file_copy_capabilities() {
        let y = r#"
apiVersion: lumorpa.io/v1
kind: Flow
metadata: { id: t, version: 0.1.0 }
spec:
  steps:
    - id: copy
      action: file.copy
      with: { from: "./in.txt", to: "./out.txt" }
"#;
        let r = lint(y);
        assert!(has(&r, "capability.fs_read"), "issues: {r:?}");
        assert!(has(&r, "capability.fs_write"), "issues: {r:?}");
    }

    #[test]
    fn warns_on_missing_image_ocr_capabilities() {
        let y = r#"
apiVersion: lumorpa.io/v1
kind: Flow
metadata: { id: t, version: 0.1.0 }
spec:
  steps:
    - id: ocr
      action: image.ocr
      with: { image: "./captcha.png" }
"#;
        let r = lint(y);
        assert!(has(&r, "capability.fs_read"), "issues: {r:?}");
        assert!(has(&r, "capability.llm"), "issues: {r:?}");
    }

    #[test]
    fn warns_on_missing_dead_capability_families_from_table() {
        // 旧前缀启发完全看不见的动作族(db pg/mysql、notify、mcp、desktop)——
        // 现在统一从 caps::ACTION_CAPS 派生,漏授权在 lint 期就报。
        let y = r#"
apiVersion: lumorpa.io/v1
kind: Flow
metadata: { id: t, version: 0.1.0 }
spec:
  steps:
    - id: q
      action: db.postgres_query
      with: { dsn: "postgres://u:p@db/x", sql: "select 1" }
    - id: ping
      action: notify.send
      with: { provider: webhook, url: "https://hooks.internal/x" }
    - id: tool
      action: mcp.call
      with: { server: lumo, tool: list_flows }
    - id: click
      action: desktop.click
      with: { x: 1, y: 2 }
"#;
        let r = lint(y);
        let net_warns = r.iter().filter(|i| i.code == "capability.network").count();
        assert_eq!(net_warns, 2, "db + notify 各一条 network 警告: {r:?}");
        assert!(has(&r, "capability.mcp"), "issues: {r:?}");
        assert!(has(&r, "capability.desktop"), "issues: {r:?}");
    }

    #[test]
    fn conditional_caps_fire_only_when_with_key_engaged() {
        // email.send:network 恒需;attachments 出现才另需 fs.read。
        // human.approve:自身无门禁;notify 字段出现才需 network。
        let plain = r#"
apiVersion: lumorpa.io/v1
kind: Flow
metadata: { id: t, version: 0.1.0 }
spec:
  capabilities: { network: ["smtp.internal"] }
  steps:
    - id: mail
      action: email.send
      with: { host: "smtp.internal", from: a@b, to: [c@d], subject: hi, body: yo }
    - id: gate
      action: human.approve
      with: { message: "ship it?" }
"#;
        let r = lint(plain);
        assert!(
            !has(&r, "capability.fs_read") && !has(&r, "capability.network"),
            "无 attachments/notify 时不应告警: {r:?}"
        );

        let engaged = r#"
apiVersion: lumorpa.io/v1
kind: Flow
metadata: { id: t, version: 0.1.0 }
spec:
  steps:
    - id: mail
      action: email.send
      with: { host: "smtp.internal", from: a@b, to: [c@d], subject: hi, body: yo, attachments: ["./r.pdf"] }
    - id: gate
      action: human.approve
      with: { message: "ship it?", notify: { provider: webhook, url: "https://h/x" } }
"#;
        let r = lint(engaged);
        assert!(
            has(&r, "capability.fs_read"),
            "attachments → fs.read: {r:?}"
        );
        assert!(
            r.iter()
                .any(|i| i.code == "capability.network" && i.step.as_deref() == Some("gate")),
            "human.approve+notify → network: {r:?}"
        );
    }

    #[test]
    fn rejects_network_grant_with_scheme_or_path() {
        let y = r#"
apiVersion: lumorpa.io/v1
kind: Flow
metadata: { id: t, version: 0.1.0 }
spec:
  capabilities:
    network: ["https://example.com", "ok.internal", "*.corp.cn", "${DB_HOST}"]
  steps:
    - id: get
      action: http.request
      with: { url: "https://example.com" }
"#;
        let r = lint(y);
        let shape: Vec<_> = r
            .iter()
            .filter(|i| i.code == "capability.network_grant")
            .collect();
        assert_eq!(shape.len(), 1, "只有带 scheme 的 grant 应报: {r:?}");
        assert!(
            matches!(shape[0].severity, LintSeverity::Error),
            "死配置是 Error 级"
        );
        assert!(
            shape[0].message.contains("`example.com`"),
            "报错要给正确写法示例: {}",
            shape[0].message
        );
        // grant 列表非空,network 授权提示不应再叠加。
        assert!(!has(&r, "capability.network"), "issues: {r:?}");
    }

    #[test]
    fn hints_env_gate_for_shell() {
        let y = r#"
apiVersion: lumorpa.io/v1
kind: Flow
metadata: { id: t, version: 0.1.0 }
spec:
  steps:
    - id: sh
      action: system.shell
      with: { command: "echo hi" }
"#;
        let r = lint(y);
        let hint = r
            .iter()
            .find(|i| i.code == "system.env_gate")
            .expect("system.shell 应有环境变量开关提示");
        assert!(
            matches!(hint.severity, LintSeverity::Warn),
            "提示是 warning 级,不是 error"
        );
        assert!(
            hint.message.contains("LUMO_ALLOW_SHELL=1"),
            "提示应点名 LUMO_ALLOW_SHELL=1: {}",
            hint.message
        );
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
      retry: { times: 0, on: ["other"] }
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
