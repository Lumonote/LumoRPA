//! Explicit FTPS: AUTH TLS must succeed before credentials are transmitted.
//! suppaftp's secure upgrade also requires PBSZ 0 + PROT P (TLS data channels).

use lumo_core::error::StepError;
use lumo_core::{ActionResult, StepCtx};
use serde_json::json;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_rustls::rustls::{
    self,
    pki_types::{pem::PemObject, CertificateDer},
};

pub(super) struct Input {
    pub host: String,
    pub port: u16,
    pub username: String,
    pub password: String,
    pub remote_path: String,
    pub local_path: PathBuf,
    pub max_bytes: u64,
    pub ca_cert: Option<PathBuf>,
    pub timeout_ms: u64,
}

pub(super) async fn transfer(
    ctx: &StepCtx,
    input: Input,
    upload: bool,
) -> Result<ActionResult, StepError> {
    if input.host.is_empty()
        || input.host.chars().any(char::is_control)
        || input.port == 0
        || input.remote_path.is_empty()
        || input.remote_path.chars().any(char::is_control)
        || input.username.chars().any(char::is_control)
        || input.password.chars().any(char::is_control)
        || input.max_bytes == 0
        || !(1..=86_400_000).contains(&input.timeout_ms)
    {
        return Err(StepError::msg(
            "ftp: invalid host, port, path, credentials or transfer limits",
        ));
    }
    if upload {
        ctx.ensure_fs_read(&input.local_path)?;
    } else {
        ctx.ensure_fs_write(&input.local_path)?;
    }
    if let Some(ca) = &input.ca_cert {
        ctx.ensure_fs_read(ca)?;
    }
    ctx.ensure_network_url(&format!("ftp://{}:{}", input.host, input.port))?;
    let work = async {
        let connector = tls_connector(input.ca_cert.as_deref()).await?;
        let ftp = suppaftp::tokio::AsyncRustlsFtpStream::connect((input.host.as_str(), input.port))
            .await
            .map_err(|_| StepError::msg("ftp: connection failed"))?;
        // Failure is terminal: credentials are never sent in plaintext and
        // there is no opportunistic downgrade when AUTH/TLS/PROT P is refused.
        let mut ftp = ftp
            .into_secure(
                suppaftp::tokio::AsyncRustlsConnector::from(connector),
                &input.host,
            )
            .await
            .map_err(|_| {
                StepError::msg(
                    "ftp: explicit TLS negotiation or certificate/hostname verification failed",
                )
            })?;
        ftp.login(&input.username, &input.password)
            .await
            .map_err(|_| StepError::msg("ftp: TLS login failed"))?;
        let result = if upload {
            upload_via(&mut ftp, &input).await
        } else {
            download_via(ctx, &mut ftp, &input).await
        };
        let _ = tokio::time::timeout(Duration::from_secs(2), ftp.quit()).await;
        result
    };
    let cancel = ctx.cancel_token();
    let cancelled = async {
        match cancel {
            Some(token) => token.cancelled().await,
            None => std::future::pending().await,
        }
    };
    tokio::select! {
        result = tokio::time::timeout(Duration::from_millis(input.timeout_ms), work) => {
            result.map_err(|_| StepError::msg("ftp: explicit TLS transfer timed out"))?
        }
        _ = cancelled => Err(StepError::msg("ftp: explicit TLS transfer cancelled")),
    }
}

async fn tls_connector(
    ca: Option<&std::path::Path>,
) -> Result<tokio_rustls::TlsConnector, StepError> {
    let mut roots = rustls::RootCertStore::empty();
    for certificate in rustls_native_certs::load_native_certs().certs {
        let _ = roots.add(certificate);
    }
    if let Some(path) = ca {
        let pem = tokio::fs::read(path)
            .await
            .map_err(|e| StepError::msg(format!("ftp: read CA certificate: {e}")))?;
        let mut count = 0;
        for certificate in CertificateDer::pem_slice_iter(&pem) {
            roots
                .add(certificate.map_err(|_| StepError::msg("ftp: malformed PEM CA certificate"))?)
                .map_err(|_| StepError::msg("ftp: invalid CA certificate"))?;
            count += 1;
        }
        if count == 0 {
            return Err(StepError::msg("ftp: CA bundle contains no certificates"));
        }
    }
    let config = rustls::ClientConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .map_err(|_| StepError::msg("ftp: TLS protocol configuration failed"))?
    .with_root_certificates(roots)
    .with_no_client_auth();
    Ok(tokio_rustls::TlsConnector::from(Arc::new(config)))
}

async fn upload_via(
    ftp: &mut suppaftp::tokio::AsyncRustlsFtpStream,
    input: &Input,
) -> Result<ActionResult, StepError> {
    let metadata = tokio::fs::metadata(&input.local_path)
        .await
        .map_err(io_error)?;
    if !metadata.is_file() || metadata.len() > input.max_bytes {
        return Err(StepError::msg(
            "ftp.upload: source is not a regular file or exceeds max_bytes",
        ));
    }
    let mut file = tokio::fs::File::open(&input.local_path)
        .await
        .map_err(io_error)?;
    let mut limited = (&mut file).take(input.max_bytes.saturating_add(1));
    let mut buffer = Vec::new();
    limited.read_to_end(&mut buffer).await.map_err(io_error)?;
    if buffer.len() as u64 > input.max_bytes {
        return Err(StepError::msg("ftp.upload: source exceeds max_bytes"));
    }
    let mut cursor = std::io::Cursor::new(buffer);
    let bytes = ftp
        .put_file(&input.remote_path, &mut cursor)
        .await
        .map_err(|_| StepError::msg("ftp.upload: encrypted data transfer failed"))?;
    Ok(ActionResult::from(
        json!({"host": input.host, "remote_path": input.remote_path, "bytes": bytes, "tls": "explicit", "data_tls": true}),
    ))
}

async fn download_via(
    ctx: &StepCtx,
    ftp: &mut suppaftp::tokio::AsyncRustlsFtpStream,
    input: &Input,
) -> Result<ActionResult, StepError> {
    let destination = super::sftp::absolute_destination(&input.local_path).await?;
    ctx.ensure_fs_write(&destination)?;
    let stage = super::sftp::LocalStage::create(&destination).await?;
    ctx.ensure_fs_write(&stage.path)?;
    let mut file = tokio::fs::OpenOptions::new()
        .write(true)
        .truncate(true)
        .open(&stage.path)
        .await
        .map_err(io_error)?;
    let mut stream = ftp
        .retr_as_stream(&input.remote_path)
        .await
        .map_err(|_| StepError::msg("ftp.download: encrypted retrieval failed"))?;
    let mut bytes = 0_u64;
    let mut buffer = [0_u8; 32768];
    loop {
        let n = stream.read(&mut buffer).await.map_err(io_error)?;
        if n == 0 {
            break;
        }
        bytes = bytes.saturating_add(n as u64);
        if bytes > input.max_bytes {
            return Err(StepError::msg("ftp.download: response exceeds max_bytes"));
        }
        file.write_all(&buffer[..n]).await.map_err(io_error)?;
    }
    stream
        .finish()
        .await
        .map_err(|_| StepError::msg("ftp.download: encrypted transfer did not finish"))?;
    file.sync_all().await.map_err(io_error)?;
    drop(file);
    stage.publish(&destination).await?;
    Ok(ActionResult::from(
        json!({"host": input.host, "remote_path": input.remote_path, "local_path": destination, "bytes": bytes, "tls": "explicit", "data_tls": true}),
    ))
}

fn io_error(error: std::io::Error) -> StepError {
    StepError::msg(format!("ftp TLS I/O: {error}"))
}
