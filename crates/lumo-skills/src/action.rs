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
        // built-in / ai / skill actions stay available recursively.
        // 架构 P0-1:子 VM 经 child_of 继承父运行的执行环境(同一 cancel 令牌、
        // step_timeout、artifacts、human prompter、repo、vault、AI provider,
        // depth 内建 +1),不再裸 new 丢环境;能力仍按声明 clamp 后只收不放。
        let vm = FlowVm::child_of(ctx).with_capability_override(clamped);
        // 同 flow.call:spawn 隔离父级取消的 future drop,子任务经共享 cancel
        // 令牌自行判死并完成 teardown,子流程资源不泄漏。
        let flow = skill.flow.clone();
        let trigger_kind = format!("skill:{}", name);
        let handle = tokio::spawn(async move {
            vm.run(
                &flow,
                RunOptions {
                    inputs,
                    trigger_kind,
                },
            )
            .await
        });
        let report = handle
            .await
            .map_err(|e| StepError::msg(format!("skill `{name}` task: {e}")))?
            .map_err(|e| StepError::msg(format!("skill `{name}`: {e}")))?;

        Ok(ActionResult::from(serde_json::json!({
            "skill": name,
            "run_id": report.run_id,
            "success": report.success,
            "outputs": report.outputs.unwrap_or(Value::Null),
        })))
    }
}
