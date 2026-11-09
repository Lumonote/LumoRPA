use std::path::{Path, PathBuf};

pub(crate) fn duplicate_flow_file(src: &Path, dir: &Path) -> Result<PathBuf, String> {
    let bytes = std::fs::read(src).map_err(|e| format!("read {}: {e}", src.display()))?;
    let stem = src
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("flow")
        .trim_end_matches(".lumoflow");
    let mut candidate = dir.join(format!("{stem}-copy.lumoflow.yaml"));
    let mut n = 2;
    while candidate.exists() {
        candidate = dir.join(format!("{stem}-copy-{n}.lumoflow.yaml"));
        n += 1;
    }
    if let Some(parent) = src.parent() {
        let skills = parent.join("skills");
        let target = dir.join("skills");
        if skills.is_dir() && skills.canonicalize().ok() != target.canonicalize().ok() {
            copy_skill_tree(&skills, &target)?;
        }
    }
    std::fs::write(&candidate, bytes).map_err(|e| format!("write {}: {e}", candidate.display()))?;
    Ok(candidate)
}

// Skills may carry files used by nested flow.call or document actions. Preserve
// the whole local skill tree, while keeping every existing user file intact.
fn copy_skill_tree(src: &Path, dest: &Path) -> Result<(), String> {
    if std::fs::symlink_metadata(src)
        .map_err(|e| e.to_string())?
        .file_type()
        .is_symlink()
        || std::fs::symlink_metadata(dest).is_ok_and(|m| m.file_type().is_symlink())
    {
        return Err(format!(
            "cannot copy skill through a symlink: {}",
            dest.display()
        ));
    }
    std::fs::create_dir_all(dest).map_err(|e| format!("create {}: {e}", dest.display()))?;
    for entry in std::fs::read_dir(src).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        let source = entry.path();
        let target = dest.join(entry.file_name());
        let kind = entry.file_type().map_err(|e| e.to_string())?;
        if kind.is_symlink() {
            return Err(format!("cannot copy skill symlink: {}", source.display()));
        }
        if kind.is_dir() {
            copy_skill_tree(&source, &target)?;
        } else if kind.is_file() {
            let bytes = std::fs::read(&source).map_err(|e| e.to_string())?;
            if target.exists() {
                if std::fs::read(&target).map_err(|e| e.to_string())? == bytes {
                    continue;
                }
                return Err(format!(
                    "local skill already exists with different content: {}",
                    target.display()
                ));
            }
            use std::io::Write;
            let mut file = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&target)
                .map_err(|e| format!("create {}: {e}", target.display()))?;
            file.write_all(&bytes).map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn copied_skill_driver_keeps_its_local_skill_dependencies() {
        let home = tempfile::tempdir().unwrap();
        let dir = home.path().join("flows");
        std::fs::create_dir(&dir).unwrap();
        let example = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../examples/skill-driver.lumoflow.yaml");
        let copy = duplicate_flow_file(&example, &dir).unwrap();
        assert!(dir.join("skills/greet/SKILL.md").is_file());
        let result = crate::execution::execute_flow(
            home.path(),
            Some(&copy),
            lumo_dsl::parse_file(&copy).unwrap(),
            serde_json::json!({"who":"copied-editor"}),
            false,
            crate::execution::DebugOpts::default(),
            &crate::CancelMap::default(),
            None,
            None,
        )
        .await
        .unwrap();
        assert!(result.report.success);
        assert_eq!(
            result
                .report
                .outputs
                .unwrap()
                .pointer("/invoke_greet/result/outputs/say/result/message"),
            Some(&serde_json::json!("hello, copied-editor!"))
        );
    }

    #[test]
    fn copying_skills_never_overwrites_an_existing_user_skill() {
        let temp = tempfile::tempdir().unwrap();
        let src = temp.path().join("source");
        let dest = temp.path().join("dest");
        for root in [&src, &dest] {
            std::fs::create_dir_all(root.join("skills/greet")).unwrap();
        }
        let flow = src.join("flow.lumoflow.yaml");
        std::fs::write(&flow, "flow").unwrap();
        std::fs::write(src.join("skills/greet/SKILL.md"), "example skill").unwrap();
        let user_skill = dest.join("skills/greet/SKILL.md");
        std::fs::write(&user_skill, "user changes").unwrap();
        let error = duplicate_flow_file(&flow, &dest).unwrap_err();
        assert!(error.contains("already exists"), "{error}");
        assert_eq!(std::fs::read_to_string(user_skill).unwrap(), "user changes");
        assert!(!dest.join("flow-copy.lumoflow.yaml").exists());
    }
}
