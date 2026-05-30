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
