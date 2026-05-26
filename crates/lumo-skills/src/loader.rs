//! Parse SKILL.md (frontmatter + markdown + embedded yaml flow block).

use crate::model::{Skill, SkillFrontmatter};
use std::path::{Path, PathBuf};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum LoadError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("frontmatter missing or malformed in {path}")]
    Frontmatter { path: PathBuf },
    #[error("no ```yaml flow block found in {path}")]
    NoFlowBlock { path: PathBuf },
    #[error("frontmatter yaml: {0}")]
    FrontmatterYaml(#[from] serde_yaml::Error),
    #[error("flow parse: {0}")]
    FlowParse(#[from] lumo_dsl::ParseError),
    #[error("flow validate: {0}")]
    FlowValidate(#[from] lumo_dsl::ValidationError),
}

/// Load one SKILL.md file into a `Skill`.
pub fn load_skill_file(path: impl AsRef<Path>) -> Result<Skill, LoadError> {
    let path = path.as_ref().to_path_buf();
    let raw = std::fs::read_to_string(&path)?;
    let (fm_yaml, body) = split_frontmatter(&raw)
        .ok_or_else(|| LoadError::Frontmatter { path: path.clone() })?;
    let fm: SkillFrontmatter = serde_yaml::from_str(fm_yaml)?;

    let yaml_block = extract_fenced_yaml(body)
        .ok_or_else(|| LoadError::NoFlowBlock { path: path.clone() })?;
    let mut flow = compose_flow(&fm, yaml_block)?;
    // Ensure parsed flow id matches frontmatter name for consistent routing.
    if flow.metadata.id != fm.name {
        flow.metadata.id = fm.name.clone();
    }
    lumo_dsl::validate(&flow)?;

    Ok(Skill {
        frontmatter: fm,
        markdown: body.to_string(),
        flow,
        source: path,
    })
}

/// Load every `SKILL.md` under `root` (recursive, max-depth 3).
pub fn load_skills_dir(root: impl AsRef<Path>) -> Result<Vec<Skill>, LoadError> {
    let root = root.as_ref();
    let mut out = Vec::new();
    if !root.exists() { return Ok(out); }
    walk(root, 0, 3, &mut out)?;
    Ok(out)
}

fn walk(dir: &Path, depth: u32, max: u32, out: &mut Vec<Skill>) -> Result<(), LoadError> {
    if depth > max { return Ok(()); }
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let p = entry.path();
        if p.is_dir() {
            walk(&p, depth + 1, max, out)?;
        } else if p.file_name().is_some_and(|n| n == "SKILL.md") {
            match load_skill_file(&p) {
                Ok(s) => out.push(s),
                Err(e) => tracing::warn!("skill at {} failed to load: {}", p.display(), e),
            }
        }
    }
    Ok(())
}

fn split_frontmatter(s: &str) -> Option<(&str, &str)> {
    let s = s.strip_prefix("---")?;
    let s = s.strip_prefix('\n').or_else(|| s.strip_prefix("\r\n")).unwrap_or(s);
    let end = s.find("\n---")?;
    let fm = &s[..end];
    let mut rest = &s[end + 4..];
    if rest.starts_with('\n') { rest = &rest[1..]; }
    else if rest.starts_with("\r\n") { rest = &rest[2..]; }
    Some((fm, rest))
}

/// Extract the first fenced ```yaml block.
fn extract_fenced_yaml(body: &str) -> Option<&str> {
    let mut iter = body.match_indices("```yaml");
    let (start, _) = iter.next()?;
    let after = &body[start + 7..];
    // Skip optional language modifiers and the newline after the open fence.
    let nl = after.find('\n')?;
    let inner_start = nl + 1;
    let end = after[inner_start..].find("```")?;
    Some(&after[inner_start..inner_start + end])
}

/// A skill's ```yaml block may be just a top-level `spec:` (with `steps:`,
/// `inputs:`, etc.) — we wrap it into a full Flow document on demand.
fn compose_flow(fm: &SkillFrontmatter, yaml_block: &str) -> Result<lumo_dsl::Flow, LoadError> {
    // Try parsing as a full Flow first.
    if let Ok(flow) = lumo_dsl::parse_str(yaml_block) {
        return Ok(flow);
    }
    // Otherwise treat the block as a flow `spec` body and synthesise the wrapper.
    let composed = format!(
        "apiVersion: lumorpa.io/v1\nkind: Flow\nmetadata:\n  id: {id}\n  version: {ver}\nspec:\n{body}\n",
        id = fm.name,
        ver = fm.version.clone().unwrap_or_else(|| "0.1.0".into()),
        body = indent(yaml_block, "  "),
    );
    Ok(lumo_dsl::parse_str(&composed)?)
}

fn indent(s: &str, prefix: &str) -> String {
    let mut out = String::with_capacity(s.len() + 8);
    for line in s.lines() {
        if line.is_empty() {
            out.push('\n');
        } else {
            out.push_str(prefix);
            out.push_str(line);
            out.push('\n');
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_skill_with_full_flow() {
        let md = r#"---
name: hello-skill
description: a smoke skill
---

# hello

```yaml
apiVersion: lumorpa.io/v1
kind: Flow
metadata: { id: hello-skill }
spec:
  steps:
    - { id: a, action: control.log, with: { message: "hi" } }
```
"#;
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("SKILL.md");
        std::fs::write(&p, md).unwrap();
        let s = load_skill_file(&p).unwrap();
        assert_eq!(s.name(), "hello-skill");
        assert_eq!(s.flow.spec.steps.len(), 1);
    }

    #[test]
    fn parses_skill_with_spec_body() {
        let md = r#"---
name: short-skill
inputs:
  - { name: who, type: string }
---

```yaml
inputs:
  - { name: who, type: string }
steps:
  - { id: a, action: control.log, with: { message: "hi {{ inputs.who }}" } }
```
"#;
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("SKILL.md");
        std::fs::write(&p, md).unwrap();
        let s = load_skill_file(&p).unwrap();
        assert_eq!(s.name(), "short-skill");
        assert_eq!(s.flow.spec.steps.len(), 1);
    }
}
