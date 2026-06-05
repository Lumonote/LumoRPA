//! CI-safe integration coverage for `email.send` / `email.fetch` (F-3).
//!
//! These tests never touch the network: they assert that input validation and
//! capability gating fire **before** any socket is opened. The deny paths need
//! no server at all, and the happy-path round-trips are sketched as `#[ignore]`d
//! e2e tests that require live SMTP/IMAP servers (run manually with
//! `cargo test -- --ignored`).

mod common;
use common::{run, run_with, Capabilities};
use serde_json::json;

fn net(host: &str) -> Capabilities {
    Capabilities {
        network: vec![host.to_string()],
        ..Default::default()
    }
}

// ─── email.send validation + gating ─────────────────────────────────────────────

#[tokio::test]
async fn send_rejects_missing_to() {
    // `to` is required by the schema; omitting it must fail deserialization
    // before anything else happens.
    let err = run(
        "email.send",
        json!({
            "host": "smtp.example.com",
            "port": 465,
            "username": "u",
            "password": "p",
            "from": "a@example.com"
        }),
    )
    .await
    .unwrap_err();
    assert!(err.contains("input invalid"), "got: {err}");
}

#[tokio::test]
async fn send_rejects_empty_to() {
    // An explicit empty recipient list is caught by the early validation guard.
    let err = run(
        "email.send",
        json!({
            "host": "smtp.example.com",
            "port": 465,
            "username": "u",
            "password": "p",
            "from": "a@example.com",
            "to": []
        }),
    )
    .await
    .unwrap_err();
    assert!(
        err.contains("at least one recipient"),
        "empty `to` must be rejected, got: {err}"
    );
}

#[tokio::test]
async fn send_is_denied_without_a_network_grant() {
    // No network grant → rejected before any socket opens, so this stays offline.
    // The error must be a capability denial for the network, NOT a connect error.
    let err = run(
        "email.send",
        json!({
            "host": "smtp.example.com",
            "port": 465,
            "username": "u",
            "password": "p",
            "from": "a@example.com",
            "to": ["b@example.com"],
            "subject": "hi",
            "body": "hello"
        }),
    )
    .await
    .unwrap_err();
    assert!(err.contains("capability denied"), "got: {err}");
    assert!(
        err.contains("network"),
        "should name the network cap: {err}"
    );
    assert!(
        !err.contains("email.send smtp"),
        "gate must fire before the SMTP connect, got: {err}"
    );
}

#[tokio::test]
async fn send_attachment_fs_read_gated_before_network() {
    // The attachment fs.read gate runs before the network gate: with a network
    // grant but no fs.read grant, the failure is an fs.read capability denial,
    // not a connection error.
    let err = run_with(
        "email.send",
        json!({
            "host": "smtp.example.com",
            "port": 465,
            "username": "u",
            "password": "p",
            "from": "a@example.com",
            "to": ["b@example.com"],
            "body": "hello",
            "attachments": ["/etc/shadow"]
        }),
        net("smtp.example.com"),
    )
    .await
    .unwrap_err();
    assert!(err.contains("capability denied"), "got: {err}");
    assert!(
        err.contains("fs.read"),
        "attachment read must be gated on fs.read, got: {err}"
    );
}

// ─── email.fetch validation + gating ─────────────────────────────────────────────

#[tokio::test]
async fn fetch_rejects_missing_required_fields() {
    // `host`/`port`/`username`/`password` are all required by the schema.
    let err = run(
        "email.fetch",
        json!({ "host": "imap.example.com", "port": 993 }),
    )
    .await
    .unwrap_err();
    assert!(err.contains("input invalid"), "got: {err}");
}

#[tokio::test]
async fn fetch_rejects_zero_limit() {
    let err = run(
        "email.fetch",
        json!({
            "host": "imap.example.com",
            "port": 993,
            "username": "u",
            "password": "p",
            "limit": 0
        }),
    )
    .await
    .unwrap_err();
    assert!(
        err.contains("limit"),
        "zero limit must be rejected, got: {err}"
    );
}

#[tokio::test]
async fn fetch_is_denied_without_a_network_grant() {
    // No grant → rejected before any IMAP socket, so this stays offline.
    let err = run(
        "email.fetch",
        json!({
            "host": "imap.example.com",
            "port": 993,
            "username": "u",
            "password": "p"
        }),
    )
    .await
    .unwrap_err();
    assert!(err.contains("capability denied"), "got: {err}");
    assert!(
        err.contains("network"),
        "should name the network cap: {err}"
    );
    assert!(
        !err.contains("connect") && !err.contains("handshake"),
        "gate must fire before the IMAP connect, got: {err}"
    );
}

// ─── e2e sketches (require live servers; run with `--ignored`) ───────────────────

#[ignore = "requires a live SMTP server; set LUMO_SMTP_* env + a network grant"]
#[tokio::test]
async fn e2e_smtp_send_round_trip() {
    // Sketch: point at a real (or mock, e.g. MailHog/Mailpit on 1025) SMTP
    // server, grant its host, and assert `ok == true`.
    //
    //   let out = ok_with("email.send", json!({
    //       "host": "127.0.0.1", "port": 1025,
    //       "username": "u", "password": "p",
    //       "from": "from@test.local", "to": ["to@test.local"],
    //       "subject": "F-3 e2e", "body": "hello",
    //       "html": "<b>hello</b>",
    //       "attachments": ["/granted/path/note.txt"]
    //   }), net("127.0.0.1")).await;
    //   assert_eq!(out["ok"], json!(true));
}

#[ignore = "requires a live IMAP server; set LUMO_IMAP_* env + a network grant"]
#[tokio::test]
async fn e2e_imap_fetch_round_trip() {
    // Sketch: against a real IMAP server (e.g. Greenmail/Dovecot on 993) with at
    // least one message in INBOX, assert the latest-N headers come back newest
    // first with uid/from/subject/date populated.
    //
    //   let out = ok_with("email.fetch", json!({
    //       "host": "imap.example.com", "port": 993,
    //       "username": "u", "password": "p",
    //       "mailbox": "INBOX", "limit": 5
    //   }), net("imap.example.com")).await;
    //   let arr = out.as_array().expect("array of messages");
    //   assert!(arr.len() <= 5);
    //   if let Some(first) = arr.first() {
    //       assert!(first.get("uid").is_some());
    //       assert!(first.get("subject").is_some());
    //   }
}
