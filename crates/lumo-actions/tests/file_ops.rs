//! Integration coverage for the `file.*` action family (P1-8).
//! All paths live under a tempdir granted via an explicit fs sandbox.

mod common;
use common::{fs_caps, ok_with, run, run_with, Capabilities};
use serde_json::json;

#[tokio::test]
async fn write_then_read_round_trips_content() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("note.txt");
    let caps = fs_caps(dir.path());

    ok_with(
        "file.write",
        json!({"path": path, "content": "hello"}),
        caps.clone(),
    )
    .await;
    assert_eq!(
        ok_with("file.read", json!({"path": path}), caps).await,
        json!("hello")
    );
}

#[tokio::test]
async fn write_append_extends_the_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("log.txt");
    let caps = fs_caps(dir.path());

    ok_with(
        "file.write",
        json!({"path": path, "content": "a"}),
        caps.clone(),
    )
    .await;
    ok_with(
        "file.write",
        json!({"path": path, "content": "b", "append": true}),
        caps.clone(),
    )
    .await;
    assert_eq!(
        ok_with("file.read", json!({"path": path}), caps).await,
        json!("ab")
    );
}

#[tokio::test]
async fn exists_reflects_presence() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("maybe.txt");
    let caps = fs_caps(dir.path());

    assert_eq!(
        ok_with("file.exists", json!({"path": path}), caps.clone()).await,
        json!(false),
        "absent before writing"
    );
    ok_with(
        "file.write",
        json!({"path": path, "content": "x"}),
        caps.clone(),
    )
    .await;
    assert_eq!(
        ok_with("file.exists", json!({"path": path}), caps).await,
        json!(true),
        "present after writing"
    );
}

#[tokio::test]
async fn read_outside_the_sandbox_is_denied() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("secret.txt");
    // Write it (with a grant) but then try to read with no grant at all.
    let caps = fs_caps(dir.path());
    ok_with("file.write", json!({"path": path, "content": "x"}), caps).await;

    let err = run("file.read", json!({"path": path})).await.unwrap_err();
    assert!(err.contains("capability denied"), "got: {err}");
}

#[tokio::test]
async fn write_outside_the_sandbox_is_denied() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("blocked.txt");
    let err = run("file.write", json!({"path": path, "content": "x"}))
        .await
        .unwrap_err();
    assert!(err.contains("capability denied"), "got: {err}");
}

#[tokio::test]
async fn management_actions_cover_common_file_workflows() {
    let dir = tempfile::tempdir().unwrap();
    let caps = fs_caps(dir.path());
    let nested = dir.path().join("nested");
    let source = nested.join("source.txt");
    let copied = dir.path().join("copied.txt");
    let moved = dir.path().join("moved.txt");

    ok_with("file.mkdir", json!({"path": nested.clone()}), caps.clone()).await;
    ok_with(
        "file.write",
        json!({"path": source.clone(), "content": "abc"}),
        caps.clone(),
    )
    .await;

    let meta = ok_with(
        "file.metadata",
        json!({"path": source.clone()}),
        caps.clone(),
    )
    .await;
    assert_eq!(meta["type"], json!("file"));
    assert_eq!(meta["len"], json!(3));

    let listing = ok_with(
        "file.list",
        json!({"path": dir.path().to_path_buf(), "recursive": true}),
        caps.clone(),
    )
    .await;
    let entries = listing["entries"].as_array().expect("entries array");
    assert!(
        entries
            .iter()
            .any(|entry| entry["name"] == json!("source.txt")),
        "listing should include nested source file: {listing}"
    );

    let copy_out = ok_with(
        "file.copy",
        json!({"from": source.clone(), "to": copied.clone()}),
        caps.clone(),
    )
    .await;
    assert_eq!(copy_out["bytes"], json!(3));
    assert_eq!(
        ok_with("file.read", json!({"path": copied.clone()}), caps.clone()).await,
        json!("abc")
    );

    ok_with(
        "file.move",
        json!({"from": copied.clone(), "to": moved.clone()}),
        caps.clone(),
    )
    .await;
    assert_eq!(
        ok_with("file.exists", json!({"path": copied.clone()}), caps.clone()).await,
        json!(false)
    );
    assert_eq!(
        ok_with("file.exists", json!({"path": moved.clone()}), caps.clone()).await,
        json!(true)
    );

    ok_with("file.delete", json!({"path": moved.clone()}), caps.clone()).await;
    assert_eq!(
        ok_with("file.exists", json!({"path": moved.clone()}), caps.clone()).await,
        json!(false)
    );

    ok_with(
        "file.delete",
        json!({"path": nested.clone(), "recursive": true}),
        caps.clone(),
    )
    .await;
    assert_eq!(
        ok_with("file.exists", json!({"path": nested.clone()}), caps).await,
        json!(false)
    );
}

#[tokio::test]
async fn list_limit_reports_truncation_and_rejects_zero() {
    let dir = tempfile::tempdir().unwrap();
    let caps = fs_caps(dir.path());
    for name in ["a.txt", "b.txt", "c.txt"] {
        std::fs::write(dir.path().join(name), name).unwrap();
    }

    let out = ok_with(
        "file.list",
        json!({"path": dir.path(), "limit": 2}),
        caps.clone(),
    )
    .await;
    assert_eq!(out["count"], json!(2));
    assert_eq!(out["truncated"], json!(true));
    assert_eq!(out["entries"].as_array().unwrap().len(), 2);

    let err = run_with("file.list", json!({"path": dir.path(), "limit": 0}), caps)
        .await
        .unwrap_err();
    assert!(err.contains("limit") && err.contains(">= 1"), "got: {err}");
}

#[tokio::test]
async fn delete_dry_run_previews_without_mutating() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("keep.txt");
    std::fs::write(&path, "keep").unwrap();
    let out = ok_with(
        "file.delete",
        json!({"path": path, "dry_run": true}),
        fs_caps(dir.path()),
    )
    .await;
    assert_eq!(out["dry_run"], json!(true));
    assert_eq!(out["would_delete"], json!(true));
    assert_eq!(out["deleted"], json!(false));
    assert!(path.exists(), "dry_run must not remove the file");
}

#[tokio::test]
async fn recursive_delete_requires_write_grants_for_children() {
    let dir = tempfile::tempdir().unwrap();
    let child = dir.path().join("child.txt");
    std::fs::write(&child, "secret").unwrap();

    let caps = Capabilities {
        fs_write: vec![dir.path().display().to_string()],
        ..Default::default()
    };
    let err = run_with(
        "file.delete",
        json!({"path": dir.path().to_path_buf(), "recursive": true}),
        caps,
    )
    .await
    .unwrap_err();
    assert!(err.contains("capability denied"), "got: {err}");
    assert!(child.exists(), "child should not be deleted after denial");
}

#[tokio::test]
async fn overwrite_destination_directory_requires_write_grants_for_children() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("source.txt");
    let dest = dir.path().join("dest");
    let dest_child = dest.join("child.txt");
    std::fs::write(&source, "new").unwrap();
    std::fs::create_dir(&dest).unwrap();
    std::fs::write(&dest_child, "old").unwrap();

    let caps = Capabilities {
        fs_read: vec![source.display().to_string()],
        fs_write: vec![source.display().to_string(), dest.display().to_string()],
        ..Default::default()
    };
    let err = run_with(
        "file.move",
        json!({"from": source, "to": dest, "overwrite": true}),
        caps,
    )
    .await
    .unwrap_err();
    assert!(err.contains("capability denied"), "got: {err}");
    assert!(dest_child.exists(), "destination child should remain");
}

#[tokio::test]
async fn copy_refuses_destination_directory_even_with_overwrite() {
    let dir = tempfile::tempdir().unwrap();
    let caps = fs_caps(dir.path());
    let source = dir.path().join("source.txt");
    let dest = dir.path().join("dest");
    let dest_child = dest.join("child.txt");
    std::fs::write(&source, "new").unwrap();
    std::fs::create_dir(&dest).unwrap();
    std::fs::write(&dest_child, "old").unwrap();

    let err = run_with(
        "file.copy",
        json!({"from": source, "to": dest, "overwrite": true}),
        caps,
    )
    .await
    .unwrap_err();
    assert!(err.contains("destination is a directory"), "got: {err}");
    assert!(dest_child.exists(), "destination directory should remain");
}

#[tokio::test]
async fn list_filters_hidden_kind_pattern_and_sort_order() {
    let dir = tempfile::tempdir().unwrap();
    let caps = fs_caps(dir.path());
    let alpha = dir.path().join("alpha.txt");
    let beta = dir.path().join("beta.log");
    let hidden = dir.path().join(".hidden.txt");
    let subdir = dir.path().join("subdir");
    std::fs::write(&alpha, "a").unwrap();
    std::fs::write(&beta, "bbbb").unwrap();
    std::fs::write(&hidden, "h").unwrap();
    std::fs::create_dir(&subdir).unwrap();

    let listing = ok_with(
        "file.list",
        json!({
            "path": dir.path().to_path_buf(),
            "kind": "files",
            "pattern": "*.txt",
            "sort_by": "name",
            "descending": true
        }),
        caps.clone(),
    )
    .await;
    let names: Vec<_> = listing["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| entry["name"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(names, vec!["alpha.txt"], "hidden files should be excluded");

    let hidden_listing = ok_with(
        "file.list",
        json!({
            "path": dir.path().to_path_buf(),
            "kind": "files",
            "pattern": "*.txt",
            "include_hidden": true,
            "sort_by": "name",
            "descending": true
        }),
        caps,
    )
    .await;
    let names: Vec<_> = hidden_listing["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| entry["name"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(names, vec!["alpha.txt", ".hidden.txt"]);
}

#[tokio::test]
async fn rename_changes_name_within_parent_and_rejects_path_names() {
    let dir = tempfile::tempdir().unwrap();
    let caps = fs_caps(dir.path());
    let source = dir.path().join("old.txt");
    let renamed = dir.path().join("new.txt");
    std::fs::write(&source, "body").unwrap();

    ok_with(
        "file.rename",
        json!({"path": source.clone(), "new_name": "new.txt"}),
        caps.clone(),
    )
    .await;
    assert!(!source.exists());
    assert_eq!(std::fs::read_to_string(&renamed).unwrap(), "body");

    let err = run_with(
        "file.rename",
        json!({"path": renamed, "new_name": "../escape.txt"}),
        caps,
    )
    .await
    .unwrap_err();
    assert!(err.contains("single file name"), "got: {err}");
}

#[tokio::test]
async fn append_creates_then_extends_the_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("app.log");
    let caps = fs_caps(dir.path());

    // First append creates the file (no prior file.write needed).
    ok_with(
        "file.append",
        json!({"path": path, "content": "line1", "newline": true}),
        caps.clone(),
    )
    .await;
    ok_with(
        "file.append",
        json!({"path": path, "content": "line2", "newline": true}),
        caps.clone(),
    )
    .await;
    assert_eq!(
        ok_with("file.read", json!({"path": path}), caps).await,
        json!("line1\nline2\n")
    );
}

#[tokio::test]
async fn append_without_newline_concatenates() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("plain.txt");
    let caps = fs_caps(dir.path());

    ok_with(
        "file.append",
        json!({"path": path, "content": "a"}),
        caps.clone(),
    )
    .await;
    ok_with(
        "file.append",
        json!({"path": path, "content": "b"}),
        caps.clone(),
    )
    .await;
    assert_eq!(
        ok_with("file.read", json!({"path": path}), caps).await,
        json!("ab")
    );
}

#[tokio::test]
async fn append_denied_without_fs_write_grant() {
    let err = run(
        "file.append",
        json!({"path": "/etc/lumo_should_not_write", "content": "x"}),
    )
    .await
    .unwrap_err();
    assert!(err.contains("capability denied"), "got: {err}");
    assert!(err.contains("fs.write"), "should name fs.write: {err}");
}

// ─── typed error classification (retry.on contract) ─────────────────────────

#[tokio::test]
async fn read_missing_file_classifies_as_io() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("absent.txt");
    let kind = common::err_kind_with("file.read", json!({"path": path}), fs_caps(dir.path())).await;
    assert_eq!(kind, lumo_core::error::ErrorKind::Io);
}

#[tokio::test]
async fn mkdir_without_parent_classifies_as_io() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("a/b/c");
    let kind = common::err_kind_with(
        "file.mkdir",
        json!({"path": path, "recursive": false}),
        fs_caps(dir.path()),
    )
    .await;
    assert_eq!(kind, lumo_core::error::ErrorKind::Io);
}

#[tokio::test]
async fn wait_timeout_classifies_as_timeout() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("never.txt");
    let kind = common::err_kind_with(
        "file.wait",
        json!({"path": path, "timeout_ms": 50, "poll_ms": 10}),
        fs_caps(dir.path()),
    )
    .await;
    assert_eq!(kind, lumo_core::error::ErrorKind::Timeout);
}

#[tokio::test]
async fn input_validation_stays_kind_other() {
    let dir = tempfile::tempdir().unwrap();
    let kind = common::err_kind_with(
        "file.rename",
        json!({"path": dir.path().join("x.txt"), "new_name": "a/b"}),
        fs_caps(dir.path()),
    )
    .await;
    assert_eq!(kind, lumo_core::error::ErrorKind::Other);
}
