use lumo_agent::{SkillManager, SkillManagerError};
use std::{fs, path::Path};

fn skill(name: &str, note: &str) -> String {
    format!("---\nname: {name}\nversion: 1.0.0\n---\n{note}\n```yaml\nsteps: []\n```\n")
}

fn write_skill(dir: &Path, name: &str, note: &str) {
    fs::create_dir_all(dir).unwrap();
    fs::write(dir.join("SKILL.md"), skill(name, note)).unwrap();
}

#[test]
fn versions_activate_update_and_rollback() {
    let t = tempfile::tempdir().unwrap();
    let src = t.path().join("source");
    let manager = SkillManager::new(t.path().join("installed")).unwrap();
    write_skill(&src, "demo", "one");
    let one = manager.import_local(&src).unwrap();
    assert_eq!(manager.active("demo").unwrap(), None);
    manager.activate("demo", &one.hash).unwrap();
    write_skill(&src, "demo", "two");
    let two = manager.import_local(&src).unwrap();
    manager.activate("demo", &two.hash).unwrap();
    assert_eq!(manager.list_versions("demo").unwrap().len(), 2);
    assert_eq!(manager.rollback("demo").unwrap().unwrap().hash, one.hash);
    assert_eq!(manager.active("demo").unwrap().unwrap().hash, one.hash);
}

#[test]
fn enable_validate_idempotency_and_invalid_preserves_manifest() {
    let t = tempfile::tempdir().unwrap();
    let src = t.path().join("source");
    let manager = SkillManager::new(t.path().join("installed")).unwrap();
    write_skill(&src, "demo", "one");
    let v = manager.import_local(src.join("SKILL.md")).unwrap();
    assert_eq!(v.hash, manager.import_local(&src).unwrap().hash);
    manager.activate("demo", &v.hash).unwrap();
    manager.set_enabled("demo", false).unwrap();
    assert!(!manager.active("demo").unwrap().unwrap().enabled);
    assert!(manager.validate("demo", &v.hash).unwrap().valid);
    let manifest = fs::read(manager.root().join("demo/active.json")).unwrap();
    assert!(manager.activate("demo", "missing").is_err());
    assert_eq!(
        fs::read(manager.root().join("demo/active.json")).unwrap(),
        manifest
    );
    fs::write(src.join("SKILL.md"), "bad").unwrap();
    assert!(matches!(
        manager.import_local(&src),
        Err(SkillManagerError::InvalidSkill(_))
    ));
}

#[test]
fn imports_from_local_git_revision() {
    let t = tempfile::tempdir().unwrap();
    let repo = t.path().join("repo");
    write_skill(&repo, "git-demo", "git");
    for args in [
        ["init"].as_slice(),
        ["add", "."].as_slice(),
        [
            "-c",
            "user.name=T",
            "-c",
            "user.email=t@e",
            "commit",
            "-m",
            "init",
        ]
        .as_slice(),
    ] {
        assert!(std::process::Command::new("git")
            .args(args)
            .current_dir(&repo)
            .status()
            .unwrap()
            .success());
    }
    let rev = String::from_utf8(
        std::process::Command::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(&repo)
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap();
    let manager = SkillManager::new(t.path().join("installed")).unwrap();
    let v = manager
        .import_git(repo.to_str().unwrap(), Some(rev.trim()))
        .unwrap();
    assert_eq!(v.name, "git-demo");
}

#[test]
fn rejects_unsafe_names_paths_and_symlinks() {
    let t = tempfile::tempdir().unwrap();
    let manager = SkillManager::new(t.path().join("installed")).unwrap();
    let src = t.path().join("source");
    write_skill(&src, "../escape", "bad");
    assert!(matches!(
        manager.import_local(&src),
        Err(SkillManagerError::UnsafeName(_))
    ));
    assert!(manager.import_local(t.path().join("missing")).is_err());
    #[cfg(unix)]
    {
        let real = t.path().join("real.md");
        fs::write(&real, skill("linked", "bad")).unwrap();
        std::os::unix::fs::symlink(&real, t.path().join("link.md")).unwrap();
        assert!(matches!(
            manager.import_local(t.path().join("link.md")),
            Err(SkillManagerError::UnsafePath(_))
        ));
    }
}
