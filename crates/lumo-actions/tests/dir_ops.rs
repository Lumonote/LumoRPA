//! Integration coverage for the `dir.*` family.
//!
//! Every test works inside a unique temp dir and cleans up after itself.

mod common;
use common::{ok, ok_with, run, run_with};
use lumo_dsl::Capabilities;
use serde_json::json;
use std::path::{Path, PathBuf};

/// A unique scratch directory under the OS temp dir.
fn scratch(tag: &str) -> PathBuf {
    let mut p = std::env::temp_dir();
    p.push(format!(
        "lumo-dir-test-{tag}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&p).unwrap();
    p
}

/// Capabilities granting full read + write, so the scratch dir is reachable
/// regardless of the workspace root.
///
/// Uses the `**` wildcard rather than `/`: macOS resolves /var through the
/// /private/var symlink, and the grant matcher compares lexically-cleaned paths
/// without following symlinks, so a literal prefix grant would miss the temp dir.
fn rw() -> Capabilities {
    Capabilities {
        fs_read: vec!["**".into()],
        fs_write: vec!["**".into()],
        ..Default::default()
    }
}

fn p(v: &Path) -> serde_json::Value {
    json!(v.display().to_string())
}

#[tokio::test]
async fn if_exist_distinguishes_dirs_files_and_missing_paths() {
    let d = scratch("exist");
    let f = d.join("a.txt");
    std::fs::write(&f, b"x").unwrap();

    assert_eq!(
        ok_with("dir.if_exist", json!({ "path": p(&d) }), rw()).await,
        json!(true)
    );
    assert_eq!(
        ok_with("dir.if_exist", json!({ "path": p(&f) }), rw()).await,
        json!(false),
        "a file is not a directory"
    );
    assert_eq!(
        ok_with("dir.if_exist", json!({ "path": p(&d.join("nope")) }), rw()).await,
        json!(false),
        "a missing path is false, not an error"
    );
    std::fs::remove_dir_all(&d).ok();
}

#[tokio::test]
async fn makedir_creates_parents_and_is_idempotent() {
    let d = scratch("mkdir");
    let nested = d.join("a/b/c");
    let out = ok_with("dir.makedir", json!({ "path": p(&nested) }), rw()).await;
    assert_eq!(out["created"], json!(true));
    assert!(nested.is_dir());
    // Running again must not fail (create_dir_all semantics).
    ok_with("dir.makedir", json!({ "path": p(&nested) }), rw()).await;
    std::fs::remove_dir_all(&d).ok();
}

#[tokio::test]
async fn empty_reports_true_for_empty_and_false_after_a_write() {
    let d = scratch("empty");
    assert_eq!(
        ok_with("dir.empty", json!({ "path": p(&d) }), rw()).await,
        json!(true)
    );
    std::fs::write(d.join("f.txt"), b"x").unwrap();
    assert_eq!(
        ok_with("dir.empty", json!({ "path": p(&d) }), rw()).await,
        json!(false)
    );
    // A missing directory counts as empty rather than erroring.
    assert_eq!(
        ok_with("dir.empty", json!({ "path": p(&d.join("gone")) }), rw()).await,
        json!(true)
    );
    std::fs::remove_dir_all(&d).ok();
}

#[tokio::test]
async fn find_files_and_subdirs_separate_the_two_kinds() {
    let d = scratch("find");
    std::fs::create_dir_all(d.join("sub")).unwrap();
    std::fs::write(d.join("a.txt"), b"1").unwrap();
    std::fs::write(d.join("b.log"), b"2").unwrap();
    std::fs::write(d.join("sub/c.txt"), b"3").unwrap();

    let files = ok_with("dir.find_files", json!({ "path": p(&d) }), rw()).await;
    assert_eq!(
        files["count"],
        json!(2),
        "non-recursive sees only the top level"
    );

    let recursive = ok_with(
        "dir.find_files",
        json!({ "path": p(&d), "recursive": true }),
        rw(),
    )
    .await;
    assert_eq!(recursive["count"], json!(3));

    let dirs = ok_with("dir.find_subdirs", json!({ "path": p(&d) }), rw()).await;
    assert_eq!(dirs["count"], json!(1));
    std::fs::remove_dir_all(&d).ok();
}

#[tokio::test]
async fn find_files_honours_the_limit_and_flags_truncation() {
    let d = scratch("limit");
    for i in 0..5 {
        std::fs::write(d.join(format!("f{i}.txt")), b"x").unwrap();
    }
    let out = ok_with("dir.find_files", json!({ "path": p(&d), "limit": 2 }), rw()).await;
    assert_eq!(out["count"], json!(2));
    assert_eq!(
        out["truncated"],
        json!(true),
        "truncation must be visible to the caller"
    );
    std::fs::remove_dir_all(&d).ok();
}

#[tokio::test]
async fn get_special_dir_resolves_known_names_and_rejects_unknown() {
    for name in ["home", "desktop", "downloads", "documents", "temp"] {
        let out = ok("dir.get_special_dir", json!({ "name": name })).await;
        assert_eq!(out["name"], json!(name));
        assert!(out["path"].as_str().unwrap().starts_with('/'), "got {out}");
    }
    // Default is home.
    let dflt = ok("dir.get_special_dir", json!({})).await;
    assert_eq!(dflt["name"], json!("home"));

    let err = run("dir.get_special_dir", json!({ "name": "mars" }))
        .await
        .expect_err("unknown name must fail");
    assert!(err.contains("unknown name"), "got: {err}");
}

#[tokio::test]
async fn copy_move_rename_round_trip_a_tree() {
    let d = scratch("copy");
    let src = d.join("src");
    std::fs::create_dir_all(src.join("inner")).unwrap();
    std::fs::write(src.join("inner/x.txt"), b"hello").unwrap();

    let dst = d.join("dst");
    let out = ok_with("dir.copy", json!({ "path": p(&src), "to": p(&dst) }), rw()).await;
    assert_eq!(out["files"], json!(1));
    assert!(dst.join("inner/x.txt").is_file(), "tree was copied deeply");
    assert_eq!(
        std::fs::read_to_string(dst.join("inner/x.txt")).unwrap(),
        "hello"
    );

    // Copying onto an existing target is refused without `overwrite`.
    let err = run_with("dir.copy", json!({ "path": p(&src), "to": p(&dst) }), rw())
        .await
        .expect_err("existing target must be refused");
    assert!(err.contains("already exists"), "got: {err}");

    let moved = d.join("moved");
    ok_with(
        "dir.move",
        json!({ "path": p(&dst), "to": p(&moved) }),
        rw(),
    )
    .await;
    assert!(moved.is_dir() && !dst.exists());

    let renamed = d.join("renamed");
    ok_with(
        "dir.rename",
        json!({ "path": p(&moved), "to": p(&renamed) }),
        rw(),
    )
    .await;
    assert!(renamed.is_dir());
    std::fs::remove_dir_all(&d).ok();
}

#[tokio::test]
async fn dry_run_reports_without_touching_the_tree() {
    let d = scratch("dry");
    let out = ok_with(
        "dir.copy",
        json!({ "path": p(&d), "to": p(&d.join("new")), "dry_run": true }),
        rw(),
    )
    .await;
    assert_eq!(out["dry_run"], json!(true));
    assert!(!d.join("new").exists(), "dry_run must not create anything");
    std::fs::remove_dir_all(&d).ok();
}

#[tokio::test]
async fn remove_deletes_a_tree_and_respects_missing_ok() {
    let d = scratch("rm");
    let victim = d.join("victim");
    std::fs::create_dir_all(victim.join("deep")).unwrap();
    std::fs::write(victim.join("deep/f.txt"), b"x").unwrap();

    // A non-empty dir requires `recursive`.
    let err = run_with("dir.remove", json!({ "path": p(&victim) }), rw())
        .await
        .expect_err("non-empty dir must need recursive");
    assert!(!err.is_empty());

    let out = ok_with(
        "dir.remove",
        json!({ "path": p(&victim), "recursive": true }),
        rw(),
    )
    .await;
    assert_eq!(out["deleted"], json!(true));
    assert!(!victim.exists());

    // Missing + missing_ok is a clean no-op; missing without it is an error.
    let gone = ok_with(
        "dir.remove",
        json!({ "path": p(&victim), "missing_ok": true }),
        rw(),
    )
    .await;
    assert_eq!(gone["deleted"], json!(false));
    run_with("dir.remove", json!({ "path": p(&victim) }), rw())
        .await
        .expect_err("missing path without missing_ok must fail");
    std::fs::remove_dir_all(&d).ok();
}

#[tokio::test]
async fn remove_refuses_a_regular_file() {
    let d = scratch("rmfile");
    let f = d.join("f.txt");
    std::fs::write(&f, b"x").unwrap();
    let err = run_with("dir.remove", json!({ "path": p(&f) }), rw())
        .await
        .expect_err("dir.remove is for directories");
    assert!(err.contains("not a directory"), "got: {err}");
    std::fs::remove_dir_all(&d).ok();
}

#[tokio::test]
async fn fs_capabilities_are_actually_enforced() {
    let d = scratch("caps");
    // No fs.read grant at all -> the read-side actions must be denied.
    let err = run("dir.find_files", json!({ "path": p(&d) }))
        .await
        .expect_err("without fs.read the action must be denied");
    assert!(
        !err.is_empty(),
        "expected a capability denial, got an empty error"
    );
    std::fs::remove_dir_all(&d).ok();
}

#[tokio::test]
async fn host_interaction_actions_fail_cleanly_without_a_prompter() {
    // No host prompter is attached in this harness: both must return a readable
    // error rather than hanging or panicking.
    for (id, input) in [
        ("dir.selected_dirs_or_files", json!({})),
        ("dir.open", json!({ "path": "/tmp" })),
    ] {
        // rw() so the fs.read gate on dir.open passes and we reach the host
        // call itself — that is the behaviour under test here.
        let err = run_with(id, input, rw())
            .await
            .expect_err("host interaction must not silently succeed headless");
        assert!(
            err.contains("host does not support human interaction"),
            "`{id}` should report the missing host, got: {err}"
        );
    }
}
