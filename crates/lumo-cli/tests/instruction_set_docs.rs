use lumo_ai::{AiRouter, ChatAction};
use lumo_core::ActionRegistry;
use lumo_skills::{register_flow_call_action, register_skill_actions, SkillRegistry};
use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::Arc;

fn full_cli_registry() -> ActionRegistry {
    let mut registry = ActionRegistry::new();
    lumo_actions::register_all(&mut registry);
    registry.register(ChatAction::new(Arc::new(AiRouter::builder().build())));
    register_skill_actions(&mut registry, Arc::new(SkillRegistry::new()));
    register_flow_call_action(&mut registry, PathBuf::from("."));
    registry
}

fn documented_action_ids() -> BTreeSet<String> {
    let doc = include_str!("../../../docs/05-LumoFlow-Instruction-Set.md");
    let start = doc
        .find("<!-- ACTIONS_START -->")
        .expect("instruction-set doc must contain ACTIONS_START marker");
    let end = doc
        .find("<!-- ACTIONS_END -->")
        .expect("instruction-set doc must contain ACTIONS_END marker");
    assert!(start < end, "ACTIONS_START must appear before ACTIONS_END");

    let mut ids = BTreeSet::new();
    let mut rest = &doc[start..end];
    while let Some(open) = rest.find('`') {
        rest = &rest[open + 1..];
        let Some(close) = rest.find('`') else {
            panic!("unclosed backtick in action list");
        };
        let candidate = &rest[..close];
        if is_action_id(candidate) {
            ids.insert(candidate.to_string());
        }
        rest = &rest[close + 1..];
    }
    ids
}

fn is_action_id(s: &str) -> bool {
    s.contains('.')
        && s.bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'.')
}

#[test]
fn instruction_set_action_list_matches_default_cli_registry() {
    let registry = full_cli_registry();
    let registered: BTreeSet<String> = registry.iter_ids().collect();
    let documented = documented_action_ids();

    let missing_from_doc: Vec<_> = registered.difference(&documented).cloned().collect();
    let missing_from_registry: Vec<_> = documented.difference(&registered).cloned().collect();

    assert!(
        missing_from_doc.is_empty() && missing_from_registry.is_empty(),
        "instruction-set action list is out of sync\nmissing from doc: {missing_from_doc:?}\nmissing from registry: {missing_from_registry:?}"
    );
}

#[test]
fn documented_actions_have_summaries_and_object_schemas() {
    let registry = full_cli_registry();
    for id in documented_action_ids() {
        let action = registry
            .get(&id)
            .unwrap_or_else(|| panic!("documented action `{id}` is not registered"));
        assert!(
            !action.summary().trim().is_empty(),
            "documented action `{id}` should expose a summary"
        );
        assert_eq!(
            action.schema().get("type").and_then(|v| v.as_str()),
            Some("object"),
            "documented action `{id}` should expose an object input schema"
        );
    }
}
