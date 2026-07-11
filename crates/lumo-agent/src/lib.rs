mod capability;
mod mcp_import;
mod mcp_profile;

pub use capability::{CapabilityDescriptor, CapabilitySource, RiskLevel};
pub use mcp_import::{discover_macos_configs, import_bytes, ImportError};
pub use mcp_profile::{
    ConfigValue, DiscoveredConfig, ImportWarning, McpConfigSource, McpImportBatch, McpServerDraft,
    McpTransportDraft, SecretCandidate,
};
