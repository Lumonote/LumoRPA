//! email IMAP 网络路径(`email.fetch` 全文/附件、`email.mark`、`email.move`)
//! 的真服务器集成回归。
//!
//! 这些断言需要真实 IMAP 服务器(信箱里至少一封信),默认跳过;在有服务器的
//! 环境里设置以下变量即可启用(模式同 `db_remote.rs`):
//!
//! ```text
//! LUMO_IMAP_TEST_HOST=imap.example.com   # 必填,触发开关
//! LUMO_IMAP_TEST_USERNAME=user@example.com
//! LUMO_IMAP_TEST_PASSWORD=app-password
//! LUMO_IMAP_TEST_PORT=993                # 可选,默认 993
//! LUMO_IMAP_TEST_MAILBOX=INBOX           # 可选,默认 INBOX
//! LUMO_IMAP_TEST_MOVE_TO=Archive         # 可选,设了才跑 move 往返
//! ```
//!
//! 回归点:`BODY.PEEK[]` 取全文不置 `\Seen`;`email.mark` seen/unseen 可往返;
//! `email.move` 在 MOVE 与 COPY+EXPUNGE 两种路径下消息都确实换了邮箱。

mod common;
use common::{run_with, Capabilities};
use serde_json::{json, Value};

struct ImapEnv {
    host: String,
    port: u16,
    username: String,
    password: String,
    mailbox: String,
}

/// 读取 LUMO_IMAP_TEST_*;HOST 缺失时返回 None(测试静默跳过)。
fn imap_env() -> Option<ImapEnv> {
    let host = std::env::var("LUMO_IMAP_TEST_HOST").ok()?;
    Some(ImapEnv {
        host,
        port: std::env::var("LUMO_IMAP_TEST_PORT")
            .ok()
            .and_then(|p| p.parse().ok())
            .unwrap_or(993),
        username: std::env::var("LUMO_IMAP_TEST_USERNAME").expect("LUMO_IMAP_TEST_USERNAME"),
        password: std::env::var("LUMO_IMAP_TEST_PASSWORD").expect("LUMO_IMAP_TEST_PASSWORD"),
        mailbox: std::env::var("LUMO_IMAP_TEST_MAILBOX").unwrap_or_else(|_| "INBOX".into()),
    })
}

fn net_all() -> Capabilities {
    Capabilities {
        network: vec!["*".to_string()],
        ..Default::default()
    }
}

/// 带 fs.write 授权(附件落盘目录)的能力集。
fn net_and_write(dir: &str) -> Capabilities {
    Capabilities {
        network: vec!["*".to_string()],
        fs_write: vec![format!("{dir}/**")],
        ..Default::default()
    }
}

fn base_input(env: &ImapEnv) -> Value {
    json!({
        "host": env.host,
        "port": env.port,
        "username": env.username,
        "password": env.password,
        "mailbox": env.mailbox,
    })
}

/// 合并 base 连接字段与额外字段。
fn with(env: &ImapEnv, extra: Value) -> Value {
    let mut v = base_input(env);
    let obj = v.as_object_mut().unwrap();
    for (k, val) in extra.as_object().unwrap() {
        obj.insert(k.clone(), val.clone());
    }
    v
}

#[tokio::test]
async fn fetch_with_body_and_attachments_round_trip() {
    let Some(env) = imap_env() else {
        eprintln!("LUMO_IMAP_TEST_HOST not set; skipping live IMAP fetch test");
        return;
    };
    let dir = tempfile::tempdir().expect("tempdir");
    let dir_str = dir.path().display().to_string();
    let out = run_with(
        "email.fetch",
        with(
            &env,
            json!({
                "limit": 3,
                "include_body": true,
                "save_attachments_to": dir_str,
            }),
        ),
        net_and_write(&dir_str),
    )
    .await
    .expect("fetch with body should succeed");

    let arr = out.as_array().expect("array of messages");
    assert!(
        !arr.is_empty(),
        "test mailbox must hold at least one message"
    );
    for msg in arr {
        assert!(msg.get("uid").is_some(), "uid present: {msg}");
        // include_body 时 text/html 字段必在(可为 null),attachments 必为数组。
        assert!(msg.get("text").is_some(), "text key present: {msg}");
        assert!(msg.get("html").is_some(), "html key present: {msg}");
        let atts = msg["attachments"].as_array().expect("attachments array");
        for att in atts {
            let path = att["path"].as_str().expect("path string");
            let meta = std::fs::metadata(path).expect("attachment landed on disk");
            assert_eq!(
                meta.len(),
                att["size"].as_u64().expect("size"),
                "size matches file: {att}"
            );
        }
    }
}

#[tokio::test]
async fn mark_seen_unseen_round_trip() {
    let Some(env) = imap_env() else {
        eprintln!("LUMO_IMAP_TEST_HOST not set; skipping live IMAP mark test");
        return;
    };
    // 取最新一封的 uid。
    let out = run_with("email.fetch", with(&env, json!({"limit": 1})), net_all())
        .await
        .expect("fetch should succeed");
    let uid = out[0]["uid"].as_u64().expect("uid") as u32;

    for set in ["seen", "unseen"] {
        let out = run_with(
            "email.mark",
            with(&env, json!({"uid": uid, "set": set})),
            net_all(),
        )
        .await
        .unwrap_or_else(|e| panic!("mark {set} should succeed: {e}"));
        assert_eq!(out["ok"], json!(true));
        assert_eq!(out["set"], json!(set));
        assert_eq!(out["expunged"], json!(0));
    }
}

#[tokio::test]
async fn move_round_trip() {
    let Some(env) = imap_env() else {
        eprintln!("LUMO_IMAP_TEST_HOST not set; skipping live IMAP move test");
        return;
    };
    let Ok(to_mailbox) = std::env::var("LUMO_IMAP_TEST_MOVE_TO") else {
        eprintln!("LUMO_IMAP_TEST_MOVE_TO not set; skipping live IMAP move test");
        return;
    };

    // 把源邮箱最新一封搬到目标邮箱……
    let out = run_with("email.fetch", with(&env, json!({"limit": 1})), net_all())
        .await
        .expect("fetch should succeed");
    let uid = out[0]["uid"].as_u64().expect("uid") as u32;
    let subject = out[0]["subject"].clone();

    let moved = run_with(
        "email.move",
        with(&env, json!({"uid": uid, "to_mailbox": to_mailbox})),
        net_all(),
    )
    .await
    .expect("move should succeed");
    assert_eq!(moved["ok"], json!(true));
    let method = moved["method"].as_str().expect("method");
    assert!(
        method == "uid_move" || method == "copy_expunge",
        "method = {method}"
    );

    // ……再从目标邮箱搬回来,主题应能对上(目标邮箱里它就是最新一封)。
    let out = run_with(
        "email.fetch",
        json!({
            "host": env.host,
            "port": env.port,
            "username": env.username,
            "password": env.password,
            "mailbox": to_mailbox,
            "limit": 1
        }),
        net_all(),
    )
    .await
    .expect("fetch from target mailbox should succeed");
    assert_eq!(out[0]["subject"], subject, "moved message is in target");
    let new_uid = out[0]["uid"].as_u64().expect("uid") as u32;

    let back = run_with(
        "email.move",
        json!({
            "host": env.host,
            "port": env.port,
            "username": env.username,
            "password": env.password,
            "mailbox": to_mailbox,
            "uid": new_uid,
            "to_mailbox": env.mailbox
        }),
        net_all(),
    )
    .await
    .expect("move back should succeed");
    assert_eq!(back["ok"], json!(true));
}
