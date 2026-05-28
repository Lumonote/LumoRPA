use crate::model::Skill;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

#[derive(Default, Clone)]
pub struct SkillRegistry {
    by_name: Arc<parking_lot::Mutex<HashMap<String, Arc<Skill>>>>,
}

impl SkillRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(&self, skill: Skill) {
        self.by_name
            .lock()
            .insert(skill.name().to_string(), Arc::new(skill));
    }

    pub fn get(&self, name: &str) -> Option<Arc<Skill>> {
        self.by_name.lock().get(name).cloned()
    }

    pub fn names(&self) -> Vec<String> {
        let g = self.by_name.lock();
        let mut v: Vec<_> = g.keys().cloned().collect();
        v.sort();
        v
    }

    pub fn all(&self) -> Vec<Arc<Skill>> {
        self.by_name.lock().values().cloned().collect()
    }

    pub fn load_dir(&self, root: impl AsRef<Path>) -> Result<usize, crate::loader::LoadError> {
        let skills = crate::loader::load_skills_dir(root)?;
        let mut g = self.by_name.lock();
        let n = skills.len();
        for s in skills {
            g.insert(s.name().to_string(), Arc::new(s));
        }
        Ok(n)
    }

    /// `~/.lumorpa/skills` (or env override).
    pub fn default_root() -> PathBuf {
        if let Ok(p) = std::env::var("LUMO_SKILLS_PATH") {
            return PathBuf::from(p);
        }
        if let Ok(p) = std::env::var("LUMO_HOME") {
            return PathBuf::from(p).join("skills");
        }
        dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(".lumorpa")
            .join("skills")
    }
}
