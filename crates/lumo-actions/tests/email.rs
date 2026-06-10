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

#[tokio::test]
async fn fetch_attachment_dir_fs_write_gated_before_network() {
    // 附件落盘目录的 fs.write 门控先于 network 门控:有网络授权但没有 fs.write
    // 授权时,失败必须是 fs.write 的能力拒绝,而不是连接错误。
    let err = run_with(
        "email.fetch",
        json!({
            "host": "imap.example.com",
            "port": 993,
            "username": "u",
            "password": "p",
            "save_attachments_to": "/srv/mail-attachments"
        }),
        net("imap.example.com"),
    )
    .await
    .unwrap_err();
    assert!(err.contains("capability denied"), "got: {err}");
    assert!(
        err.contains("fs.write"),
        "attachment dir must be gated on fs.write, got: {err}"
    );
    assert!(
        !err.contains("connect") && !err.contains("handshake"),
        "gate must fire before the IMAP connect, got: {err}"
    );
}

#[tokio::test]
async fn fetch_rejects_zero_max_bytes() {
    let err = run(
        "email.fetch",
        json!({
            "host": "imap.example.com",
            "port": 993,
            "username": "u",
            "password": "p",
            "max_bytes_per_message": 0
        }),
    )
    .await
    .unwrap_err();
    assert!(
        err.contains("max_bytes_per_message"),
        "zero max_bytes must be rejected, got: {err}"
    );
}

// ─── email.mark validation + gating ─────────────────────────────────────────────

#[tokio::test]
async fn mark_accepts_single_uid_and_is_denied_without_network() {
    // 单个 uid(非列表)是合法形状:反序列化通过后,卡在 network 门控,
    // 证明 untagged UidSpec 两种写法都能进到门控这一步。
    let err = run(
        "email.mark",
        json!({
            "host": "imap.example.com",
            "port": 993,
            "username": "u",
            "password": "p",
            "uid": 42,
            "set": "seen"
        }),
    )
    .await
    .unwrap_err();
    assert!(err.contains("capability denied"), "got: {err}");
    assert!(err.contains("network"), "should name the network cap: {err}");
    assert!(
        !err.contains("connect") && !err.contains("handshake"),
        "gate must fire before the IMAP connect, got: {err}"
    );
}

#[tokio::test]
async fn mark_rejects_empty_uid_list() {
    let err = run(
        "email.mark",
        json!({
            "host": "imap.example.com",
            "port": 993,
            "username": "u",
            "password": "p",
            "uid": [],
            "set": "seen"
        }),
    )
    .await
    .unwrap_err();
    assert!(
        err.contains("at least one message"),
        "empty uid list must be rejected, got: {err}"
    );
}

#[tokio::test]
async fn mark_rejects_unknown_set_value() {
    let err = run(
        "email.mark",
        json!({
            "host": "imap.example.com",
            "port": 993,
            "username": "u",
            "password": "p",
            "uid": [1],
            "set": "starred"
        }),
    )
    .await
    .unwrap_err();
    assert!(err.contains("input invalid"), "got: {err}");
}

#[tokio::test]
async fn mark_rejects_expunge_without_deleted() {
    // expunge 只配 set: deleted,其他组合是笔误,在触网之前显式拒绝。
    let err = run(
        "email.mark",
        json!({
            "host": "imap.example.com",
            "port": 993,
            "username": "u",
            "password": "p",
            "uid": [1, 2],
            "set": "seen",
            "expunge": true
        }),
    )
    .await
    .unwrap_err();
    assert!(
        err.contains("expunge") && err.contains("deleted"),
        "expunge+seen must be rejected explicitly, got: {err}"
    );
}

// ─── email.move validation + gating ─────────────────────────────────────────────

#[tokio::test]
async fn move_is_denied_without_a_network_grant() {
    let err = run(
        "email.move",
        json!({
            "host": "imap.example.com",
            "port": 993,
            "username": "u",
            "password": "p",
            "uid": [7],
            "to_mailbox": "Archive"
        }),
    )
    .await
    .unwrap_err();
    assert!(err.contains("capability denied"), "got: {err}");
    assert!(err.contains("network"), "should name the network cap: {err}");
    assert!(
        !err.contains("connect") && !err.contains("handshake"),
        "gate must fire before the IMAP connect, got: {err}"
    );
}

#[tokio::test]
async fn move_rejects_same_source_and_target_mailbox() {
    let err = run(
        "email.move",
        json!({
            "host": "imap.example.com",
            "port": 993,
            "username": "u",
            "password": "p",
            "mailbox": "INBOX",
            "uid": [7],
            "to_mailbox": "INBOX"
        }),
    )
    .await
    .unwrap_err();
    assert!(
        err.contains("must differ"),
        "same-mailbox move must be rejected, got: {err}"
    );
}

#[tokio::test]
async fn move_rejects_empty_target_mailbox() {
    let err = run(
        "email.move",
        json!({
            "host": "imap.example.com",
            "port": 993,
            "username": "u",
            "password": "p",
            "uid": [7],
            "to_mailbox": "  "
        }),
    )
    .await
    .unwrap_err();
    assert!(
        err.contains("to_mailbox"),
        "empty target must be rejected, got: {err}"
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
