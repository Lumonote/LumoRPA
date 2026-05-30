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

#[tokio::test]
async fn download_blocks_redirect_to_ungranted_host() {
    // SSRF guard: a granted host 302s to an ungranted internal target (cloud
    // metadata). The redirect policy must error BEFORE connecting to 169.254,
    // so this stays hermetic — and no partial file may be left behind.
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/redir"))
        .respond_with(
            ResponseTemplate::new(302)
                .insert_header("Location", "http://169.254.169.254/latest/meta-data/"),
        )
        .mount(&server)
        .await;

    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("ssrf.bin");
    let err = run_with(
        "http.download",
        json!({"url": format!("{}/redir", server.uri()), "dest": dest}),
        net_fs("127.0.0.1", dir.path()),
    )
    .await
    .unwrap_err();
    assert!(
        err.contains("redirect") || err.contains("network capability"),
        "got: {err}"
    );
    assert!(
        !dest.exists(),
        "a blocked redirect must not leave a partial file"
    );
}

#[tokio::test]
async fn download_follows_redirect_to_granted_host() {
    // Legit same/granted-host redirects must still work end to end.
    let server = MockServer::start().await;
    let to = format!("{}/b", server.uri());
    Mock::given(method("GET"))
        .and(path("/a"))
        .respond_with(ResponseTemplate::new(302).insert_header("Location", to.as_str()))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/b"))
        .respond_with(ResponseTemplate::new(200).set_body_string("redirected-ok"))
        .mount(&server)
        .await;

    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("redir.bin");
    let out = common::ok_with(
        "http.download",
        json!({"url": format!("{}/a", server.uri()), "dest": dest}),
        net_fs("127.0.0.1", dir.path()),
    )
    .await;
    assert_eq!(out["status"], json!(200));
    assert_eq!(std::fs::read_to_string(&dest).unwrap(), "redirected-ok");
}

#[tokio::test]
async fn download_streaming_guard_trips_without_content_length() {
    // wiremock always sets Content-Length, so the per-chunk streaming guard
    // (and its remove_file cleanup) is only exercised by a response that
    // declares NO length. A raw TCP server writes the body then closes the
    // socket: reqwest reads to EOF, `content_length()` is None, so the
    // pre-check is skipped and the streaming guard is the only thing that can
    // stop an oversize body.
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        if let Ok((mut sock, _)) = listener.accept().await {
            // Drain the request line/headers (best effort; we don't parse them).
            let mut buf = [0u8; 1024];
            let _ = sock.read(&mut buf).await;
            let body = "x".repeat(100);
            let resp = format!("HTTP/1.1 200 OK\r\nConnection: close\r\n\r\n{body}");
            let _ = sock.write_all(resp.as_bytes()).await;
            let _ = sock.flush().await;
            // Drop `sock` → EOF, signalling the end of the body.
        }
    });

    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("nolen.bin");
    let err = run_with(
        "http.download",
        json!({"url": format!("http://127.0.0.1:{port}/"), "dest": dest, "max_bytes": 10}),
        net_fs("127.0.0.1", dir.path()),
    )
    .await
    .unwrap_err();
    assert!(err.contains("max_bytes"), "got: {err}");
    assert!(
        !dest.exists(),
        "streaming guard must delete the partial file on overflow"
    );
}
