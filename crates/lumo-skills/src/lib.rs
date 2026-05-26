//! LumoRPA Skills — reusable, named, parameterised flow snippets stored as
//! Markdown files with YAML frontmatter.
//!
//! Inspired by Claude Code's `SKILL.md` convention. A skill lives at
//! `~/.lumorpa/skills/<name>/SKILL.md`; the file holds YAML frontmatter, a
//! free-form markdown body, and a fenced ```yaml block containing either a
//! full Flow document or a bare `spec:`-style body (loader wraps it).
//!
//! Skills are loadable as `Flow` objects and invocable via the `skill.invoke`
//! action, exposed automatically when [`register_skill_actions`] is called.

pub mod action;
pub mod loader;
pub mod model;
pub mod registry;

pub use model::{Skill, SkillFrontmatter};
pub use registry::SkillRegistry;
pub use action::register_skill_actions;
