use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs, io,
    path::{Path, PathBuf},
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};
use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillVersion {
    pub name: String,
    pub hash: String,
    pub path: PathBuf,
    pub enabled: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillValidationReport {
    pub valid: bool,
    pub name: Option<String>,
    pub error: Option<String>,
}

#[derive(Debug, Error)]
pub enum SkillManagerError {
    #[error("I/O error: {0}")]
    Io(#[from] io::Error),
    #[error("invalid skill: {0}")]
    InvalidSkill(String),
    #[error("unsafe skill name: {0}")]
    UnsafeName(String),
    #[error("unsafe source path: {0}")]
    UnsafePath(PathBuf),
    #[error("skill version not found: {0}/{1}")]
    VersionNotFound(String, String),
    #[error("git operation failed: {0}")]
    Git(String),
    #[error("no previous active version for {0}")]
    NoPreviousVersion(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ActiveManifest {
    hash: String,
    enabled: bool,
    #[serde(default)]
    history: Vec<String>,
}

pub struct SkillManager {
    root: PathBuf,
}

impl SkillManager {
    pub fn new(root: impl Into<PathBuf>) -> Result<Self, SkillManagerError> {
        let root = root.into();
        fs::create_dir_all(&root)?;
        Ok(Self { root })
    }
    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn import_local(
        &self,
        source: impl AsRef<Path>,
    ) -> Result<SkillVersion, SkillManagerError> {
        let source = source.as_ref();
        let md = if source.is_dir() {
            source.join("SKILL.md")
        } else {
            source.to_path_buf()
        };
        let meta = fs::symlink_metadata(&md).map_err(SkillManagerError::Io)?;
        if meta.file_type().is_symlink() || !meta.is_file() {
            return Err(SkillManagerError::UnsafePath(md));
        }
        let bytes = fs::read(&md)?;
        let staging = self.root.join(".staging").join(unique());
        fs::create_dir_all(&staging)?;
        let staged = staging.join("SKILL.md");
        fs::write(&staged, &bytes)?;
        let loaded = lumo_skills::loader::load_skill_file(&staged)
            .map_err(|e| SkillManagerError::InvalidSkill(e.to_string()));
        let loaded = match loaded {
            Ok(v) => v,
            Err(e) => {
                let _ = fs::remove_dir_all(&staging);
                return Err(e);
            }
        };
        validate_name(loaded.name())?;
        let hash = format!("{:x}", Sha256::digest(&bytes));
        let version_dir = self.root.join(loaded.name()).join("versions").join(&hash);
        let final_file = version_dir.join("SKILL.md");
        if final_file.exists() {
            fs::remove_dir_all(&staging)?;
        } else {
            fs::create_dir_all(version_dir.parent().expect("version parent"))?;
            fs::rename(&staging, &version_dir)?;
        }
        Ok(SkillVersion {
            name: loaded.name().to_owned(),
            hash,
            path: final_file,
            enabled: self
                .active(loaded.name())?
                .map(|a| a.enabled)
                .unwrap_or(true),
        })
    }

    pub fn import_git(
        &self,
        url: &str,
        revision: Option<&str>,
    ) -> Result<SkillVersion, SkillManagerError> {
        let checkout = std::env::temp_dir().join(format!("lumo-skill-git-{}", unique()));
        let result = (|| {
            let output = Command::new("git")
                .args(["clone", "--quiet", "--", url])
                .arg(&checkout)
                .env("GIT_TERMINAL_PROMPT", "0")
                .output()?;
            if !output.status.success() {
                return Err(SkillManagerError::Git(
                    String::from_utf8_lossy(&output.stderr).into_owned(),
                ));
            }
            if let Some(rev) = revision {
                let output = Command::new("git")
                    .args(["checkout", "--quiet", "--detach", rev])
                    .current_dir(&checkout)
                    .env("GIT_TERMINAL_PROMPT", "0")
                    .output()?;
                if !output.status.success() {
                    return Err(SkillManagerError::Git(
                        String::from_utf8_lossy(&output.stderr).into_owned(),
                    ));
                }
            }
            self.import_local(&checkout)
        })();
        let _ = fs::remove_dir_all(&checkout);
        result
    }

    pub fn list_versions(&self, name: &str) -> Result<Vec<SkillVersion>, SkillManagerError> {
        validate_name(name)?;
        let enabled = self.active(name)?.map(|a| a.enabled).unwrap_or(true);
        let dir = self.root.join(name).join("versions");
        if !dir.exists() {
            return Ok(vec![]);
        }
        let mut out = Vec::new();
        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            if entry.file_type()?.is_dir() {
                let hash = entry.file_name().to_string_lossy().into_owned();
                out.push(SkillVersion {
                    name: name.into(),
                    path: entry.path().join("SKILL.md"),
                    hash,
                    enabled,
                });
            }
        }
        out.sort_by(|a, b| a.hash.cmp(&b.hash));
        Ok(out)
    }

    pub fn active(&self, name: &str) -> Result<Option<SkillVersion>, SkillManagerError> {
        validate_name(name)?;
        let Some(m) = self.read_manifest(name)? else {
            return Ok(None);
        };
        Ok(Some(SkillVersion {
            name: name.into(),
            path: self.version_file(name, &m.hash),
            hash: m.hash,
            enabled: m.enabled,
        }))
    }

    pub fn activate(&self, name: &str, hash: &str) -> Result<SkillVersion, SkillManagerError> {
        validate_name(name)?;
        validate_hash(hash)?;
        if !self.version_file(name, hash).is_file() {
            return Err(SkillManagerError::VersionNotFound(name.into(), hash.into()));
        }
        self.validate(name, hash)?;
        let old = self.read_manifest(name)?;
        let mut history = old.as_ref().map(|m| m.history.clone()).unwrap_or_default();
        if let Some(old) = &old {
            if old.hash != hash {
                history.push(old.hash.clone());
            }
        }
        let manifest = ActiveManifest {
            hash: hash.into(),
            enabled: old.map(|m| m.enabled).unwrap_or(true),
            history,
        };
        self.write_manifest(name, &manifest)?;
        Ok(self.active(name)?.expect("manifest just written"))
    }

    pub fn rollback(&self, name: &str) -> Result<Option<SkillVersion>, SkillManagerError> {
        let mut m = self
            .read_manifest(name)?
            .ok_or_else(|| SkillManagerError::NoPreviousVersion(name.into()))?;
        let Some(hash) = m.history.pop() else {
            return Err(SkillManagerError::NoPreviousVersion(name.into()));
        };
        if !self.version_file(name, &hash).is_file() {
            return Err(SkillManagerError::VersionNotFound(name.into(), hash));
        }
        m.hash = hash;
        self.write_manifest(name, &m)?;
        self.active(name)
    }

    pub fn set_enabled(&self, name: &str, enabled: bool) -> Result<(), SkillManagerError> {
        let mut m = self
            .read_manifest(name)?
            .ok_or_else(|| SkillManagerError::VersionNotFound(name.into(), "active".into()))?;
        m.enabled = enabled;
        self.write_manifest(name, &m)
    }

    pub fn validate(
        &self,
        name: &str,
        hash: &str,
    ) -> Result<SkillValidationReport, SkillManagerError> {
        validate_name(name)?;
        validate_hash(hash)?;
        let path = self.version_file(name, hash);
        if !path.is_file() {
            return Err(SkillManagerError::VersionNotFound(name.into(), hash.into()));
        }
        match lumo_skills::loader::load_skill_file(path) {
            Ok(s) if s.name() == name => Ok(SkillValidationReport {
                valid: true,
                name: Some(name.into()),
                error: None,
            }),
            Ok(s) => Err(SkillManagerError::InvalidSkill(format!(
                "expected name {name}, found {}",
                s.name()
            ))),
            Err(e) => Err(SkillManagerError::InvalidSkill(e.to_string())),
        }
    }

    fn version_file(&self, name: &str, hash: &str) -> PathBuf {
        self.root
            .join(name)
            .join("versions")
            .join(hash)
            .join("SKILL.md")
    }
    fn read_manifest(&self, name: &str) -> Result<Option<ActiveManifest>, SkillManagerError> {
        let p = self.root.join(name).join("active.json");
        if !p.exists() {
            return Ok(None);
        }
        serde_json::from_slice(&fs::read(p)?)
            .map(Some)
            .map_err(|e| SkillManagerError::InvalidSkill(e.to_string()))
    }
    fn write_manifest(&self, name: &str, m: &ActiveManifest) -> Result<(), SkillManagerError> {
        let dir = self.root.join(name);
        fs::create_dir_all(&dir)?;
        let tmp = dir.join(format!(".active-{}.tmp", unique()));
        fs::write(
            &tmp,
            serde_json::to_vec_pretty(m)
                .map_err(|e| SkillManagerError::InvalidSkill(e.to_string()))?,
        )?;
        fs::rename(tmp, dir.join("active.json"))?;
        Ok(())
    }
}

fn validate_name(name: &str) -> Result<(), SkillManagerError> {
    if name.is_empty()
        || name == "."
        || name == ".."
        || !name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
    {
        return Err(SkillManagerError::UnsafeName(name.into()));
    }
    Ok(())
}
fn validate_hash(hash: &str) -> Result<(), SkillManagerError> {
    if hash.len() != 64 || !hash.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(SkillManagerError::UnsafeName(hash.into()));
    }
    Ok(())
}
fn unique() -> String {
    format!(
        "{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    )
}
