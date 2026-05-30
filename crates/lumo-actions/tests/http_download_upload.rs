//! Integration coverage for `http.download` / `http.upload` and the
//! `http.request` size cap (S-class F-11). Hermetic via `wiremock` + tempdir.

#![allow(dead_code)] // `net` is shared with the Task 4 upload tests added later

mod common;
use common::{run, run_with, Capabilities};
use serde_json::json;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn net(host: &str) -> Capabilities {
    Capabilities {
        network: vec![host.to_string()],
        ..Default::default()
    }
}

/// Grant a tempdir for writes/reads AND localhost for the network.
fn net_fs(host: &str, dir: &std::path::Path) -> Capabilities {
    let glob = format!("{}/**", dir.display());
    Capabilities {
        network: vec![host.to_string()],
        fs_read: vec![glob.clone()],
        fs_write: vec![glob],
        ..Default::default()
    }
}

#[tokio::test]
async fn download_writes_file_and_reports_bytes() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/file"))
        .respond_with(ResponseTemplate::new(200).set_body_string("hello-dl"))
        .mount(&server)
        .await;

    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("got.bin");
    let out = common::ok_with(
        "http.download",
        json!({"url": format!("{}/file", server.uri()), "dest": dest}),
        net_fs("127.0.0.1", dir.path()),
    )
    .await;
    assert_eq!(out["status"], json!(200));
    assert_eq!(out["bytes"], json!(8));
    assert_eq!(std::fs::read_to_string(&dest).unwrap(), "hello-dl");
}

#[tokio::test]
async fn download_rejects_oversize_and_leaves_no_file() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/big"))
        .respond_with(ResponseTemplate::new(200).set_body_string("x".repeat(100)))
        .mount(&server)
        .await;

    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("toobig.bin");
    let err = run_with(
        "http.download",
        json!({"url": format!("{}/big", server.uri()), "dest": dest, "max_bytes": 10}),
        net_fs("127.0.0.1", dir.path()),
    )
    .await
    .unwrap_err();
    assert!(err.contains("max_bytes"), "got: {err}");
    assert!(
        !dest.exists(),
        "rejected download must not leave a partial file"
    );
}

#[tokio::test]
async fn download_denied_without_network_grant() {
    let dir = tempfile::tempdir().unwrap();
    let err = run(
        "http.download",
        json!({"url": "https://example.com/x", "dest": dir.path().join("x")}),
    )
    .await
    .unwrap_err();
    assert!(err.contains("capability denied"), "got: {err}");
}
