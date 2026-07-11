use std::sync::Arc;

use async_trait::async_trait;
use lumo_agent::{
    CapabilityDescriptor, CapabilityKind, CapabilitySource, FlowAdapter, InvocationAdapter,
    InvocationContext, InvocationError, InvocationRequest, McpAdapter, McpToolInvoker, RiskLevel,
    SkillAdapter,
};
use lumo_core::{ActionRegistry, FlowVm};
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
