use std::{path::Path, sync::Arc};

use lumo_agent::{
    capture_source_snapshot, CapabilityDescriptor, CapabilitySource, FlowAdapter,
    InvocationAdapter, InvocationContext, InvocationRequest, RiskLevel, SOURCE_SNAPSHOTS_KEY,
};
use lumo_core::{ActionRegistry, FlowVm};
use lumo_skills::{register_flow_call_action, register_skill_actions, SkillRegistry};
use serde_json::json;

fn write_flow(path: &Path, steps: &str, extra: &str) {
    std::fs::write(path, format!("apiVersion: lumorpa.io/v1\nkind: Flow\nmetadata: {{ id: snapshot-test }}\nspec:\n{extra}  steps:\n{steps}\n")).unwrap();
}

fn descriptor(path: &Path) -> CapabilityDescriptor {
    let mut descriptor = CapabilityDescriptor {
        id: "flow:snapshot-test".into(),
        source: CapabilitySource::Flow {
            path: path.to_string_lossy().into_owned(),
        },
        name: "snapshot-test".into(),
        description: String::new(),
        input_schema: json!({"type":"object"}),
        output_schema: None,
        aliases: vec![],
        examples: vec![],
        risk: RiskLevel::L0,
        enabled: true,
        version_hash: String::new(),
    };
    descriptor.bind_source_content(&std::fs::read(path).unwrap());
    descriptor
}

#[tokio::test]
async fn dynamic_flow_and_skill_execute_the_serialized_snapshot_after_files_change() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("main.yaml");
    let child = temp.path().join("child.lumoflow.yaml");
    let skill_path = temp.path().join("SKILL.md");
    write_flow(&root, "    - id: child\n      action: flow.call\n      with: {path: '{{ inputs.child }}'}\n    - id: skill\n      action: skill.invoke\n      with: {name: '{{ inputs.skill }}'}", "");
    write_flow(
        &child,
        "    - {id: old, action: control.log, with: {message: old-child}}",
        "",
    );
    std::fs::write(&skill_path, "---\nname: helper\n---\n```yaml\nsteps:\n  - {id: old, action: control.log, with: {message: old-skill}}\n```\n").unwrap();
    let skills = Arc::new(SkillRegistry::new());
    skills.insert(lumo_skills::loader::load_skill_file(&skill_path).unwrap());
    let frozen = capture_source_snapshot(&root, &skills).unwrap();
    let persisted = serde_json::to_value(frozen).unwrap();
    let capability = descriptor(&root);
    // A restart with new source/registry content must still execute the plan
    // that was approved, including a dynamically selected dependency.
    write_flow(
        &child,
        "    - {id: fail, action: control.fail, with: {message: new-child}}",
        "",
    );
    std::fs::remove_file(&root).unwrap();
    std::fs::remove_file(&skill_path).unwrap();
    skills.remove("helper");
    let base = temp.path().to_path_buf();
    let adapter = FlowAdapter::new(Arc::new(move || {
        let mut registry = ActionRegistry::new();
        lumo_actions::register_all(&mut registry);
        register_flow_call_action(&mut registry, base.clone());
        register_skill_actions(&mut registry, skills.clone());
        FlowVm::new(registry, None)
    }));
    let mut context = InvocationContext::new("run", "node");
    context
        .metadata
        .insert(SOURCE_SNAPSHOTS_KEY.into(), persisted);
    adapter
        .invoke(
            InvocationRequest {
                capability,
                arguments: json!({"child":"child.lumoflow.yaml","skill":"helper"}),
                attempt: 1,
                timeout_ms: 2_000,
            },
            context,
        )
        .await
        .unwrap();
}

#[test]
fn declared_file_dependencies_detect_changed_bytes_without_copying_them() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("main.yaml");
    let data = temp.path().join("template.txt");
    std::fs::write(&data, "original template").unwrap();
    write_flow(
        &root,
        "    - {id: log, action: control.log, with: {message: ok}}",
        "  dependencies: [template.txt]\n",
    );
    let snapshot = capture_source_snapshot(&root, &SkillRegistry::new()).unwrap();
    snapshot.verify_dependencies().unwrap();
    assert!(!serde_json::to_string(&snapshot)
        .unwrap()
        .contains("original template"));
    std::fs::write(&data, "changed template").unwrap();
    assert!(snapshot
        .verify_dependencies()
        .unwrap_err()
        .contains("changed after approval"));
}

#[test]
fn declared_directory_dependency_is_rejected() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("main.yaml");
    std::fs::create_dir(temp.path().join("data")).unwrap();
    write_flow(
        &root,
        "    - {id: log, action: control.log, with: {message: ok}}",
        "  dependencies: [data]\n",
    );
    assert!(capture_source_snapshot(&root, &SkillRegistry::new())
        .unwrap_err()
        .contains("regular file"));
}

#[cfg(unix)]
#[test]
fn declared_fifo_dependency_is_rejected_without_waiting_for_a_writer() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("main.yaml");
    assert!(std::process::Command::new("mkfifo")
        .arg(temp.path().join("data"))
        .status()
        .unwrap()
        .success());
    write_flow(
        &root,
        "    - {id: log, action: control.log, with: {message: ok}}",
        "  dependencies: [data]\n",
    );
    let (sender, receiver) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let result = capture_source_snapshot(&root, &SkillRegistry::new());
        let _ = sender.send(result);
    });
    let result = receiver
        .recv_timeout(std::time::Duration::from_secs(2))
        .expect("dependency hashing must reject FIFOs without blocking on open");
    assert!(result.unwrap_err().contains("regular file"));
}

#[test]
fn unrelated_invalid_sibling_does_not_block_a_static_plan() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("main.yaml");
    write_flow(
        &root,
        "    - {id: log, action: control.log, with: {message: ok}}",
        "",
    );
    std::fs::write(temp.path().join("draft.lumoflow.yaml"), "broken: [").unwrap();
    assert_eq!(
        capture_source_snapshot(&root, &SkillRegistry::new())
            .unwrap()
            .sources
            .len(),
        1
    );
}

#[test]
fn unrelated_installed_skill_does_not_block_an_independent_flow() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("main.yaml");
    let skill_path = temp.path().join("SKILL.md");
    write_flow(
        &root,
        "    - {id: log, action: control.log, with: {message: ok}}",
        "",
    );
    std::fs::write(&skill_path,"---\nname: unused\n---\n```yaml\nsteps:\n  - {id: log, action: control.log, with: {message: unused}}\n```\n").unwrap();
    let skills = SkillRegistry::new();
    skills.insert(lumo_skills::loader::load_skill_file(&skill_path).unwrap());
    let frozen = capture_source_snapshot(&root, &skills).unwrap();
    assert_eq!(frozen.sources.len(), 1);
    assert!(frozen.skills.is_empty());
    frozen.validate().unwrap();
}

#[test]
fn dynamic_references_capture_plain_yaml_flows_and_ignore_invalid_drafts() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("main.yaml");
    let child = temp.path().join("child.yaml");
    write_flow(
        &root,
        "    - {id: call, action: flow.call, with: {path: '{{ inputs.file }}'}}",
        "",
    );
    write_flow(
        &child,
        "    - {id: log, action: control.log, with: {message: frozen}}",
        "",
    );
    std::fs::write(temp.path().join("draft.lumoflow.yaml"), "broken: [").unwrap();
    std::fs::write(temp.path().join("binary-draft.yaml"), [0xff, 0xfe]).unwrap();
    let frozen = capture_source_snapshot(&root, &SkillRegistry::new()).unwrap();
    assert!(frozen.source(&child).unwrap().contains("frozen"));
    assert_eq!(frozen.sources.len(), 2);
    std::fs::write(&child, "changed after planning").unwrap();
    assert!(frozen.source(&child).unwrap().contains("frozen"));
}

#[test]
fn repeated_dynamic_calls_count_each_source_only_once() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("main.yaml");
    write_flow(
        &root,
        "    - {id: first, action: flow.call, with: {path: '{{ inputs.first }}'}}\n    - {id: second, action: flow.call, with: {path: '{{ inputs.second }}'}}",
        "",
    );
    for index in 0..512 {
        write_flow(
            &temp.path().join(format!("child-{index}.yaml")),
            "    - {id: log, action: control.log, with: {message: frozen}}",
            "",
        );
    }
    let frozen = capture_source_snapshot(&root, &SkillRegistry::new()).unwrap();
    assert_eq!(frozen.sources.len(), 513);
}

#[test]
fn dynamic_sources_reject_more_than_the_unique_file_limit() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("main.yaml");
    write_flow(
        &root,
        "    - {id: call, action: flow.call, with: {path: '{{ inputs.file }}'}}",
        "",
    );
    for index in 0..lumo_core::source_snapshot::MAX_SOURCE_FILES {
        write_flow(
            &temp.path().join(format!("child-{index}.yaml")),
            "    - {id: log, action: control.log, with: {message: frozen}}",
            "",
        );
    }
    assert!(capture_source_snapshot(&root, &SkillRegistry::new())
        .unwrap_err()
        .contains("1024 unique files"));
}

#[test]
fn oversized_source_is_rejected_and_oversized_dynamic_candidate_is_ignored() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("main.yaml");
    let large = temp.path().join("oversized.yaml");
    std::fs::File::create(&large)
        .unwrap()
        .set_len(lumo_core::source_snapshot::MAX_SOURCE_BYTES as u64 + 1)
        .unwrap();
    assert!(capture_source_snapshot(&large, &SkillRegistry::new())
        .unwrap_err()
        .contains("8 MiB"));
    write_flow(
        &root,
        "    - {id: call, action: flow.call, with: {path: '{{ inputs.file }}'}}",
        "",
    );
    let frozen = capture_source_snapshot(&root, &SkillRegistry::new()).unwrap();
    assert_eq!(frozen.sources.len(), 1);
}

#[cfg(unix)]
#[test]
fn fifo_source_is_rejected_without_waiting_for_a_writer() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("source.yaml");
    assert!(std::process::Command::new("mkfifo")
        .arg(&source)
        .status()
        .unwrap()
        .success());
    let (sender, receiver) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let result = capture_source_snapshot(&source, &SkillRegistry::new());
        let _ = sender.send(result);
    });
    let result = receiver
        .recv_timeout(std::time::Duration::from_secs(2))
        .expect("snapshot must reject FIFOs without blocking on open");
    assert!(result.unwrap_err().contains("regular file"));
}

#[cfg(unix)]
#[test]
fn dependency_symlink_escape_is_rejected() {
    let temp = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let root = temp.path().join("main.yaml");
    let child = outside.path().join("child.yaml");
    write_flow(
        &child,
        "    - {id: log, action: control.log, with: {message: ok}}",
        "",
    );
    std::os::unix::fs::symlink(&child, temp.path().join("child.yaml")).unwrap();
    write_flow(
        &root,
        "    - {id: child, action: flow.call, with: {path: child.yaml}}",
        "",
    );
    assert!(capture_source_snapshot(&root, &SkillRegistry::new())
        .unwrap_err()
        .contains("escapes"));
}
