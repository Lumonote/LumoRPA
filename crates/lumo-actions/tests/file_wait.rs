//! `file.wait` 集成测试:等待文件出现并稳定(下载完成场景)。
//! 覆盖:已存在立即返回 / 轮询中出现 / 稳定窗口(写两次,隔够 stable_ms 才返回)/
//! 超时报错文案 / fs.read 沙箱拒绝 / must_exist_new 旧文件不算。

mod common;
use common::{fs_caps, ok_with, run, run_with};
use serde_json::json;
use std::time::Duration;

#[tokio::test]
async fn returns_immediately_for_an_existing_stable_file() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("ready.txt");
    std::fs::write(&file, b"hello").unwrap();

    let out = ok_with(
        "file.wait",
        json!({"path": file, "stable_ms": 0, "poll_ms": 20, "timeout_ms": 2_000}),
        fs_caps(dir.path()),
    )
    .await;
    assert_eq!(out["size"], json!(5));
    assert!(
        out["waited_ms"].as_u64().unwrap() < 2_000,
        "should return well before timeout: {out}"
    );
}

#[tokio::test]
async fn picks_up_a_file_that_appears_while_polling() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("late.txt");
    let writer_path = file.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(150)).await;
        std::fs::write(&writer_path, b"abc").unwrap();
    });

    let out = ok_with(
        "file.wait",
        json!({"path": file, "stable_ms": 0, "poll_ms": 25, "timeout_ms": 5_000}),
        fs_caps(dir.path()),
    )
    .await;
    assert_eq!(out["size"], json!(3));
}

#[tokio::test]
async fn waits_for_the_size_to_settle_before_returning() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("download.bin");
    // 模拟分段下载:先落一半,150ms 后写全量;stable_ms=400 应跨过第一次写入,
    // 等到最终 size 稳定后才返回(返回的 size 必须是第二次写入后的)。
    std::fs::write(&file, b"part").unwrap();
    let writer_path = file.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(150)).await;
        std::fs::write(&writer_path, b"full-content").unwrap();
    });

    let out = ok_with(
        "file.wait",
        json!({"path": file, "stable_ms": 400, "poll_ms": 25, "timeout_ms": 5_000}),
        fs_caps(dir.path()),
    )
    .await;
    assert_eq!(out["size"], json!(12), "must see the final size: {out}");
    assert!(
        out["waited_ms"].as_u64().unwrap() >= 400,
        "must honor the stability window: {out}"
    );
}

#[tokio::test]
async fn times_out_with_a_clear_message_when_the_file_never_appears() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("never.txt");

    let err = run_with(
        "file.wait",
        json!({"path": file, "timeout_ms": 200, "poll_ms": 40}),
        fs_caps(dir.path()),
    )
    .await
    .unwrap_err();
    // typed 错误约定:file.wait 超时是 `timeout` kind(诊断细节走 run 日志)。
    assert!(err.contains("timed out after 200ms"), "got: {err}");
    let kind = common::err_kind_with(
        "file.wait",
        json!({"path": dir.path().join("never2.txt"), "timeout_ms": 60, "poll_ms": 20}),
        fs_caps(dir.path()),
    )
    .await;
    assert_eq!(kind, lumo_core::error::ErrorKind::Timeout);
}

#[tokio::test]
async fn times_out_reporting_still_changing_when_size_never_settles() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("growing.bin");
    std::fs::write(&file, b"x").unwrap();
    // 每 50ms 追加一次,size 永不稳定 → 超时文案应说明"还在变化"。
    let writer_path = file.clone();
    let writer = tokio::spawn(async move {
        for _ in 0..20 {
            tokio::time::sleep(Duration::from_millis(50)).await;
            let mut cur = std::fs::read(&writer_path).unwrap_or_default();
            cur.push(b'x');
            std::fs::write(&writer_path, &cur).unwrap();
        }
    });

    let err = run_with(
        "file.wait",
        json!({"path": file, "timeout_ms": 400, "poll_ms": 30, "stable_ms": 10_000}),
        fs_caps(dir.path()),
    )
    .await
    .unwrap_err();
    writer.abort();
    assert!(err.contains("timed out after 400ms"), "got: {err}");
}

#[tokio::test]
async fn must_exist_new_rejects_a_pre_existing_unchanged_file() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("old.txt");
    std::fs::write(&file, b"stale").unwrap();

    let err = run_with(
        "file.wait",
        json!({
            "path": file,
            "must_exist_new": true,
            "timeout_ms": 250,
            "poll_ms": 40,
            "stable_ms": 0
        }),
        fs_caps(dir.path()),
    )
    .await
    .unwrap_err();
    assert!(err.contains("timed out after 250ms"), "got: {err}");
}

#[tokio::test]
async fn wait_denied_without_fs_grant() {
    let err = run(
        "file.wait",
        json!({"path": "/etc/hosts", "timeout_ms": 100}),
    )
    .await
    .unwrap_err();
    assert!(err.contains("capability denied"), "got: {err}");
}
