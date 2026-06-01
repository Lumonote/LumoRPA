//! `flow.call` action — run another flow *file* as a sub-flow (F-15).
//!
//! Inputs:
//!   * `path` — relative path (within the configured base directory) to a flow
//!     `.yaml` file (required)
//!   * `inputs` — map passed to the sub-flow's `inputs` namespace
//!
//! Output: `{ flow, run_id, success, outputs }` — the sub-run's id plus the
//! outputs snapshot it held at completion.
//!
//! `flow.call` is the file-sourced sibling of [`crate::action`]'s
//! `skill.invoke`: same sub-VM on the *same* action registry, capabilities
//! clamped to the caller's sandbox, and recursion bounded by the **shared**
//! `skill_depth` counter (so a flow → skill → flow chain is bounded as one).
//! Where `skill.invoke` resolves a named, pre-installed skill, `flow.call`
//! resolves a path **confined to `base_dir`**: absolute paths and any `..`
//! component are rejected, so a flow can only call sibling flows under that
//! base — never read an arbitrary file off disk.

use async_trait::async_trait;
use lumo_core::error::StepError;
use lumo_core::{Action, ActionRegistry, ActionResult, FlowVm, RunOptions, StepCtx};
use once_cell::sync::Lazy;
use serde::Deserialize;
use serde_json::Value;
use std::path::{Component, Path, PathBuf};

/// Maximum sub-flow nesting depth. Shares the `StepCtx::skill_depth` budget
/// with `skill.invoke` (also 8), so mixed flow.call / skill.invoke chains are
/// bounded together against runaway / cyclic recursion (P0-5).
const MAX_FLOW_DEPTH: u32 = 8;

/// Register `flow.call`, resolving sub-flow paths within `base_dir` (typically
/// the running flow's directory, falling back to `$LUMO_HOME`).
pub fn register_flow_call_action(reg: &mut ActionRegistry, base_dir: PathBuf) {
    reg.register(FlowCallAction { base_dir });
}

pub struct FlowCallAction {
    pub base_dir: PathBuf,
}

#[derive(Deserialize)]
struct FlowCallIn {
    path: String,
    #[serde(default)]
    inputs: Value,
}

/// A relative path with no `..` components and no absolute/root/prefix anchor —
/// the only shape `flow.call` will resolve against its base directory.
fn is_confined(path: &str) -> bool {
    let p = Path::new(path);
    !p.components().any(|c| {
        matches!(
            c,
            Component::ParentDir | Component::RootDir | Component::Prefix(_)
        )
    })
}

#[async_trait]
impl Action for FlowCallAction {
    fn id(&self) -> &'static str {
        "flow.call"
    }
    fn summary(&self) -> &'static str {
        "Run another flow file as a sub-flow"
    }
    fn schema(&self) -> &'static Value {
        static SCHEMA: Lazy<Value> = Lazy::new(|| {
            serde_json::json!({
                "type": "object",
                "required": ["path"],
                "properties": {
                    "path": { "type": "string" },
                    "inputs": { "type": "object" }
                },
                "additionalProperties": false
            })
        });
        &SCHEMA
    }
    async fn execute(&self, ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let FlowCallIn { path, inputs } = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("flow.call input invalid: {e}")))?;

        // Sandbox: only relative, non-escaping paths under `base_dir`. Rejected
        // *before* any filesystem access so a malicious `..`/absolute path can
        // never even stat an out-of-tree file.
        if !is_confined(&path) {
            return Err(StepError::msg(format!(
                "flow.call path `{path}` must be relative and stay within the flows directory \
                 (no `..`, no absolute paths)"
            )));
        }
        let full = self.base_dir.join(&path);
        let src = std::fs::read_to_string(&full)
            .map_err(|e| StepError::msg(format!("flow.call cannot read `{path}`: {e}")))?;
        let flow = lumo_dsl::parse_str(&src)
            .map_err(|e| StepError::msg(format!("flow.call parse `{path}`: {e}")))?;

        // P0-5: bound recursion before descending, so a self-/cyclic-calling
        // flow can't overflow the stack. `skill_depth` is the shared sub-flow
        // depth, seeded into each sub-run's context.
        let depth = ctx.skill_depth();
        if depth >= MAX_FLOW_DEPTH {
            return Err(StepError::msg(format!(
                "flow.call recursion limit reached ({MAX_FLOW_DEPTH}); possible cyclic flow `{path}`"
            )));
        }

        // P0-5: clamp the sub-flow's declared capabilities to the caller's
        // sandbox so it can never do what the calling flow could not.
        let clamped = lumo_core::clamp_capabilities(&flow.spec.capabilities, ctx.capabilities());

        // Reuse the caller's action registry (so built-in / ai / skill / flow
        // actions stay available recursively), with depth bumped + caps clamped.
        let vm = FlowVm::new(ctx.registry.clone(), None)
            .with_skill_depth(depth + 1)
            .with_capability_override(clamped);
        let report = vm
            .run(
                &flow,
                RunOptions {
                    inputs,
                    trigger_kind: format!("flow.call:{path}"),
                },
            )
            .await
            .map_err(|e| StepError::msg(format!("flow `{path}`: {e}")))?;

        Ok(ActionResult::from(serde_json::json!({
            "flow": path,
            "run_id": report.run_id,
            "success": report.success,
            "outputs": report.outputs.unwrap_or(Value::Null),
        })))
    }
}

#[cfg(test)]
mod tests {
    use super::is_confined;

    #[test]
    fn confinement_rules() {
        assert!(is_confined("sub.yaml"));
        assert!(is_confined("flows/sub.yaml"));
        assert!(is_confined("a/b/c.yaml"));
        assert!(!is_confined("../sub.yaml"));
        assert!(!is_confined("a/../../etc/passwd"));
        assert!(!is_confined("/etc/hosts"));
    }
}
