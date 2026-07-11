use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum CapabilitySource {
    Flow { path: String },
    Skill { name: String, source: String },
    Mcp { server: String, tool: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum RiskLevel {
    L0,
    L1,
    L2,
    L3,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CapabilityDescriptor {
    pub id: String,
    pub source: CapabilitySource,
    pub name: String,
    pub description: String,
    pub input_schema: serde_json::Value,
    pub output_schema: Option<serde_json::Value>,
    pub aliases: Vec<String>,
    pub examples: Vec<String>,
    pub risk: RiskLevel,
    pub enabled: bool,
    pub version_hash: String,
}

impl CapabilityDescriptor {
    pub fn mcp(
        server: impl Into<String>,
        tool: impl Into<String>,
        input_schema: serde_json::Value,
    ) -> Self {
        let server = server.into();
        let tool = tool.into();
        let mut descriptor = Self {
            id: format!("mcp:{server}/{tool}"),
            source: CapabilitySource::Mcp {
                server,
                tool: tool.clone(),
            },
            name: tool,
            description: String::new(),
            input_schema,
            output_schema: None,
            aliases: Vec::new(),
            examples: Vec::new(),
            risk: RiskLevel::L0,
            enabled: true,
            version_hash: String::new(),
        };
        descriptor.version_hash = descriptor.compute_version_hash();
        descriptor
    }

    fn compute_version_hash(&self) -> String {
        let fields = (
            &self.id,
            &self.source,
            &self.name,
            &self.description,
            &self.input_schema,
            &self.output_schema,
            &self.aliases,
            &self.examples,
            self.risk,
            self.enabled,
        );
        let bytes = serde_json::to_vec(&fields).expect("capability fields are JSON serializable");
        format!("{:x}", Sha256::digest(bytes))
    }
}
