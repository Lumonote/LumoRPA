use std::sync::Arc;

use async_trait::async_trait;
use lumo_agent::{
    CapabilityDescriptor, CapabilityKind, CapabilitySource, FlowAdapter, InvocationAdapter,
    InvocationContext, InvocationError, InvocationRequest, McpAdapter, McpToolInvoker, RiskLevel,
    SkillAdapter,
};
use lumo_core::{Action, ActionRegistry, ActionResult, FlowVm, StepCtx, StepError};
use lumo_skills::SkillRegistry;
use serde_json::{json, Value};

fn descriptor(id: &str, source: CapabilitySource) -> CapabilityDescriptor {
    let mut descriptor = CapabilityDescriptor {
        id: id.into(),
        source,
        name: id.into(),
        description: String::new(),
        input_schema: json!({"type": "object"}),
        output_schema: None,
        aliases: vec![],
        examples: vec![],
        risk: RiskLevel::L0,
        enabled: true,
        version_hash: String::new(),
    };
    descriptor.refresh_version_hash();
    descriptor
}

fn request(capability: CapabilityDescriptor) -> InvocationRequest {
    InvocationRequest {
        capability,
        arguments: json!({"value": "echo"}),
        attempt: 1,
        timeout_ms: 2_000,
    }
}

struct EchoMcp;

#[async_trait]
impl McpToolInvoker for EchoMcp {
    async fn call(
        &self,
        _server: &str,
        _tool: &str,
        arguments: Value,
        _context: &InvocationContext,
        _timeout_ms: u64,
    ) -> Result<Value, InvocationError> {
        Ok(arguments)
    }
}

#[tokio::test]
async fn flow_skill_and_mcp_normalize_to_one_result_contract() {
    let temp = tempfile::tempdir().unwrap();
    let flow_path = temp.path().join("empty.lumoflow.yaml");
    std::fs::write(
        &flow_path,
        "apiVersion: lumorpa.io/v1\nkind: Flow\nmetadata: { id: empty }\nspec: { steps: [] }\n",
    )
    .unwrap();
    let skill_path = temp.path().join("SKILL.md");
    std::fs::write(
        &skill_path,
        "---\nname: empty-skill\n---\n```yaml\nsteps: []\n```\n",
    )
    .unwrap();
    let skill = lumo_skills::loader::load_skill_file(&skill_path).unwrap();
    let skills = Arc::new(SkillRegistry::new());
    skills.insert(skill);

    let vm_factory = Arc::new(|| FlowVm::new(ActionRegistry::new(), None));
    let flow = FlowAdapter::new(vm_factory.clone());
    let skill = SkillAdapter::new(skills, vm_factory);
    let mcp = McpAdapter::new(Arc::new(EchoMcp));
    let context = InvocationContext::new("run-1", "node-1");

    let flow_result = flow
        .invoke(
            request(descriptor(
                "flow:empty",
                CapabilitySource::Flow {
                    path: flow_path.display().to_string(),
                },
            )),
            context.clone(),
        )
        .await
        .unwrap();
    let skill_result = skill
        .invoke(
            request(descriptor(
                "skill:empty-skill",
                CapabilitySource::Skill {
                    name: "empty-skill".into(),
                    source: skill_path.display().to_string(),
                },
            )),
            context.clone(),
        )
        .await
        .unwrap();
    let mcp_result = mcp
        .invoke(
            request(descriptor(
                "mcp:srv/echo",
                CapabilitySource::Mcp {
                    server: "srv".into(),
                    tool: "echo".into(),
                },
            )),
            context,
        )
        .await
        .unwrap();

    assert_eq!(flow.source_kind(), CapabilityKind::Flow);
    assert_eq!(skill.source_kind(), CapabilityKind::Skill);
    assert_eq!(mcp.source_kind(), CapabilityKind::Mcp);
    assert_eq!(flow_result.output, Value::Null);
    assert_eq!(skill_result.output, Value::Null);
    assert_eq!(mcp_result.output, json!({"value": "echo"}));
}

struct MeteredAction;

#[async_trait]
impl Action for MeteredAction {
    fn id(&self) -> &'static str {
        "test.metered"
    }
    async fn execute(&self, ctx: &mut StepCtx, _: Value) -> Result<ActionResult, StepError> {
        ctx.repo()
            .unwrap()
            .record_ai_call(lumo_storage::AiCallInsert {
                flow_run_id: ctx.run_id(),
                step_id: Some("meter"),
                helper: "chat",
                provider: "test",
                model: "metered",
                input_tokens: 7,
                output_tokens: 5,
                latency_ms: 1,
                cost_usd_micro: 100,
            })
            .map_err(|error| StepError::msg(error.to_string()))?;
        Ok(ActionResult::from(json!({"ok":true})))
    }
}

#[tokio::test]
async fn flow_and_skill_results_carry_their_real_vm_ledger_usage() {
    let temp = tempfile::tempdir().unwrap();
    let flow_path = temp.path().join("metered.yaml");
    std::fs::write(&flow_path,"apiVersion: lumorpa.io/v1\nkind: Flow\nmetadata: { id: metered }\nspec:\n  steps:\n    - { id: meter, action: test.metered, with: {} }\n").unwrap();
    let skill_path = temp.path().join("SKILL.md");
    std::fs::write(&skill_path,"---\nname: metered-skill\n---\n```yaml\nsteps:\n  - { id: meter, action: test.metered, with: {} }\n```\n").unwrap();
    let skills = Arc::new(SkillRegistry::new());
    skills.insert(lumo_skills::loader::load_skill_file(&skill_path).unwrap());
    let repo = lumo_storage::Repo::open_in_memory().unwrap();
    let vm_repo = repo.clone();
    let factory = Arc::new(move || {
        let mut registry = ActionRegistry::new();
        registry.register(MeteredAction);
        FlowVm::new(registry, Some(vm_repo.clone()))
    });
    let adapters: Vec<(Box<dyn InvocationAdapter>, CapabilityDescriptor)> = vec![
        (
            Box::new(FlowAdapter::new(factory.clone())),
            descriptor(
                "flow:metered",
                CapabilitySource::Flow {
                    path: flow_path.display().to_string(),
                },
            ),
        ),
        (
            Box::new(SkillAdapter::new(skills, factory)),
            descriptor(
                "skill:metered",
                CapabilitySource::Skill {
                    name: "metered-skill".into(),
                    source: skill_path.display().to_string(),
                },
            ),
        ),
    ];
    for (adapter, capability) in adapters {
        let result = adapter
            .invoke(request(capability), InvocationContext::new("agent", "node"))
            .await
            .unwrap();
        assert_eq!(result.tokens_used, 12);
        assert_eq!(result.cost_usd_micro, 100);
        let flow_run_id = result.metadata["flowRunId"].as_str().unwrap();
        let calls = repo.list_ai_calls(flow_run_id).unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(
            calls[0].input_tokens + calls[0].output_tokens,
            result.tokens_used as i64
        );
        assert_eq!(calls[0].cost_usd_micro, result.cost_usd_micro as i64);
    }
}
