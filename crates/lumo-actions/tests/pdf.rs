//! Integration coverage for the `pdf.*` action family.
//!
//! PDF is a LOCAL family (no network) so these are *real* behavioral tests:
//! write a known text PDF, extract it back, assert the words round-trip, and
//! that `pdf.info` reports a page. All paths live under a tempdir granted via
//! an explicit fs sandbox; gating + validation are exercised too.

mod common;
use common::{fs_caps, ok_with, run, run_with};
use serde_json::json;

#[tokio::test]
async fn write_then_extract_round_trips_text() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("doc.pdf");
    let caps = fs_caps(dir.path());

    // 写入已知文本（多行，用 lines 数组）。
    let out = ok_with(
        "pdf.write",
        json!({ "path": path, "lines": ["Hello LumoRPA", "PDF round trip"] }),
        caps.clone(),
    )
    .await;
    assert_eq!(out.get("path").and_then(|v| v.as_str()), path.to_str());
    assert!(path.exists(), "pdf.write should create the file");

    // 抽取回来：允许抽取器对空白做处理，只断言包含关键词。
    let extracted = ok_with("pdf.extract_text", json!({ "path": path }), caps.clone()).await;
    let text = extracted
        .get("text")
        .and_then(|v| v.as_str())
        .expect("text field");
    assert!(text.contains("Hello"), "missing 'Hello' in: {text:?}");
    assert!(text.contains("LumoRPA"), "missing 'LumoRPA' in: {text:?}");
    assert!(text.contains("PDF"), "missing 'PDF' in: {text:?}");
    assert!(
        text.contains("round") && text.contains("trip"),
        "missing 'round trip' in: {text:?}"
    );
    assert_eq!(
        extracted.get("pages").and_then(|v| v.as_u64()),
        Some(1),
        "single-page document"
    );
}

#[tokio::test]
async fn info_reports_at_least_one_page() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("doc.pdf");
    let caps = fs_caps(dir.path());

    ok_with(
        "pdf.write",
        json!({ "path": path, "text": "single page" }),
        caps.clone(),
    )
    .await;

    let info = ok_with("pdf.info", json!({ "path": path }), caps).await;
    let pages = info.get("pages").and_then(|v| v.as_u64()).expect("pages");
    assert!(pages >= 1, "expected >=1 page, got {pages}");
    assert!(
        info.get("version").and_then(|v| v.as_str()).is_some(),
        "version metadata present"
    );
}

#[tokio::test]
async fn write_from_plain_text_splits_lines() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("multi.pdf");
    let caps = fs_caps(dir.path());

    ok_with(
        "pdf.write",
        json!({ "path": path, "text": "line one\nline two\nline three" }),
        caps.clone(),
    )
    .await;
    let text = ok_with("pdf.extract_text", json!({ "path": path }), caps).await;
    let body = text.get("text").and_then(|v| v.as_str()).unwrap();
    assert!(body.contains("line one") && body.contains("line three"), "got: {body:?}");
}

#[tokio::test]
async fn extract_text_denies_ungranted_path() {
    // No capabilities granted → fs.read gate must reject before touching disk.
    let err = run("pdf.extract_text", json!({ "path": "/nope/secret.pdf" }))
        .await
        .unwrap_err();
    assert!(err.contains("capability denied"), "got: {err}");
    assert!(err.contains("fs.read"), "got: {err}");
}

#[tokio::test]
async fn write_denies_ungranted_path() {
    let err = run("pdf.write", json!({ "path": "/nope/out.pdf", "text": "x" }))
        .await
        .unwrap_err();
    assert!(err.contains("capability denied"), "got: {err}");
    assert!(err.contains("fs.write"), "got: {err}");
}

#[tokio::test]
async fn write_rejects_missing_path() {
    let dir = tempfile::tempdir().unwrap();
    let caps = fs_caps(dir.path());
    let err = run_with("pdf.write", json!({ "text": "x" }), caps)
        .await
        .unwrap_err();
    assert!(err.contains("input invalid"), "got: {err}");
}

#[tokio::test]
async fn extract_text_rejects_missing_path() {
    let err = run("pdf.extract_text", json!({}))
        .await
        .unwrap_err();
    assert!(err.contains("input invalid"), "got: {err}");
}

#[tokio::test]
async fn write_rejects_no_content() {
    // Neither `text` nor `lines` → explicit error after the gate passes.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("empty.pdf");
    let caps = fs_caps(dir.path());
    let err = run_with("pdf.write", json!({ "path": path }), caps)
        .await
        .unwrap_err();
    assert!(err.contains("requires `text` or `lines`"), "got: {err}");
}
