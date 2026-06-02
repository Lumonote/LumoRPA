//! F-6 远程文件传输动作的 CI 安全覆盖(无 live server):每个 action 校验
//! 必填字段拒绝、network 未授权 → `capability denied` + `network`(连接之前)、
//! fs 门禁按方向触发(上传读源 / 下载写目标)。真实 FTP/S3 往返见文件末尾的
//! `#[ignore]` e2e 草图(对本地 MinIO / FTP server)。

mod common;
use common::{run, run_with, Capabilities};
use serde_json::json;

fn net(host: &str) -> Capabilities {
    Capabilities {
        network: vec![host.to_string()],
        ..Default::default()
    }
}
fn fs_only(dir: &std::path::Path) -> Capabilities {
    let glob = format!("{}/**", dir.display());
    Capabilities {
        fs_read: vec![glob.clone()],
        fs_write: vec![glob],
        ..Default::default()
    }
}

// ─── 必填字段校验 ────────────────────────────────────────────────────────────

#[tokio::test]
async fn ftp_upload_rejects_missing_required_field() {
    // 缺 local_path → 反序列化失败,绝不建连。
    let err = run(
        "ftp.upload",
        json!({"host": "ftp.example.com", "username": "u", "password": "p", "remote_path": "/x"}),
    )
    .await
    .unwrap_err();
    assert!(err.contains("input invalid"), "got: {err}");
}

#[tokio::test]
async fn ftp_download_rejects_missing_required_field() {
    // 缺 remote_path → 反序列化失败,绝不建连。
    let err = run(
        "ftp.download",
        json!({"host": "h", "username": "u", "password": "p", "local_path": "/t"}),
    )
    .await
    .unwrap_err();
    assert!(err.contains("input invalid"), "got: {err}");
}

#[tokio::test]
async fn s3_put_rejects_missing_required_field() {
    let err = run(
        "s3.put",
        json!({"endpoint": "https://e", "bucket": "b", "key": "k", "access_key": "a"}),
    )
    .await
    .unwrap_err();
    assert!(err.contains("input invalid"), "got: {err}");
}

#[tokio::test]
async fn s3_get_rejects_missing_required_field() {
    let err = run(
        "s3.get",
        json!({"endpoint": "https://e", "bucket": "b", "key": "k"}),
    )
    .await
    .unwrap_err();
    assert!(err.contains("input invalid"), "got: {err}");
}

// ─── network 未授权 → capability denied(连接之前) ──────────────────────────

#[tokio::test]
async fn ftp_upload_denied_network_before_connect() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("f.txt");
    std::fs::write(&src, "x").unwrap();
    // 给了 fs(读源会过),但无 network → 必须在建连前以 network 被拒。
    let err = run_with(
        "ftp.upload",
        json!({"host": "ftp.example.com", "username": "u", "password": "p",
               "remote_path": "/up.txt", "local_path": src}),
        fs_only(dir.path()),
    )
    .await
    .unwrap_err();
    assert!(err.contains("capability denied"), "got: {err}");
    assert!(err.contains("network"), "got: {err}");
}

#[tokio::test]
async fn ftp_download_denied_network_before_connect() {
    let dir = tempfile::tempdir().unwrap();
    let err = run_with(
        "ftp.download",
        json!({"host": "ftp.example.com", "username": "u", "password": "p",
               "remote_path": "/r.txt", "local_path": dir.path().join("got.txt")}),
        fs_only(dir.path()),
    )
    .await
    .unwrap_err();
    assert!(err.contains("capability denied"), "got: {err}");
    assert!(err.contains("network"), "got: {err}");
}

#[tokio::test]
async fn s3_put_denied_network_before_connect() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("o.bin");
    std::fs::write(&src, "data").unwrap();
    let err = run_with(
        "s3.put",
        json!({"endpoint": "https://minio.example.com:9000", "bucket": "b", "key": "k",
               "access_key": "a", "secret_key": "s", "local_path": src}),
        fs_only(dir.path()),
    )
    .await
    .unwrap_err();
    assert!(err.contains("capability denied"), "got: {err}");
    assert!(err.contains("network"), "got: {err}");
}

#[tokio::test]
async fn s3_get_denied_network_before_connect() {
    let dir = tempfile::tempdir().unwrap();
    let err = run_with(
        "s3.get",
        json!({"endpoint": "https://minio.example.com:9000", "bucket": "b", "key": "k",
               "access_key": "a", "secret_key": "s", "local_path": dir.path().join("o.bin")}),
        fs_only(dir.path()),
    )
    .await
    .unwrap_err();
    assert!(err.contains("capability denied"), "got: {err}");
    assert!(err.contains("network"), "got: {err}");
}

// ─── fs 门禁按方向触发 ───────────────────────────────────────────────────────

#[tokio::test]
async fn ftp_upload_denied_fs_read_for_source() {
    // 给了 network,但无 fs.read → 读源前以 fs.read 被拒(连接之前)。
    let err = run_with(
        "ftp.upload",
        json!({"host": "ftp.example.com", "username": "u", "password": "p",
               "remote_path": "/up.txt", "local_path": "/secret/forbidden.txt"}),
        net("ftp.example.com"),
    )
    .await
    .unwrap_err();
    assert!(err.contains("capability denied"), "got: {err}");
    assert!(err.contains("fs.read"), "got: {err}");
}

#[tokio::test]
async fn ftp_download_denied_fs_write_for_dest() {
    let err = run_with(
        "ftp.download",
        json!({"host": "ftp.example.com", "username": "u", "password": "p",
               "remote_path": "/r.txt", "local_path": "/secret/out.txt"}),
        net("ftp.example.com"),
    )
    .await
    .unwrap_err();
    assert!(err.contains("capability denied"), "got: {err}");
    assert!(err.contains("fs.write"), "got: {err}");
}

#[tokio::test]
async fn s3_put_denied_fs_read_for_source() {
    let err = run_with(
        "s3.put",
        json!({"endpoint": "https://minio.example.com:9000", "bucket": "b", "key": "k",
               "access_key": "a", "secret_key": "s", "local_path": "/secret/o.bin"}),
        net("minio.example.com"),
    )
    .await
    .unwrap_err();
    assert!(err.contains("capability denied"), "got: {err}");
    assert!(err.contains("fs.read"), "got: {err}");
}

#[tokio::test]
async fn s3_get_denied_fs_write_for_dest() {
    let err = run_with(
        "s3.get",
        json!({"endpoint": "https://minio.example.com:9000", "bucket": "b", "key": "k",
               "access_key": "a", "secret_key": "s", "local_path": "/secret/o.bin"}),
        net("minio.example.com"),
    )
    .await
    .unwrap_err();
    assert!(err.contains("capability denied"), "got: {err}");
    assert!(err.contains("fs.write"), "got: {err}");
}

// ─── e2e 草图(默认 #[ignore],需真实 server) ──────────────────────────────

/// 真实 FTP 往返:对本地 FTP server(如 `docker run -p 21:21 ...
/// stilliard/pure-ftpd`)上传后再下载,断言字节一致。设好环境变量后:
/// `cargo test -p lumo-actions --test transfer -- --ignored ftp_roundtrip`。
#[tokio::test]
#[ignore = "需真实 FTP server;设 FTP_HOST/FTP_USER/FTP_PASS 后手动运行"]
async fn ftp_roundtrip_against_live_server() {
    let host = std::env::var("FTP_HOST").unwrap();
    let user = std::env::var("FTP_USER").unwrap();
    let pass = std::env::var("FTP_PASS").unwrap();
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("up.txt");
    std::fs::write(&src, "f6-ftp-roundtrip").unwrap();
    let caps = Capabilities {
        network: vec![host.clone()],
        fs_read: vec![format!("{}/**", dir.path().display())],
        fs_write: vec![format!("{}/**", dir.path().display())],
        ..Default::default()
    };
    common::ok_with(
        "ftp.upload",
        json!({"host": host, "username": user, "password": pass,
               "remote_path": "f6.txt", "local_path": src}),
        caps.clone(),
    )
    .await;
    let dst = dir.path().join("down.txt");
    common::ok_with(
        "ftp.download",
        json!({"host": host, "username": user, "password": pass,
               "remote_path": "f6.txt", "local_path": dst.clone()}),
        caps,
    )
    .await;
    assert_eq!(std::fs::read_to_string(&dst).unwrap(), "f6-ftp-roundtrip");
}

/// 真实 S3 往返:对本地 MinIO(`docker run -p 9000:9000 minio/minio server /data`)
/// put 后 get,断言字节一致。需 S3_ENDPOINT/S3_BUCKET/S3_AK/S3_SK,且 bucket 已建。
#[tokio::test]
#[ignore = "需真实 S3/MinIO;设 S3_ENDPOINT/S3_BUCKET/S3_AK/S3_SK 后手动运行"]
async fn s3_roundtrip_against_live_minio() {
    let endpoint = std::env::var("S3_ENDPOINT").unwrap();
    let bucket = std::env::var("S3_BUCKET").unwrap();
    let ak = std::env::var("S3_AK").unwrap();
    let sk = std::env::var("S3_SK").unwrap();
    let host = endpoint
        .split_once("://")
        .map(|(_, r)| r)
        .unwrap_or(&endpoint)
        .to_string();
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("o.bin");
    std::fs::write(&src, b"f6-s3-roundtrip").unwrap();
    let caps = Capabilities {
        network: vec![host],
        fs_read: vec![format!("{}/**", dir.path().display())],
        fs_write: vec![format!("{}/**", dir.path().display())],
        ..Default::default()
    };
    common::ok_with(
        "s3.put",
        json!({"endpoint": endpoint, "bucket": bucket, "key": "f6/o.bin",
               "access_key": ak, "secret_key": sk, "local_path": src}),
        caps.clone(),
    )
    .await;
    let dst = dir.path().join("got.bin");
    common::ok_with(
        "s3.get",
        json!({"endpoint": endpoint, "bucket": bucket, "key": "f6/o.bin",
               "access_key": ak, "secret_key": sk, "local_path": dst.clone()}),
        caps,
    )
    .await;
    assert_eq!(std::fs::read(&dst).unwrap(), b"f6-s3-roundtrip");
}
