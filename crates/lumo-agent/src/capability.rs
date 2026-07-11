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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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
        descriptor.refresh_version_hash();
        descriptor
    }

    pub fn refresh_version_hash(&mut self) {
        self.version_hash = self.compute_version_hash();
    }

    pub fn has_valid_version_hash(&self) -> bool {
        self.version_hash == self.compute_version_hash()
    }

    fn compute_version_hash(&self) -> String {
        let fields = serde_json::Value::Array(vec![
            self.id.clone().into(),
            capability_source_value(&self.source),
            self.name.clone().into(),
            self.description.clone().into(),
            canonicalize_json(&self.input_schema),
            self.output_schema
                .as_ref()
                .map(canonicalize_json)
                .unwrap_or(serde_json::Value::Null),
            self.aliases
                .iter()
                .cloned()
                .map(serde_json::Value::String)
                .collect::<Vec<_>>()
                .into(),
            self.examples
                .iter()
                .cloned()
                .map(serde_json::Value::String)
                .collect::<Vec<_>>()
                .into(),
            risk_level_value(self.risk).into(),
            self.enabled.into(),
        ]);
        let canonical = canonicalize_json(&fields).to_string();
        format!("{:x}", Sha256::digest(canonical.as_bytes()))
    }
}

fn capability_source_value(source: &CapabilitySource) -> serde_json::Value {
    let fields = match source {
        CapabilitySource::Flow { path } => vec!["flow".into(), path.clone().into()],
        CapabilitySource::Skill { name, source } => {
            vec!["skill".into(), name.clone().into(), source.clone().into()]
        }
        CapabilitySource::Mcp { server, tool } => {
            vec!["mcp".into(), server.clone().into(), tool.clone().into()]
        }
    };
    serde_json::Value::Array(fields)
}

fn risk_level_value(risk: RiskLevel) -> &'static str {
    match risk {
        RiskLevel::L0 => "L0",
        RiskLevel::L1 => "L1",
        RiskLevel::L2 => "L2",
        RiskLevel::L3 => "L3",
    }
}

fn canonicalize_json(value: &serde_json::Value) -> serde_json::Value {
    match value {
        serde_json::Value::Array(values) => values
            .iter()
            .map(canonicalize_json)
            .collect::<Vec<_>>()
            .into(),
        serde_json::Value::Object(values) => {
            let mut entries = values.iter().collect::<Vec<_>>();
            entries.sort_unstable_by_key(|(key, _)| *key);
            let canonical = entries
                .into_iter()
                .map(|(key, value)| (key.clone(), canonicalize_json(value)))
                .collect();
            serde_json::Value::Object(canonical)
        }
        scalar => scalar.clone(),
    }
}
