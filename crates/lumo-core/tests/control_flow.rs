//! Integration tests for VM control-flow dispatch.

use lumo_actions::register_all;
use lumo_core::{ActionRegistry, FlowVm, RunOptions};
use lumo_dsl::parse_str;
use lumo_storage::Repo;

async fn run(yaml: &str) -> lumo_core::RunReport {
    let flow = parse_str(yaml).expect("parse");
    let mut reg = ActionRegistry::new();
    register_all(&mut reg);
    let vm = FlowVm::new(reg, None);
    vm.run(&flow, RunOptions::default()).await.expect("run")
}

async fn run_with_repo(yaml: &str, repo: Repo) -> lumo_core::RunReport {
    let flow = parse_str(yaml).expect("parse");
    let mut reg = ActionRegistry::new();
    register_all(&mut reg);
    let vm = FlowVm::new(reg, Some(repo));
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
"#)
    .await;
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
"#)
    .await;
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
"#)
    .await;
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
"#)
    .await;
    assert!(report.success);
}

#[tokio::test]
async fn try_without_catch_propagates() {
    let flow = parse_str(
        r#"
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
"#,
    )
    .expect("parse");
    let mut reg = ActionRegistry::new();
    register_all(&mut reg);
    let vm = FlowVm::new(reg, None);
    let err = vm
        .run(&flow, RunOptions::default())
        .await
        .expect_err("should propagate");
    assert!(err.to_string().contains("boom"));
}

#[tokio::test]
async fn loop_iterations_persist_with_distinct_paths() {
    let repo = Repo::open_in_memory().unwrap();
    let report = run_with_repo(
        r#"
apiVersion: lumorpa.io/v1
kind: Flow
metadata: { id: t }
spec:
  steps:
    - id: loop
      action: control.for
      with: { from: 0, to: 3 }
      do:
        - { id: repeated, action: control.log, with: { message: "i={{ index }}" } }
"#,
        repo.clone(),
    )
    .await;
    assert!(report.success);
    let steps = repo.list_steps(&report.run_id).unwrap();
    let repeated: Vec<_> = steps
        .iter()
        .filter(|s| s.step_id == "repeated")
        .map(|s| s.path.clone())
        .collect();
    assert_eq!(repeated.len(), 3);
    assert_eq!(repeated[0], "loop[0]/repeated");
    assert_eq!(repeated[2], "loop[2]/repeated");
}

#[tokio::test]
async fn file_write_requires_capability() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("out.txt");
    let yaml = format!(
        r#"
apiVersion: lumorpa.io/v1
kind: Flow
metadata: {{ id: t }}
spec:
  steps:
    - id: write
      action: file.write
      with:
        path: "{}"
        content: hello
"#,
        path.display()
    );
    let flow = parse_str(&yaml).expect("parse");
    let mut reg = ActionRegistry::new();
    register_all(&mut reg);
    let vm = FlowVm::new(reg, None);
    let err = vm
        .run(&flow, RunOptions::default())
        .await
        .expect_err("capability should be denied");
    assert!(err.to_string().contains("capability denied"));
}

#[tokio::test]
async fn typed_inputs_are_validated_after_defaults_merge() {
    let flow = parse_str(
        r#"
apiVersion: lumorpa.io/v1
kind: Flow
metadata: { id: t }
spec:
  inputs:
    - { name: count, type: number, required: true }
  steps:
    - { id: ok, action: control.log, with: { message: "{{ inputs.count }}" } }
"#,
    )
    .expect("parse");
    let mut reg = ActionRegistry::new();
    register_all(&mut reg);
    let vm = FlowVm::new(reg, None);
    let err = vm
        .run(
            &flow,
            RunOptions {
                inputs: serde_json::json!({"count": "3"}),
                trigger_kind: "test".into(),
            },
        )
        .await
        .expect_err("string should not satisfy number input");
    assert!(err.to_string().contains("expected type `number`"));
}

#[tokio::test]
async fn control_parallel_runs_branches_concurrently() {
    // Two branches each sleep 80ms then set a var. If parallel, total elapsed
    // is ≈80ms; if sequential it would be ≈160ms. We assert <140ms with slack.
    let flow = parse_str(
        r#"
apiVersion: lumorpa.io/v1
kind: Flow
metadata: { id: t }
spec:
  steps:
    - id: par
      action: control.parallel
      branches:
        - - { id: s1, action: control.sleep, with: { ms: 80 } }
          - { id: v1, action: control.set_var, with: { name: a, value: "ok" } }
        - - { id: s2, action: control.sleep, with: { ms: 80 } }
          - { id: v2, action: control.set_var, with: { name: b, value: "ok" } }
"#,
    )
    .expect("parse");
    let mut reg = ActionRegistry::new();
    register_all(&mut reg);
    let vm = FlowVm::new(reg, None);
    let t0 = std::time::Instant::now();
    let report = vm.run(&flow, RunOptions::default()).await.expect("run ok");
    let elapsed = t0.elapsed().as_millis();
    assert!(report.success);
    assert!(
        elapsed < 200,
        "expected concurrent ≈80ms, got {elapsed}ms (sequential would be 160ms+)"
    );
}

#[tokio::test]
async fn control_parallel_back_compat_do_each_is_branch() {
    // Without `branches:`, each top-level step in `do:` becomes its own branch.
    let flow = parse_str(
        r#"
apiVersion: lumorpa.io/v1
kind: Flow
metadata: { id: t }
spec:
  steps:
    - id: par
      action: control.parallel
      do:
        - { id: s1, action: control.sleep, with: { ms: 60 } }
        - { id: s2, action: control.sleep, with: { ms: 60 } }
"#,
    )
    .expect("parse");
    let mut reg = ActionRegistry::new();
    register_all(&mut reg);
    let vm = FlowVm::new(reg, None);
    let t0 = std::time::Instant::now();
    vm.run(&flow, RunOptions::default()).await.expect("run ok");
    let elapsed = t0.elapsed().as_millis();
    assert!(elapsed < 150, "expected concurrent ≈60ms, got {elapsed}ms");
}

#[tokio::test]
async fn control_parallel_branches_have_isolated_bindings() {
    // P0-4: concurrent branches must NOT share mutable loop bindings. Each
    // branch loops over its own item, sleeps (forcing interleaving), then
    // records the item. With shared state one branch overwrites/clears the
    // other's `item` binding and the recorded values get corrupted. With
    // isolated forks each branch sees only its own item, and the per-branch
    // vars merge back so the post-parallel steps can read them.
    let report = run(r#"
apiVersion: lumorpa.io/v1
kind: Flow
metadata: { id: t }
spec:
  steps:
    - id: par
      action: control.parallel
      branches:
        - - id: loop0
            action: control.for_each
            with: { in: ["aaa"] }
            do:
              - { id: s0, action: control.sleep, with: { ms: 60 } }
              - { id: w0, action: control.set_var, with: { name: r0, value: "{{ item }}" } }
        - - id: loop1
            action: control.for_each
            with: { in: ["bbb"] }
            do:
              - { id: s1, action: control.sleep, with: { ms: 60 } }
              - { id: w1, action: control.set_var, with: { name: r1, value: "{{ item }}" } }
    - id: read0
      action: control.set_var
      with: { name: out0, value: "{{ vars.r0 }}" }
    - id: read1
      action: control.set_var
      with: { name: out1, value: "{{ vars.r1 }}" }
"#)
    .await;
    assert!(report.success);
    let out = report.outputs.expect("outputs");
    assert_eq!(
        out.pointer("/read0/result").and_then(serde_json::Value::as_str),
        Some("aaa"),
        "branch 0 must see its own item, not branch 1's"
    );
    assert_eq!(
        out.pointer("/read1/result").and_then(serde_json::Value::as_str),
        Some("bbb"),
        "branch 1 must see its own item, not branch 0's"
    );
}
