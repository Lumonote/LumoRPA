use lumo_dsl::{parse_str, validate, render, TemplateCtx};
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
    let ctx = TemplateCtx { inputs: json!({ "n": 42 }), ..Default::default() };
    let out = render(&json!("{{ inputs.n }}"), &ctx).unwrap();
    assert_eq!(out, json!(42));
}

#[test]
fn template_recurses_into_objects() {
    let ctx = TemplateCtx { inputs: json!({ "host": "example.com" }), ..Default::default() };
    let out = render(
        &json!({ "url": "https://{{ inputs.host }}/", "static": 1 }),
        &ctx,
    ).unwrap();
    assert_eq!(out, json!({ "url": "https://example.com/", "static": 1 }));
}
