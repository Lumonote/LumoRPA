use lumo_dsl::{parse_str, render, validate, TemplateCtx};
use serde_json::json;

const HELLO: &str = r#"
apiVersion: lumorpa.io/v1
kind: Flow
metadata:
  id: hello
  version: 0.1.0
spec:
  inputs:
    - { name: who, type: string, default: world }
  steps:
    - id: greet
      action: control.log
      with: { message: "hello {{ inputs.who }}" }
"#;

#[test]
fn parse_minimal() {
    let f = parse_str(HELLO).expect("parse");
    assert_eq!(f.metadata.id, "hello");
    assert_eq!(f.spec.steps.len(), 1);
    assert_eq!(f.spec.steps[0].action, "control.log");
}

#[test]
fn validate_minimal() {
    let f = parse_str(HELLO).expect("parse");
    validate(&f).expect("validate");
}

#[test]
fn rejects_duplicate_step_ids() {
    let yaml = r#"
apiVersion: lumorpa.io/v1
kind: Flow
metadata: { id: dup }
spec:
  steps:
    - { id: a, action: control.log, with: {} }
    - { id: a, action: control.log, with: {} }
"#;
    let f = parse_str(yaml).expect("parse");
    assert!(validate(&f).is_err());
}

#[test]
fn rejects_stray_do_block() {
    let yaml = r#"
apiVersion: lumorpa.io/v1
kind: Flow
metadata: { id: bad }
spec:
  steps:
    - id: x
      action: control.log
      with: {}
      do:
        - { id: y, action: control.log, with: {} }
"#;
    let f = parse_str(yaml).expect("parse");
    assert!(validate(&f).is_err());
}

#[test]
fn template_renders_inputs() {
    let ctx = TemplateCtx {
        inputs: json!({ "who": "LumoRPA" }),
        ..Default::default()
    };
    let out = render(&json!("hello {{ inputs.who }}"), &ctx).unwrap();
    assert_eq!(out, json!("hello LumoRPA"));
}

#[test]
fn template_preserves_numeric_types() {
    let ctx = TemplateCtx {
        inputs: json!({ "n": 42 }),
        ..Default::default()
    };
    let out = render(&json!("{{ inputs.n }}"), &ctx).unwrap();
    assert_eq!(out, json!(42));
}

#[test]
fn template_recurses_into_objects() {
    let ctx = TemplateCtx {
        inputs: json!({ "host": "example.com" }),
        ..Default::default()
    };
    let out = render(
        &json!({ "url": "https://{{ inputs.host }}/", "static": 1 }),
        &ctx,
    )
    .unwrap();
    assert_eq!(out, json!({ "url": "https://example.com/", "static": 1 }));
}

#[test]
fn step_ai_defaults_to_off_when_absent() {
    let f = parse_str(HELLO).expect("parse");
    assert!(f.spec.steps[0].ai.is_none());
    validate(&f).expect("validate");
}

#[test]
fn step_ai_fallback_requires_llm_capability() {
    let yaml = r##"
apiVersion: lumorpa.io/v1
kind: Flow
metadata: { id: ai-no-cap }
spec:
  steps:
    - id: a
      action: browser.click
      with: { selector: "#x" }
      ai: { mode: fallback }
"##;
    let f = parse_str(yaml).expect("parse");
    let err = validate(&f).expect_err("should reject ai without llm cap");
    assert!(matches!(
        err,
        lumo_dsl::ValidationError::AiMissingLlmCapability { .. }
    ));
}

#[test]
fn step_ai_with_llm_capability_passes() {
    let yaml = r##"
apiVersion: lumorpa.io/v1
kind: Flow
metadata: { id: ai-ok }
spec:
  capabilities:
    llm: ["*"]
  steps:
    - id: a
      action: browser.click
      with: { selector: "#x" }
      ai: { mode: fallback }
"##;
    let f = parse_str(yaml).expect("parse");
    validate(&f).expect("validate");
}

#[test]
fn flow_ai_disabled_master_skips_cap_check() {
    let yaml = r##"
apiVersion: lumorpa.io/v1
kind: Flow
metadata:
  id: ai-disabled
  ai: { enabled: false }
spec:
  steps:
    - id: a
      action: browser.click
      with: { selector: "#x" }
      ai: { mode: primary }
"##;
    let f = parse_str(yaml).expect("parse");
    validate(&f).expect("ai disabled at flow level should bypass cap check");
}

#[test]
fn flow_ai_metadata_roundtrip() {
    let yaml = r#"
apiVersion: lumorpa.io/v1
kind: Flow
metadata:
  id: roundtrip
  ai:
    enabled: true
    model: "openai/gpt-4o"
    diagnose_on_failure: true
    budget: { max_calls_per_run: 25 }
spec:
  capabilities: { llm: ["*"] }
  steps:
    - id: a
      action: control.log
      with: { message: hi }
      ai:
        mode: fallback
        prompt: "say hi"
"#;
    let f = parse_str(yaml).expect("parse");
    let fa = f.metadata.ai.as_ref().expect("flow ai");
    assert!(fa.enabled);
    assert_eq!(fa.model.as_deref(), Some("openai/gpt-4o"));
    assert!(fa.diagnose_on_failure);
    assert_eq!(fa.budget.max_calls_per_run, 25);
    let sa = f.spec.steps[0].ai.as_ref().expect("step ai");
    assert!(matches!(sa.mode, lumo_dsl::AiMode::Fallback));
    assert_eq!(sa.prompt.as_deref(), Some("say hi"));
}
