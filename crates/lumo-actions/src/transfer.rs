//! F-6 远程文件传输动作族:FTP 上传/下载 + S3 兼容对象存储 上传/下载。
//!
//! 设计要点(与 `http.rs`/`file.rs` 同一套范式):
//! * 输入结构体 `#[derive(Deserialize)]` 为单一真源;`schema()` 返回手写
//!   `static SCHEMA`(本分支尚无 schemars 派生,沿用 http/file 的写法)。
//! * 能力门禁在「连接之前」全部跑完:校验必填字段 → 按方向 gate fs(上传读源 /
//!   下载写目标)→ gate 远端 host(`ensure_network_url`)。未授权能力会以
//!   `capability denied` 快速失败,绝不先建连。
//! * 机密(password / secret_key)以普通输入串到达(flow 作者写
//!   `${{ vault.* }}`,VM 在本动作执行前已解析)。仅作字段接收,**永不日志**。
//!
//! TLS:全部走 rustls(ring)——FTP 经 suppaftp tokio-rustls-ring(此处用明文
//! 控制连接,数据走 rustls 能力已编译进,后续可加 explicit FTPS);S3 经
//! workspace 的 reqwest(rustls/ring)。不引 openssl/native-tls/任何 C 依赖。
//!
//! 已 DEFER:SFTP。纯 Rust 的 russh + russh-sftp 需自管密钥交换、host-key 校验、
//! channel/subsystem 握手与会话状态机,体量与本族其余动作不成比例;为保 F-6
//! 小而干净、全门禁通过,SFTP 暂缓(参考 F-10 对 iframe 的处理方式)。引入时应
//! 沿用 russh(纯 Rust,避开 libssh2 这个会破坏交叉编译的 C 依赖)。

mod sigv4;

use async_trait::async_trait;
use lumo_core::error::StepError;
use lumo_core::{Action, ActionRegistry, ActionResult, StepCtx};
use once_cell::sync::Lazy;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::Value;
use std::path::PathBuf;

pub fn register(r: &mut ActionRegistry) {
    r.register(FtpUploadAction);
    r.register(FtpDownloadAction);
    r.register(S3PutAction);
    r.register(S3GetAction);
}

fn default_ftp_port() -> u16 {
    21
}
fn default_region() -> String {
    "us-east-1".into()
}
fn default_max_bytes() -> u64 {
    100 * 1024 * 1024 // 100 MiB,与 http.* 对齐
}

// ─── ftp.upload ─────────────────────────────────────────────────────────────

pub struct FtpUploadAction;

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct FtpUploadIn {
    host: String,
    #[serde(default = "default_ftp_port")]
    port: u16,
    username: String,
    password: String,
    remote_path: String,
    local_path: PathBuf,
    #[serde(default = "default_max_bytes")]
    max_bytes: u64,
}

#[async_trait]
impl Action for FtpUploadAction {
    fn id(&self) -> &'static str {
        "ftp.upload"
    }
    fn summary(&self) -> &'static str {
        "Upload a local file to an FTP server"
    }
    fn schema(&self) -> &'static serde_json::Value {
        static SCHEMA: Lazy<Value> = Lazy::new(crate::schema::derive::<FtpUploadIn>);
        &SCHEMA
    }
    async fn execute(&self, ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let FtpUploadIn {
            host,
            port,
            username,
            password,
            remote_path,
            local_path,
            max_bytes,
        } = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("ftp.upload input invalid: {e}")))?;

        // 门禁:先 fs.read 源,再 network host。两者任一未授权 → capability denied,
        // 不建连。`ftp://{host}` 让 ensure_network_url 的 extract_host 取到 host。
        ctx.ensure_fs_read(&local_path)?;
        ctx.ensure_network_url(&format!("ftp://{host}"))?;

        // OOM 护栏:stat 后比对 max_bytes,再读入内存。
        let meta = tokio::fs::metadata(&local_path)
            .await
            .map_err(|e| StepError::msg(format!("ftp.upload stat {}: {e}", local_path.display())))?;
        if meta.len() > max_bytes {
            return Err(StepError::msg(format!(
                "ftp.upload: file size {} exceeds max_bytes {max_bytes}",
                meta.len()
            )));
        }
        let bytes = tokio::fs::read(&local_path)
            .await
            .map_err(|e| StepError::msg(format!("ftp.upload read {}: {e}", local_path.display())))?;

        let mut ftp = ftp_connect_login(&host, port, &username, &password, "ftp.upload").await?;
        let mut cursor = std::io::Cursor::new(bytes);
        let written = ftp
            .put_file(&remote_path, &mut cursor)
            .await
            .map_err(|e| StepError::msg(format!("ftp.upload store {remote_path}: {e}")))?;
        let _ = ftp.quit().await;

        Ok(ActionResult::from(serde_json::json!({
            "host": host,
            "remote_path": remote_path,
            "bytes": written,
        })))
    }
}

// ─── ftp.download ───────────────────────────────────────────────────────────

pub struct FtpDownloadAction;

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct FtpDownloadIn {
    host: String,
    #[serde(default = "default_ftp_port")]
    port: u16,
    username: String,
    password: String,
    remote_path: String,
    local_path: PathBuf,
    #[serde(default = "default_max_bytes")]
    max_bytes: u64,
}

#[async_trait]
impl Action for FtpDownloadAction {
    fn id(&self) -> &'static str {
        "ftp.download"
    }
    fn summary(&self) -> &'static str {
        "Download a file from an FTP server to a local path"
    }
    fn schema(&self) -> &'static serde_json::Value {
        static SCHEMA: Lazy<Value> = Lazy::new(crate::schema::derive::<FtpDownloadIn>);
        &SCHEMA
    }
    async fn execute(&self, ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let FtpDownloadIn {
            host,
            port,
            username,
            password,
            remote_path,
            local_path,
            max_bytes,
        } = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("ftp.download input invalid: {e}")))?;

        // 门禁:先 fs.write 目标,再 network host —— 连接之前完成。
        ctx.ensure_fs_write(&local_path)?;
        ctx.ensure_network_url(&format!("ftp://{host}"))?;

        let mut ftp = ftp_connect_login(&host, port, &username, &password, "ftp.download").await?;
        let mut stream = ftp
            .retr_as_stream(&remote_path)
            .await
            .map_err(|e| StepError::msg(format!("ftp.download retr {remote_path}: {e}")))?;

        // 读入内存并 max_bytes 兜底(FTP 无 Content-Length,只能边读边卡)。
        use tokio::io::AsyncReadExt;
        let mut buf = Vec::new();
        let mut chunk = [0u8; 64 * 1024];
        loop {
            let n = stream
                .read(&mut chunk)
                .await
                .map_err(|e| StepError::msg(format!("ftp.download read: {e}")))?;
            if n == 0 {
                break;
            }
            buf.extend_from_slice(&chunk[..n]);
            if buf.len() as u64 > max_bytes {
                return Err(StepError::msg(format!(
                    "ftp.download: response exceeds max_bytes {max_bytes}"
                )));
            }
        }
        ftp.finalize_retr_stream(stream)
            .await
            .map_err(|e| StepError::msg(format!("ftp.download finalize: {e}")))?;
        let _ = ftp.quit().await;

        if let Some(parent) = local_path.parent() {
            let _ = tokio::fs::create_dir_all(parent).await;
        }
        tokio::fs::write(&local_path, &buf).await.map_err(|e| {
            StepError::msg(format!("ftp.download write {}: {e}", local_path.display()))
        })?;

        Ok(ActionResult::from(serde_json::json!({
            "host": host,
            "remote_path": remote_path,
            "local_path": local_path,
            "bytes": buf.len(),
        })))
    }
}

/// 建立明文 FTP 控制连接并登录。错误串只带 host/动作名,**不带凭据**。
async fn ftp_connect_login(
    host: &str,
    port: u16,
    username: &str,
    password: &str,
    who: &str,
) -> Result<suppaftp::tokio::AsyncFtpStream, StepError> {
    let mut ftp = suppaftp::tokio::AsyncFtpStream::connect((host, port))
        .await
        .map_err(|e| StepError::msg(format!("{who} connect {host}:{port}: {e}")))?;
    ftp.login(username, password)
        .await
        // 故意只回 "login failed",不回原始错误,避免回显用户名/口令片段。
        .map_err(|_| StepError::msg(format!("{who} login failed for {host}:{port}")))?;
    Ok(ftp)
}

// ─── s3.put ─────────────────────────────────────────────────────────────────

pub struct S3PutAction;

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct S3PutIn {
    endpoint: String,
    #[serde(default = "default_region")]
    region: String,
    bucket: String,
    key: String,
    access_key: String,
    secret_key: String,
    local_path: PathBuf,
    #[serde(default = "default_max_bytes")]
    max_bytes: u64,
}

#[async_trait]
impl Action for S3PutAction {
    fn id(&self) -> &'static str {
        "s3.put"
    }
    fn summary(&self) -> &'static str {
        "Upload a local file to an S3-compatible bucket (MinIO/OSS via endpoint)"
    }
    fn schema(&self) -> &'static serde_json::Value {
        static SCHEMA: Lazy<Value> = Lazy::new(crate::schema::derive::<S3PutIn>);
        &SCHEMA
    }
    async fn execute(&self, ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let S3PutIn {
            endpoint,
            region,
            bucket,
            key,
            access_key,
            secret_key,
            local_path,
            max_bytes,
        } = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("s3.put input invalid: {e}")))?;

        // 门禁:fs.read 源 + network endpoint host —— 连接之前完成。
        ctx.ensure_fs_read(&local_path)?;
        ctx.ensure_network_url(&endpoint)?;

        let meta = tokio::fs::metadata(&local_path)
            .await
            .map_err(|e| StepError::msg(format!("s3.put stat {}: {e}", local_path.display())))?;
        if meta.len() > max_bytes {
            return Err(StepError::msg(format!(
                "s3.put: file size {} exceeds max_bytes {max_bytes}",
                meta.len()
            )));
        }
        let body = tokio::fs::read(&local_path)
            .await
            .map_err(|e| StepError::msg(format!("s3.put read {}: {e}", local_path.display())))?;

        let resp = s3_request(
            "PUT",
            &endpoint,
            &region,
            &bucket,
            &key,
            &access_key,
            &secret_key,
            body,
        )
        .await?;
        let status = resp.status;
        if !(200..300).contains(&status) {
            return Err(StepError::msg(format!(
                "s3.put: server returned status {status}"
            )));
        }
        Ok(ActionResult::from(serde_json::json!({
            "bucket": bucket,
            "key": key,
            "status": status,
            "bytes": meta.len(),
        })))
    }
}

// ─── s3.get ─────────────────────────────────────────────────────────────────

pub struct S3GetAction;

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct S3GetIn {
    endpoint: String,
    #[serde(default = "default_region")]
    region: String,
    bucket: String,
    key: String,
    access_key: String,
    secret_key: String,
    local_path: PathBuf,
    #[serde(default = "default_max_bytes")]
    max_bytes: u64,
}

#[async_trait]
impl Action for S3GetAction {
    fn id(&self) -> &'static str {
        "s3.get"
    }
    fn summary(&self) -> &'static str {
        "Download an object from an S3-compatible bucket to a local path"
    }
    fn schema(&self) -> &'static serde_json::Value {
        static SCHEMA: Lazy<Value> = Lazy::new(crate::schema::derive::<S3GetIn>);
        &SCHEMA
    }
    async fn execute(&self, ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let S3GetIn {
            endpoint,
            region,
            bucket,
            key,
            access_key,
            secret_key,
            local_path,
            max_bytes,
        } = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("s3.get input invalid: {e}")))?;

        // 门禁:fs.write 目标 + network endpoint host —— 连接之前完成。
        ctx.ensure_fs_write(&local_path)?;
        ctx.ensure_network_url(&endpoint)?;

        let resp = s3_request(
            "GET",
            &endpoint,
            &region,
            &bucket,
            &key,
            &access_key,
            &secret_key,
            Vec::new(),
        )
        .await?;
        if !(200..300).contains(&resp.status) {
            return Err(StepError::msg(format!(
                "s3.get: server returned status {}",
                resp.status
            )));
        }
        if resp.body.len() as u64 > max_bytes {
            return Err(StepError::msg(format!(
                "s3.get: object {} bytes exceeds max_bytes {max_bytes}",
                resp.body.len()
            )));
        }

        if let Some(parent) = local_path.parent() {
            let _ = tokio::fs::create_dir_all(parent).await;
        }
        tokio::fs::write(&local_path, &resp.body)
            .await
            .map_err(|e| StepError::msg(format!("s3.get write {}: {e}", local_path.display())))?;

        Ok(ActionResult::from(serde_json::json!({
            "bucket": bucket,
            "key": key,
            "local_path": local_path,
            "status": resp.status,
            "bytes": resp.body.len(),
        })))
    }
}

struct S3Resp {
    status: u16,
    body: Vec<u8>,
}

/// 对 S3 兼容存储发一次 path-style 请求(`{endpoint}/{bucket}/{key}`),自签
/// SigV4。path-style 兼容 MinIO/OSS 自定义 endpoint 而无需通配证书。错误串经
/// `without_url()` 剥离 URL,防 query/userinfo 里的凭据落日志。
#[allow(clippy::too_many_arguments)]
async fn s3_request(
    method: &str,
    endpoint: &str,
    region: &str,
    bucket: &str,
    key: &str,
    access_key: &str,
    secret_key: &str,
    body: Vec<u8>,
) -> Result<S3Resp, StepError> {
    let endpoint = endpoint.trim_end_matches('/');
    // endpoint 必须带 scheme;host 用于 SigV4 的 canonical host 头。
    let host = endpoint
        .split_once("://")
        .map(|(_, rest)| rest)
        .unwrap_or(endpoint)
        .trim_end_matches('/');
    let canonical_path = format!("/{}/{}", bucket.trim_matches('/'), key.trim_start_matches('/'));
    let url = format!("{endpoint}{canonical_path}");

    let now = chrono::Utc::now();
    let amz_date = now.format("%Y%m%dT%H%M%SZ").to_string();
    let date_stamp = now.format("%Y%m%d").to_string();
    let payload_sha256 = sigv4::sha256_hex(&body);

    let auth = sigv4::authorization_header(&sigv4::SigV4Input {
        method,
        host,
        canonical_path: &canonical_path,
        region,
        access_key,
        secret_key,
        payload_sha256: &payload_sha256,
        amz_date: &amz_date,
        date_stamp: &date_stamp,
    });

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(60))
        .build()
        .map_err(|e| StepError::msg(format!("s3 client: {e}")))?;
    let mut req = client
        .request(
            method
                .parse()
                .map_err(|e| StepError::msg(format!("s3 bad method: {e}")))?,
            &url,
        )
        .header("host", host)
        .header("x-amz-date", &amz_date)
        .header("x-amz-content-sha256", &payload_sha256)
        .header("authorization", auth);
    if method == "PUT" {
        req = req.body(body);
    }

    let resp = req
        .send()
        .await
        .map_err(|e| StepError::msg(format!("s3 send: {}", e.without_url())))?;
    let status = resp.status().as_u16();
    let body = resp
        .bytes()
        .await
        .map_err(|e| StepError::msg(format!("s3 body: {}", e.without_url())))?
        .to_vec();
    Ok(S3Resp { status, body })
}
