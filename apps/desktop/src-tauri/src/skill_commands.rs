use super::{app_home, skills_root, AppHandle};
use lumo_agent::{SkillManager, SkillValidationReport, SkillVersion};
use std::path::PathBuf;
use tauri::command;

fn manager(app: &AppHandle) -> Result<SkillManager, String> {
    SkillManager::new(skills_root(&app_home(app)?)).map_err(|error| error.to_string())
}

fn validate_and_activate(
    manager: &SkillManager,
    imported: SkillVersion,
) -> Result<SkillVersion, String> {
    let report = manager
        .validate(&imported.name, &imported.hash)
        .map_err(|error| error.to_string())?;
    if !report.valid {
        return Err(format!(
            "skill `{}` failed validation: {}",
            imported.name,
            report
                .error
                .unwrap_or_else(|| "unknown validation error".into())
        ));
    }
    manager
        .activate(&imported.name, &imported.hash)
        .map_err(|error| error.to_string())
}

#[command]
pub(super) fn skill_import_local(app: AppHandle, path: String) -> Result<SkillVersion, String> {
    let manager = manager(&app)?;
    let imported = manager
        .import_local(PathBuf::from(path))
        .map_err(|error| error.to_string())?;
    validate_and_activate(&manager, imported)
}

#[command]
pub(super) async fn skill_import_git(
    app: AppHandle,
    url: String,
    revision: Option<String>,
) -> Result<SkillVersion, String> {
    let root = skills_root(&app_home(&app)?);
    tauri::async_runtime::spawn_blocking(move || {
        let manager = SkillManager::new(root).map_err(|error| error.to_string())?;
        let imported = manager
            .import_git(&url, revision.as_deref())
            .map_err(|error| error.to_string())?;
        validate_and_activate(&manager, imported)
    })
    .await
    .map_err(|error| error.to_string())?
}

#[command]
pub(super) fn skill_versions(app: AppHandle, name: String) -> Result<Vec<SkillVersion>, String> {
    manager(&app)?
        .list_versions(&name)
        .map_err(|error| error.to_string())
}

#[command]
pub(super) fn skill_activate(
    app: AppHandle,
    name: String,
    hash: String,
) -> Result<SkillVersion, String> {
    manager(&app)?
        .activate(&name, &hash)
        .map_err(|error| error.to_string())
}

#[command]
pub(super) fn skill_set_enabled(app: AppHandle, name: String, enabled: bool) -> Result<(), String> {
    manager(&app)?
        .set_enabled(&name, enabled)
        .map_err(|error| error.to_string())
}

#[command]
pub(super) fn skill_validate(
    app: AppHandle,
    name: String,
    hash: String,
) -> Result<SkillValidationReport, String> {
    manager(&app)?
        .validate(&name, &hash)
        .map_err(|error| error.to_string())
}

#[command]
pub(super) fn skill_rollback(app: AppHandle, name: String) -> Result<Option<SkillVersion>, String> {
    manager(&app)?
        .rollback(&name)
        .map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn imported_skill_must_validate_before_activation() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source");
        fs::create_dir_all(&source).unwrap();
        fs::write(
            source.join("SKILL.md"),
            "---\nname: demo-skill\nversion: 1.0.0\n---\n```yaml\nsteps: []\n```\n",
        )
        .unwrap();
        let manager = SkillManager::new(temp.path().join("registry")).unwrap();
        let imported = manager.import_local(&source).unwrap();
        fs::write(&imported.path, "broken").unwrap();
        assert!(validate_and_activate(&manager, imported).is_err());
        assert!(manager.active("demo-skill").unwrap().is_none());
    }
}
