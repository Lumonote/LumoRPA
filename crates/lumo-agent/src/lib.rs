mod capability;
mod catalog;
mod mcp_import;
mod mcp_profile;
mod profile;
mod skill_manager;

pub use capability::{CapabilityDescriptor, CapabilitySource, RiskLevel};
pub use catalog::{
    CapabilityCatalog, CapabilityCatalogBuilder, CapabilityCatalogError, CatalogError,
};
pub use mcp_import::{discover_macos_configs, import_bytes, ImportError};
pub use mcp_profile::{
    ConfigValue, DiscoveredConfig, ImportWarning, McpConfigSource, McpImportBatch, McpServerDraft,
    McpTransportDraft, SecretCandidate,
};
pub use profile::{
    validate as validate_profile, AgentProfile, AgentProfileDraft, PermissionDecision,
    PermissionRule, ProfileError,
};
pub use skill_manager::{SkillManager, SkillManagerError, SkillValidationReport, SkillVersion};
