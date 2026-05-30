//! Integration coverage for `archive.zip` / `archive.unzip` (S-class F-7).
//! Hermetic: everything runs under a tempdir granted via `fs_caps`.

mod common;
use common::{fs_caps, ok_with, run, run_with};
use serde_json::json;

#[tokio::test]
async fn zip_then_unzip_round_trips_content() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("note.txt");
    std::fs::write(&src, "hello-zip").unwrap();
    let archive = dir.path().join("out.zip");
    let caps = fs_caps(dir.path());

    let zres = ok_with(
        "archive.zip",
        json!({"paths": [src], "dest": archive}),
        caps.clone(),
    )
    .await;
    assert_eq!(zres["entries"], json!(1));

    let out = dir.path().join("unpacked");
    let ures = ok_with("archive.unzip", json!({"src": archive, "dest": out}), caps).await;
    assert_eq!(ures["entries"], json!(1));
    assert_eq!(
        std::fs::read_to_string(out.join("note.txt")).unwrap(),
        "hello-zip"
    );
}

#[tokio::test]
async fn zip_requires_at_least_one_path() {
    let dir = tempfile::tempdir().unwrap();
    let archive = dir.path().join("empty.zip");
    let err = run_with(
        "archive.zip",
        json!({"paths": [], "dest": archive}),
        fs_caps(dir.path()),
    )
    .await
    .unwrap_err();
    assert!(err.contains("at least one path"), "got: {err}");
}

#[tokio::test]
async fn zip_outside_sandbox_is_denied() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("a.txt");
    std::fs::write(&src, "x").unwrap();
    // No caps at all → read of the source is denied.
    let err = run(
        "archive.zip",
        json!({"paths": [src], "dest": dir.path().join("a.zip")}),
    )
    .await
    .unwrap_err();
    assert!(err.contains("capability denied"), "got: {err}");
}
