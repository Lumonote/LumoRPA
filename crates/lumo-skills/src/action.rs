//! `skill.invoke` action — call a registered Skill as a sub-flow.
//!
//! Inputs:
//!   * `name` — skill name (required)
//!   * `inputs` — map passed to the skill's `inputs` namespace
//!
//! Output: an object `{ outputs: {...}, vars: {...} }` — exactly what the
//! sub-flow's StepCtx held at completion. Captures both produced outputs and
//! any variables the skill wrote with `control.set_var`.

use async_trait::async_trait;
use lumo_core::error::StepError;
use lumo_core::{Action, ActionRegistry, ActionResult, FlowVm, RunOptions, StepCtx};
use once_cell::sync::Lazy;
use serde::Deserialize;
use serde_json::Value;
use std::sync::Arc;

use crate::registry::SkillRegistry;

/// Maximum `skill.invoke` nesting depth. Bounds runaway / cyclic recursion
/// (a skill invoking itself) before it can overflow the stack (P0-5).
const MAX_SKILL_DEPTH: u32 = 8;

pub fn register_skill_actions(reg: &mut ActionRegistry, skills: Arc<SkillRegistry>) {
    reg.register(InvokeAction { skills });
}

pub struct InvokeAction {
    pub skills: Arc<SkillRegistry>,
}

#[derive(Deserialize)]
struct InvokeIn {
    name: String,
    #[serde(default)]
    inputs: Value,
}

#[async_trait]
impl Action for InvokeAction {
    fn id(&self) -> &'static str {
        "skill.invoke"
    }
    fn summary(&self) -> &'static str {
        "Invoke a registered Skill (sub-flow)"
    }
    fn schema(&self) -> &'static serde_json::Value {
        static SCHEMA: Lazy<Value> = Lazy::new(|| {
            serde_json::json!({
                "type": "object",
                "required": ["name"],
                "properties": {
                    "name": { "type": "string" },
                    "inputs": { "type": "object" }
                },
                "additionalProperties": false
            })
        });
        &SCHEMA
    }
    async fn execute(&self, ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let InvokeIn { name, inputs } = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("skill.invoke input invalid: {e}")))?;

        let skill = self
            .skills
            .get(&name)
            .ok_or_else(|| StepError::msg(format!("unknown skill `{name}`")))?;

        // P0-5: bound recursion so a self-invoking / cyclic skill can't overflow
        // the stack. `skill_depth` is seeded into each sub-flow's context.
        let depth = ctx.skill_depth();
        if depth >= MAX_SKILL_DEPTH {
            return Err(StepError::msg(format!(
                "skill.invoke recursion limit reached ({MAX_SKILL_DEPTH}); possible cyclic skill `{name}`"
            )));
        }

        // P0-5: clamp the skill's declared capabilities to the caller's sandbox
        // so an invoked skill can never perform actions the calling flow was not
        // itself permitted to perform.
        let clamped =
            lumo_core::clamp_capabilities(&skill.flow.spec.capabilities, ctx.capabilities());

        // Run the skill's flow with the *same* action registry — so any
        // built-in / ai / skill actions stay available recursively — but with
        // the depth bumped and capabilities clamped.
        let vm = FlowVm::new(ctx.registry.clone(), None)
            .with_skill_depth(depth + 1)
            .with_capability_override(clamped);
        let report = vm
            .run(
                &skill.flow,
                RunOptions {
                    inputs,
                    trigger_kind: format!("skill:{}", name),
                },
            )
            .await
            .map_err(|e| StepError::msg(format!("skill `{name}`: {e}")))?;

        Ok(ActionResult::from(serde_json::json!({
            "skill": name,
            "run_id": report.run_id,
            "success": report.success,
            "outputs": report.outputs.unwrap_or(Value::Null),
        })))
    }
}
