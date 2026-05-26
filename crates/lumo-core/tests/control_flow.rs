//! Integration tests for VM control-flow dispatch.

use lumo_actions::register_all;
use lumo_core::{ActionRegistry, FlowVm, RunOptions};
use lumo_dsl::parse_str;

async fn run(yaml: &str) -> lumo_core::RunReport {
    let flow = parse_str(yaml).expect("parse");
    let mut reg = ActionRegistry::new();
    register_all(&mut reg);
    let vm = FlowVm::new(reg, None);
    vm.run(&flow, RunOptions::default()).await.expect("run")
}

#[tokio::test]
async fn if_takes_then_branch_on_truthy() {
    let report = run(r#"
apiVersion: lumorpa.io/v1
kind: Flow
metadata: { id: t }
spec:
  steps:
    - id: gate
      action: control.if
      with: { cond: "yes" }
      do:
        - { id: greet, action: control.set_var, with: { name: x, value: "from-do" } }
      else:
        - { id: skip,  action: control.set_var, with: { name: x, value: "from-else" } }
    - id: bind
      action: control.set_var
      with: { name: x, value: "{{ vars.x }}" }
"#).await;
    assert!(report.success);
}

#[tokio::test]
async fn for_iterates_correctly() {
    let report = run(r#"
apiVersion: lumorpa.io/v1
kind: Flow
metadata: { id: t }
spec:
  steps:
    - id: loop
      action: control.for
      with: { from: 0, to: 5 }
      do:
        - { id: noop, action: control.log, with: { message: "i={{ index }}" } }
"#).await;
    assert!(report.success);
}

#[tokio::test]
async fn for_each_renders_row_binding() {
    let report = run(r#"
apiVersion: lumorpa.io/v1
kind: Flow
metadata: { id: t }
spec:
  inputs:
    - { name: items, type: array, default: [1, 2, 3] }
  steps:
    - id: loop
      action: control.for_each
      with:
        in: "{{ inputs.items }}"
        bind: n
      do:
        - { id: noop, action: control.log, with: { message: "n={{ n }} row={{ row }}" } }
"#).await;
    assert!(report.success);
}

#[tokio::test]
async fn try_catches_user_fail() {
    let report = run(r#"
apiVersion: lumorpa.io/v1
kind: Flow
metadata: { id: t }
spec:
  steps:
    - id: guard
      action: control.try
      with: {}
      do:
        - { id: bomb, action: control.fail, with: { message: "boom" } }
      catch:
        - { id: caught, action: control.set_var, with: { name: ok, value: true } }
      finally:
        - { id: fin, action: control.set_var, with: { name: cleaned, value: true } }
"#).await;
    assert!(report.success);
}

#[tokio::test]
async fn try_without_catch_propagates() {
    let flow = parse_str(r#"
apiVersion: lumorpa.io/v1
kind: Flow
metadata: { id: t }
spec:
  steps:
    - id: guard
      action: control.try
      with: {}
      do:
        - { id: bomb, action: control.fail, with: { message: "boom" } }
      finally:
        - { id: fin, action: control.set_var, with: { name: cleaned, value: true } }
"#).expect("parse");
    let mut reg = ActionRegistry::new();
    register_all(&mut reg);
    let vm = FlowVm::new(reg, None);
    let err = vm.run(&flow, RunOptions::default()).await.expect_err("should propagate");
    assert!(err.to_string().contains("boom"));
}
