//! Immutable executable sources and explicitly declared file dependencies.

use std::{
    collections::BTreeMap,
    io::Read,
    path::{Component, Path, PathBuf},
};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub const MAX_SOURCE_BYTES: usize = 8 * 1024 * 1024;
pub const MAX_SOURCE_FILES: usize = 1024;
pub const MAX_DEPENDENCY_BYTES: u64 = 128 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SourceSnapshot {
    pub root: PathBuf,
    pub flow_base: PathBuf,
    pub sources: BTreeMap<PathBuf, String>,
    pub skills: BTreeMap<String, PathBuf>,
    /// File hashes, rather than file contents, avoid copying credentials and
    /// large input documents into persisted Agent plans.
    #[serde(default)]
    pub dependencies: BTreeMap<PathBuf, String>,
}

impl SourceSnapshot {
    pub fn source(&self, path: &Path) -> Result<&str, String> {
        let path = absolute_path(path)?;
        self.sources.get(&path).map(String::as_str).ok_or_else(|| {
            format!(
                "source `{}` is outside the approved dependency snapshot; create a new plan",
                path.display()
            )
        })
    }

    pub fn skill_source(&self, name: &str) -> Result<(&Path, &str), String> {
        let path = self
            .skills
            .get(name)
            .ok_or_else(|| format!("skill `{name}` is outside the approved dependency snapshot"))?;
        Ok((path.as_path(), self.source(path)?))
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.sources.len() > MAX_SOURCE_FILES
            || self.sources.values().map(String::len).sum::<usize>() > MAX_SOURCE_BYTES
            || self.dependencies.len() > MAX_SOURCE_FILES
        {
            return Err("dependency snapshot exceeds its size limit".into());
        }
        if !self.root.is_absolute() || !self.flow_base.is_absolute() {
            return Err("dependency snapshot paths must be absolute".into());
        }
        self.source(&self.root)?;
        for path in self.sources.keys().chain(self.dependencies.keys()) {
            if absolute_path(path)? != *path {
                return Err("dependency snapshot contains a non-normalized path".into());
            }
        }
        for name in self.skills.keys() {
            self.skill_source(name)?;
        }
        if self
            .dependencies
            .values()
            .any(|hash| hash.len() != 64 || !hash.bytes().all(|b| b.is_ascii_hexdigit()))
        {
            return Err("dependency snapshot contains an invalid file hash".into());
        }
        Ok(())
    }

    pub fn verify_dependencies(&self) -> Result<(), String> {
        for (path, expected) in &self.dependencies {
            if &hash_file(path)? != expected {
                return Err(format!(
                    "dependency `{}` changed after approval; create a new plan",
                    path.display()
                ));
            }
        }
        Ok(())
    }
}

/// Normalize lexically: a frozen source remains addressable after its original
/// file is removed. Capture separately rejects symlinks escaping the source root.
pub fn absolute_path(path: &Path) -> Result<PathBuf, String> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map_err(|e| e.to_string())?
            .join(path)
    };
    let mut normalized = PathBuf::new();
    for component in absolute.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            component => normalized.push(component.as_os_str()),
        }
    }
    Ok(normalized)
}

pub fn hash_file(path: &Path) -> Result<String, String> {
    // Check before open so a declared FIFO cannot block plan approval waiting
    // for a writer. Recheck the opened handle in case the path was replaced.
    if !std::fs::metadata(path)
        .map_err(|e| format!("cannot read dependency `{}`: {e}", path.display()))?
        .is_file()
    {
        return Err(format!(
            "dependency `{}` must be a regular file",
            path.display()
        ));
    }
    let mut file = std::fs::File::open(path)
        .map_err(|e| format!("cannot read dependency `{}`: {e}", path.display()))?;
    let metadata = file.metadata().map_err(|e| e.to_string())?;
    if !metadata.is_file() {
        return Err(format!(
            "dependency `{}` must be a regular file",
            path.display()
        ));
    }
    if metadata.len() > MAX_DEPENDENCY_BYTES {
        return Err(format!("dependency `{}` exceeds 128 MiB", path.display()));
    }
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    let mut total = 0_u64;
    loop {
        let count = file.read(&mut buffer).map_err(|e| e.to_string())?;
        if count == 0 {
            break;
        }
        total += count as u64;
        if total > MAX_DEPENDENCY_BYTES {
            return Err("dependency grew beyond its size limit".into());
        }
        hasher.update(&buffer[..count]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}
