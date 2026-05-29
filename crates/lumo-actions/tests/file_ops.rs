//! Integration coverage for the `file.*` action family (P1-8).
//! All paths live under a tempdir granted via an explicit fs sandbox.

mod common;
use common::{fs_caps, ok_with, run};
use serde_json::json;

#[tokio::test]
async fn write_then_read_round_trips_content() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("note.txt");
    let caps = fs_caps(dir.path());

    ok_with("file.write", json!({"path": path, "content": "hello"}), caps.clone()).await;
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

    ok_with("file.write", json!({"path": path, "content": "a"}), caps.clone()).await;
    ok_with("file.write", json!({"path": path, "content": "b", "append": true}), caps.clone()).await;
    assert_eq!(ok_with("file.read", json!({"path": path}), caps).await, json!("ab"));
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
    ok_with("file.write", json!({"path": path, "content": "x"}), caps.clone()).await;
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
