//! Production registry gates and adversarial inputs; no external network needed.
mod common;
use common::{run, run_with, Capabilities};
use serde_json::{json, Value};

fn sftp_input(dir: &std::path::Path) -> Value {
    json!({"host": "127.0.0.1", "username": "test", "identity_file": dir.join("key"),
        "known_hosts": dir.join("known_hosts"), "remote_path": "/remote.txt", "local_path": dir.join("local.txt")})
}
fn caps(dir: &std::path::Path) -> Capabilities {
    Capabilities {
        fs_read: vec![format!("{}/**", dir.display())],
        fs_write: vec![format!("{}/**", dir.display())],
        ..Default::default()
    }
}

#[tokio::test]
async fn sftp_requires_explicit_host_key_file_and_rejects_password_authentication() {
    let dir = tempfile::tempdir().unwrap();
    let mut input = sftp_input(dir.path());
    input.as_object_mut().unwrap().remove("known_hosts");
    assert!(run("sftp.download", input)
        .await
        .unwrap_err()
        .contains("known_hosts"));
    let mut input = sftp_input(dir.path());
    input["password"] = "not-supported".into();
    assert!(run("sftp.upload", input)
        .await
        .unwrap_err()
        .contains("unknown field"));
}

#[tokio::test]
async fn sftp_rejects_command_and_host_option_injection_before_connect() {
    let dir = tempfile::tempdir().unwrap();
    for (field, value) in [
        ("remote_path", "file\n!touch /tmp/exploit"),
        ("host", "-oProxyCommand=evil"),
        ("username", "user\nProxyCommand=evil"),
    ] {
        let mut input = sftp_input(dir.path());
        input[field] = value.into();
        let error = run("sftp.download", input).await.unwrap_err();
        assert!(!error.contains("cannot start"));
        assert!(error.contains("sftp:"), "{error}");
    }
}

#[tokio::test]
async fn sftp_network_denial_precedes_credentials_and_process_start() {
    let dir = tempfile::tempdir().unwrap();
    let error = run_with("sftp.download", sftp_input(dir.path()), caps(dir.path()))
        .await
        .unwrap_err();
    assert!(
        error.contains("capability denied") && error.contains("network"),
        "{error}"
    );
}

#[tokio::test]
async fn sftp_identity_file_needs_its_own_read_grant() {
    let dir = tempfile::tempdir().unwrap();
    let mut input = sftp_input(dir.path());
    input["identity_file"] = "/forbidden/id_ed25519".into();
    let error = run_with("sftp.download", input, caps(dir.path()))
        .await
        .unwrap_err();
    assert!(error.contains("fs.read"), "{error}");
}

#[tokio::test]
async fn ftp_refuses_ca_bundle_without_explicit_tls() {
    for action in ["ftp.upload", "ftp.download"] {
        let error = run(action, json!({"host": "localhost", "username": "u", "password": "p", "remote_path": "x", "local_path": "/tmp/local", "ca_cert": "/tmp/ca.pem"})).await.unwrap_err();
        assert!(error.contains("requires tls: explicit"), "{error}");
    }
}

#[tokio::test]
async fn explicit_ftps_is_gated_before_tls_or_credential_access() {
    let dir = tempfile::tempdir().unwrap();
    let error = run_with("ftp.download", json!({"host": "localhost", "username": "u", "password": "p", "remote_path": "x", "local_path": dir.path().join("out"), "tls": "explicit"}), caps(dir.path())).await.unwrap_err();
    assert!(
        error.contains("capability denied") && error.contains("network"),
        "{error}"
    );
}

#[tokio::test]
async fn explicit_ftps_never_sends_login_when_auth_tls_is_refused() {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = tokio::spawn(async move {
        let (mut connection, _) = listener.accept().await.unwrap();
        connection.write_all(b"220 test\r\n").await.unwrap();
        let mut reader = BufReader::new(connection);
        let mut command = String::new();
        reader.read_line(&mut command).await.unwrap();
        assert_eq!(command, "AUTH TLS\r\n");
        reader
            .get_mut()
            .write_all(b"500 TLS not supported\r\n")
            .await
            .unwrap();
        command.clear();
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            reader.read_line(&mut command),
        )
        .await;
        assert!(matches!(result, Ok(Ok(0))) || !command.starts_with("USER"));
    });
    let dir = tempfile::tempdir().unwrap();
    let mut permissions = caps(dir.path());
    permissions.network.push("127.0.0.1".into());
    let error = run_with("ftp.download", json!({"host": "127.0.0.1", "port": port, "username": "credential-must-not-leak", "password": "must-not-leak", "remote_path": "x", "local_path": dir.path().join("out"), "tls": "explicit", "timeout_ms": 5000}), permissions).await.unwrap_err();
    assert!(error.contains("explicit TLS negotiation"), "{error}");
    assert!(!error.contains("must-not-leak"));
    server.await.unwrap();
}

#[tokio::test]
#[ignore = "requires isolated OpenSSH server; use scripts/secure-transfer-e2e.py"]
async fn live_sftp_roundtrip_and_host_key_rejection() {
    let host = "127.0.0.1";
    let port: u16 = std::env::var("LUMO_SFTP_PORT").unwrap().parse().unwrap();
    let root = std::path::PathBuf::from(std::env::var_os("LUMO_SFTP_TEST_ROOT").unwrap());
    let mut input = json!({"host": host, "port": port, "username": std::env::var("LUMO_SFTP_USER").unwrap(),
        "identity_file": root.join("client"), "known_hosts": root.join("known_hosts"),
        "remote_path": root.join("remote file.txt"), "local_path": root.join("source file.txt"), "timeout_ms": 10000});
    let mut permissions = caps(&root);
    permissions.network.push(host.into());
    std::fs::write(root.join("source file.txt"), "SFTP 中文 roundtrip").unwrap();
    common::ok_with("sftp.upload", input.clone(), permissions.clone()).await;
    input["local_path"] = json!(root.join("result file.txt"));
    let result = common::ok_with("sftp.download", input.clone(), permissions.clone()).await;
    assert_eq!(result["host_key_verified"], true);
    assert_eq!(
        std::fs::read_to_string(root.join("result file.txt")).unwrap(),
        "SFTP 中文 roundtrip"
    );
    // Oversize/host-key failures must preserve the existing destination.
    input["max_bytes"] = 1.into();
    assert!(
        run_with("sftp.download", input.clone(), permissions.clone())
            .await
            .unwrap_err()
            .contains("max_bytes")
    );
    assert_eq!(
        std::fs::read_to_string(root.join("result file.txt")).unwrap(),
        "SFTP 中文 roundtrip"
    );
    input["max_bytes"] = 1_000_000.into();
    input["known_hosts"] = json!(root.join("wrong_known_hosts"));
    assert!(run_with("sftp.download", input, permissions).await.is_err());
    assert_eq!(
        std::fs::read_to_string(root.join("result file.txt")).unwrap(),
        "SFTP 中文 roundtrip"
    );
}

#[tokio::test]
#[ignore = "requires isolated TLS FTP fixture; use scripts/secure-transfer-e2e.py"]
async fn live_ftps_roundtrip_and_certificate_validation() {
    let port: u16 = std::env::var("LUMO_FTPS_PORT").unwrap().parse().unwrap();
    let root = std::path::PathBuf::from(std::env::var_os("LUMO_SFTP_TEST_ROOT").unwrap());
    let source = root.join("ftps source.txt");
    let destination = root.join("ftps result.txt");
    std::fs::write(&source, "FTPS encrypted 中文 roundtrip").unwrap();
    let mut permissions = caps(&root);
    permissions.network = vec!["localhost".into(), "127.0.0.1".into()];
    let mut input = json!({"host": "localhost", "port": port, "username": "test", "password": "fixture", "tls": "explicit",
        "ca_cert": root.join("ca.crt"), "remote_path": "roundtrip.txt", "local_path": source, "timeout_ms": 10000});
    let uploaded = common::ok_with("ftp.upload", input.clone(), permissions.clone()).await;
    assert_eq!(uploaded["data_tls"], true);
    input["local_path"] = json!(&destination);
    common::ok_with("ftp.download", input.clone(), permissions.clone()).await;
    assert_eq!(
        std::fs::read_to_string(&destination).unwrap(),
        "FTPS encrypted 中文 roundtrip"
    );
    input["max_bytes"] = 1.into();
    let error = run_with("ftp.download", input.clone(), permissions.clone())
        .await
        .unwrap_err();
    assert!(error.contains("max_bytes"), "{error}");
    assert_eq!(
        std::fs::read_to_string(&destination).unwrap(),
        "FTPS encrypted 中文 roundtrip"
    );
    input["max_bytes"] = 1_000_000.into();
    input["host"] = "127.0.0.1".into();
    assert!(run_with("ftp.download", input.clone(), permissions.clone())
        .await
        .unwrap_err()
        .contains("certificate/hostname"));
    input["host"] = "localhost".into();
    input.as_object_mut().unwrap().remove("ca_cert");
    assert!(run_with("ftp.download", input, permissions)
        .await
        .unwrap_err()
        .contains("certificate/hostname"));
    assert_eq!(
        std::fs::read_to_string(&destination).unwrap(),
        "FTPS encrypted 中文 roundtrip"
    );
}
