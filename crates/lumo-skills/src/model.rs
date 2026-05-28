use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillFrontmatter {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub version: Option<String>,
    #[serde(default)]
    pub inputs: Vec<lumo_dsl::IoDecl>,
    #[serde(default)]
    pub outputs: Vec<lumo_dsl::IoDecl>,
    /// Keywords / patterns that suggest invoking this skill (used by
    /// future LLM routers as system-prompt hints).
    #[serde(default)]
    pub triggers: Vec<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub author: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Skill {
    pub frontmatter: SkillFrontmatter,
    /// Markdown body (everything after the closing `---` line).
    pub markdown: String,
    /// Compiled `Flow` extracted from a fenced ```yaml block.
    pub flow: lumo_dsl::Flow,
    /// On-disk source path (`SKILL.md`).
    pub source: PathBuf,
}

impl Skill {
    pub fn name(&self) -> &str {
        &self.frontmatter.name
    }
    pub fn description(&self) -> Option<&str> {
        self.frontmatter.description.as_deref()
    }
}
