//! Integration coverage for `http.request` (P1-8). A local `wiremock` server
//! stands in for the network so the happy path is hermetic; the deny path needs
//! no server at all.

mod common;
use common::{ok_with, run, Capabilities};
use serde_json::json;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn net(host: &str) -> Capabilities {
    Capabilities {
        network: vec![host.to_string()],
        ..Default::default()
    }
}

#[tokio::test]
async fn request_returns_status_and_parsed_json() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/hello"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("x-trace", "abc")
                .set_body_json(json!({"ok": true})),
        )
        .mount(&server)
        .await;

    let out = ok_with(
        "http.request",
        json!({"url": format!("{}/hello", server.uri())}),
        net("127.0.0.1"),
    )
    .await;
    assert_eq!(out["status"], json!(200));
    assert_eq!(
        out["json"],
        json!({"ok": true}),
        "JSON bodies are parsed into `json`"
    );
    assert_eq!(out["headers"]["x-trace"], json!("abc"));
}

#[tokio::test]
async fn request_exposes_raw_text_when_body_is_not_json() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/plain"))
        .respond_with(ResponseTemplate::new(200).set_body_string("just text"))
        .mount(&server)
        .await;

    let out = ok_with(
        "http.request",
        json!({"url": format!("{}/plain", server.uri())}),
        net("127.0.0.1"),
    )
    .await;
    assert_eq!(out["text"], json!("just text"));
    assert_eq!(
        out["json"],
        json!(null),
        "non-JSON bodies leave `json` null"
    );
}

#[tokio::test]
async fn request_is_denied_without_a_network_grant() {
    // No grant → rejected before any socket is opened, so this stays offline.
    let err = run("http.request", json!({"url": "https://example.com/"}))
        .await
        .unwrap_err();
    assert!(err.contains("capability denied"), "got: {err}");
}

#[tokio::test]
async fn request_applies_bearer_auth_header() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/secure"))
        .and(wiremock::matchers::header("authorization", "Bearer tok-123"))
        .respond_with(ResponseTemplate::new(200).set_body_string("ok"))
        .mount(&server)
        .await;

    let out = ok_with(
        "http.request",
        json!({
            "url": format!("{}/secure", server.uri()),
            "auth": {"kind": "bearer", "token": "tok-123"}
        }),
        net("127.0.0.1"),
    )
    .await;
    assert_eq!(out["status"], json!(200));
}

#[tokio::test]
async fn request_applies_basic_auth_header() {
    let server = MockServer::start().await;
    // base64("alice:s3cret") == "YWxpY2U6czNjcmV0"
    Mock::given(method("GET"))
        .and(path("/basic"))
        .and(wiremock::matchers::header(
            "authorization",
            "Basic YWxpY2U6czNjcmV0",
        ))
        .respond_with(ResponseTemplate::new(204))
        .mount(&server)
        .await;

    let out = ok_with(
        "http.request",
        json!({
            "url": format!("{}/basic", server.uri()),
            "auth": {"kind": "basic", "user": "alice", "pass": "s3cret"}
        }),
        net("127.0.0.1"),
    )
    .await;
    assert_eq!(out["status"], json!(204));
}

#[tokio::test]
async fn request_without_auth_still_works() {
    // Back-compat: omitting `auth` behaves exactly as before.
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/open"))
        .respond_with(ResponseTemplate::new(200).set_body_string("hi"))
        .mount(&server)
        .await;
    let out = ok_with(
        "http.request",
        json!({"url": format!("{}/open", server.uri())}),
        net("127.0.0.1"),
    )
    .await;
    assert_eq!(out["status"], json!(200));
    assert_eq!(out["text"], json!("hi"));
}

#[tokio::test]
async fn request_rejects_unknown_auth_kind() {
    // deny_unknown_fields / the tagged enum reject a bogus scheme at deserialize.
    let err = run(
        "http.request",
        json!({"url": "https://example.com/", "auth": {"kind": "digest", "token": "x"}}),
    )
    .await
    .unwrap_err();
    assert!(err.contains("input invalid"), "got: {err}");
}
