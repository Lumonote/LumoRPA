use std::collections::BTreeMap;
use std::fmt;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum McpTransportDraft {
    Stdio {
        command: String,
        args: Vec<String>,
        env: BTreeMap<String, ConfigValue>,
    },
    StreamableHttp {
        url: String,
        headers: BTreeMap<String, ConfigValue>,
    },
    Sse {
        url: String,
        headers: BTreeMap<String, ConfigValue>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "camelCase")]
pub enum ConfigValue {
    Plain(String),
    VaultRef(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpServerDraft {
    pub id: String,
    pub name: String,
    pub transport: McpTransportDraft,
    pub enabled: bool,
    #[serde(default, flatten)]
    pub extensions: serde_json::Map<String, Value>,
}

impl McpServerDraft {
    pub fn redacted_json(&self) -> Value {
        serde_json::to_value(self).expect("MCP server drafts contain only serializable values")
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct SecretCandidate {
    pub server_id: String,
    pub field_path: String,
    pub suggested_vault_key: String,
    pub value: String,
}

impl fmt::Debug for SecretCandidate {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SecretCandidate")
            .field("server_id", &self.server_id)
            .field("field_path", &self.field_path)
            .field("suggested_vault_key", &self.suggested_vault_key)
            .field("value", &"[REDACTED]")
            .finish()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportWarning {
    pub server_id: String,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct McpImportBatch {
    pub servers: Vec<McpServerDraft>,
    pub secrets: Vec<SecretCandidate>,
    pub warnings: Vec<ImportWarning>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum McpConfigSource {
    ClaudeDesktop,
    Cursor,
    Codex,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveredConfig {
    pub source: McpConfigSource,
    pub source_name: &'static str,
    pub path: PathBuf,
}
