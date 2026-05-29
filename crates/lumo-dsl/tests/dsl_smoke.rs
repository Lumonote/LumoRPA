use lumo_dsl::{parse_str, render, validate, TemplateCtx};
use serde_json::json;
use std::path::PathBuf;

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

// ---- P1-10 Task A: deny_unknown_fields ------------------------------------

#[test]
fn rejects_unknown_top_level_key() {
    let yaml = r#"
apiVersion: lumorpa.io/v1
kind: Flow
metadata: { id: t }
spec:
  steps:
    - { id: a, action: control.log, with: {} }
bogusTopLevel: 123
"#;
    let err = parse_str(yaml).expect_err("unknown top-level key must fail");
    let msg = err.to_string();
    assert!(
        msg.contains("bogusTopLevel"),
        "error should name the bad key, got: {msg}"
    );
}

#[test]
fn rejects_unknown_spec_key() {
    let yaml = r#"
apiVersion: lumorpa.io/v1
kind: Flow
metadata: { id: t }
spec:
  notARealSpecField: true
  steps:
    - { id: a, action: control.log, with: {} }
"#;
    let err = parse_str(yaml).expect_err("unknown spec key must fail");
    let msg = err.to_string();
    assert!(
        msg.contains("notARealSpecField"),
        "error should name the bad key, got: {msg}"
    );
}

#[test]
fn rejects_unknown_step_key() {
    let yaml = r#"
apiVersion: lumorpa.io/v1
kind: Flow
metadata: { id: t }
spec:
  steps:
    - id: a
      action: control.log
      with: {}
      typoField: oops
"#;
    let err = parse_str(yaml).expect_err("unknown step key must fail");
    let msg = err.to_string();
    assert!(
        msg.contains("typoField"),
        "error should name the bad key, got: {msg}"
    );
}

#[test]
fn rejects_unknown_metadata_key() {
    let yaml = r#"
apiVersion: lumorpa.io/v1
kind: Flow
metadata: { id: t, mispeltVersion: 1 }
spec:
  steps:
    - { id: a, action: control.log, with: {} }
"#;
    let err = parse_str(yaml).expect_err("unknown metadata key must fail");
    let msg = err.to_string();
    assert!(
        msg.contains("mispeltVersion"),
        "error should name the bad key, got: {msg}"
    );
}

#[test]
fn valid_flow_with_all_known_keys_still_parses() {
    let yaml = r#"
apiVersion: lumorpa.io/v1
kind: Flow
metadata:
  id: full
  version: 1.2.3
  name: Full
  description: every known key
  authors: [a]
  tags: [t]
  ai:
    enabled: true
    model: "x/y"
    diagnose_on_failure: true
    budget: { max_calls_per_run: 7 }
spec:
  inputs:
    - { name: x, type: string, default: world, required: false, description: d }
  outputs:
    - { name: y, type: string }
  vault: [smtp]
  triggers:
    - { kind: manual, with: {} }
  capabilities:
    network: ["*"]
    fs.read: ["/tmp/**"]
    fs.write: ["/tmp/**"]
    llm: ["*"]
    mcp: ["*"]
  resources:
    foo: bar
  steps:
    - id: a
      action: control.log
      with: { message: "hi {{ inputs.x }}" }
      retry: { times: 1, backoff: fixed, initial_ms: 100, on: [timeout] }
      when: "true"
      bind: out
      ai: { mode: fallback, model: "x/y", prompt: "p" }
"#;
    let f = parse_str(yaml).expect("valid all-known-keys flow must parse");
    assert_eq!(f.metadata.id, "full");
    assert_eq!(f.spec.steps.len(), 1);
}

// ---- P1-10 Task B: SemiStrict undefined behavior --------------------------

#[test]
fn render_undefined_variable_is_error() {
    let ctx = TemplateCtx::default();
    let out = render(&json!("value = {{ inputs.does_not_exist }}"), &ctx);
    assert!(
        out.is_err(),
        "rendering an undefined variable must Err under SemiStrict, got: {out:?}"
    );
}

#[test]
fn render_bare_undefined_variable_is_error() {
    // A standalone `{{ missing }}` (no surrounding text) must also Err, not
    // silently resolve to null via the lookup fast-path.
    let ctx = TemplateCtx::default();
    let out = render(&json!("{{ inputs.does_not_exist }}"), &ctx);
    assert!(
        out.is_err(),
        "bare undefined variable must Err under SemiStrict, got: {out:?}"
    );
}

#[test]
fn render_defined_variable_succeeds() {
    let ctx = TemplateCtx {
        inputs: json!({ "present": "ok" }),
        ..Default::default()
    };
    let out = render(&json!("value = {{ inputs.present }}"), &ctx).expect("defined var renders");
    assert_eq!(out, json!("value = ok"));
}

#[test]
fn render_is_defined_test_still_works() {
    // SemiStrict (not Strict) must keep `is defined` / default-style guards usable.
    let ctx = TemplateCtx::default();
    let out = render(
        &json!("{% if inputs.maybe is defined %}yes{% else %}no{% endif %}"),
        &ctx,
    )
    .expect("`is defined` guard must not error under SemiStrict");
    assert_eq!(out, json!("no"));
}

// ---- P1-10 Verification: example corpus must still parse -------------------

#[test]
fn example_corpus_parses_under_deny_unknown_fields() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("examples");
    let mut checked = 0;
    let mut failures = Vec::new();
    for entry in std::fs::read_dir(&root).expect("examples dir") {
        let path = entry.unwrap().path();
        let is_yaml = path
            .extension()
            .map(|e| e == "yaml" || e == "yml")
            .unwrap_or(false);
        if !is_yaml {
            continue;
        }
        checked += 1;
        let src = std::fs::read_to_string(&path).unwrap();
        if let Err(e) = parse_str(&src) {
            failures.push(format!("{}: {e}", path.display()));
        }
    }
    assert!(checked > 0, "expected to find example flows in {root:?}");
    assert!(
        failures.is_empty(),
        "example flows failed to parse:\n{}",
        failures.join("\n")
    );
}
