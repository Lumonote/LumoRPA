//! B2 (F-17): every shipped example flow's leaf-action `with:` must validate
//! against that action's schema. This guards two things at once: that the
//! validator doesn't reject legitimate flows, and that the hand-written action
//! schemas are complete enough for the real flows the project ships.
//!
//! Validation runs on the RAW (pre-render) `with`, so `{{ }}` template values
//! are skipped by the validator; what's exercised here is key structure
//! (required present, no unknown fields) plus literal-value types.

use lumo_actions::register_all;
use lumo_core::{schema::validate_input, ActionRegistry};
use lumo_dsl::{parse_str, Step};
use std::path::PathBuf;

fn check_steps(reg: &ActionRegistry, steps: &[Step], file: &str, errs: &mut Vec<String>) {
    for step in steps {
        // `control.*` are dispatched inline by the VM and not schema-validated.
        if !step.action.starts_with("control.") {
            if let Some(action) = reg.get(&step.action) {
                let raw = serde_json::to_value(&step.with).unwrap_or(serde_json::Value::Null);
                if let Err(msg) = validate_input(action.schema(), &raw) {
                    errs.push(format!(
                        "{file} :: step `{}` ({}) → {msg}",
                        step.id, step.action
                    ));
                }
            }
        }
        for child in step.children() {
            check_steps(reg, child, file, errs);
        }
    }
}

#[test]
fn example_flows_satisfy_action_schemas() {
    let mut reg = ActionRegistry::new();
    register_all(&mut reg);

    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples");
    let mut errs = Vec::new();
    let mut checked = 0;
    for entry in std::fs::read_dir(&root).expect("examples dir") {
        let path = entry.expect("dir entry").path();
        if path.extension().and_then(|e| e.to_str()) != Some("yaml") {
            continue;
        }
        let src = std::fs::read_to_string(&path).expect("read example");
        let Ok(flow) = parse_str(&src) else {
            continue; // parse failures are the dsl_smoke test's concern
        };
        checked += 1;
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        check_steps(&reg, &flow.spec.steps, &name, &mut errs);
    }

    assert!(checked > 0, "expected example flows in {root:?}");
    assert!(
        errs.is_empty(),
        "example flows violate their action schemas (validator too strict, or a \
         schema is missing a real field):\n{}",
        errs.join("\n")
    );
}
