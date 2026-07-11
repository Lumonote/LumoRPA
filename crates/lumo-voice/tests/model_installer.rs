use async_trait::async_trait;
use lumo_voice::model_installer::{
    install_voice_model_with_downloader, sha256_hex, ModelDownloader, VoiceModelError,
    VoiceModelKind, VoiceModelManifest,
};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::io::AsyncWriteExt;
use tokio_util::sync::CancellationToken;

struct TestDir(PathBuf);

impl TestDir {
    fn new(label: &str) -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path =
            std::env::temp_dir().join(format!("lumo-model-{label}-{}-{nonce}", std::process::id()));
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[derive(Clone)]
struct BytesDownloader {
    bytes: Arc<Vec<u8>>,
}

#[async_trait]
impl ModelDownloader for BytesDownloader {
    async fn download(
        &self,
        _url: &str,
        destination: &Path,
        cancel: CancellationToken,
    ) -> Result<(), VoiceModelError> {
        if cancel.is_cancelled() {
            return Err(VoiceModelError::Cancelled);
        }
        tokio::fs::write(destination, self.bytes.as_slice()).await?;
        Ok(())
    }
}

struct CancellingDownloader {
    wrote_partial: Arc<AtomicBool>,
}

struct IgnoringDownloader;

#[async_trait]
impl ModelDownloader for IgnoringDownloader {
    async fn download(
        &self,
        _url: &str,
        destination: &Path,
        _cancel: CancellationToken,
    ) -> Result<(), VoiceModelError> {
        tokio::fs::write(destination, b"partial").await?;
        std::future::pending().await
    }
}

#[async_trait]
impl ModelDownloader for CancellingDownloader {
    async fn download(
        &self,
        _url: &str,
        destination: &Path,
        cancel: CancellationToken,
    ) -> Result<(), VoiceModelError> {
        let mut file = tokio::fs::File::create(destination).await?;
        file.write_all(b"partial").await?;
        file.flush().await?;
        self.wrote_partial.store(true, Ordering::SeqCst);
        cancel.cancelled().await;
        Err(VoiceModelError::Cancelled)
    }
}

fn archive(files: &[(&str, &[u8])]) -> Vec<u8> {
    let mut bytes = Vec::new();
    for (path, contents) in files {
        let mut header = [0_u8; 512];
        header[..path.len()].copy_from_slice(path.as_bytes());
        write_octal(&mut header[100..108], 0o644);
        write_octal(&mut header[108..116], 0);
        write_octal(&mut header[116..124], 0);
        write_octal(&mut header[124..136], contents.len() as u64);
        write_octal(&mut header[136..148], 0);
        header[148..156].fill(b' ');
        header[156] = b'0';
        header[257..263].copy_from_slice(b"ustar\0");
        header[263..265].copy_from_slice(b"00");
        let checksum: u64 = header.iter().map(|byte| u64::from(*byte)).sum();
        write_checksum(&mut header[148..156], checksum);
        bytes.extend_from_slice(&header);
        bytes.extend_from_slice(contents);
        let padding = (512 - contents.len() % 512) % 512;
        bytes.resize(bytes.len() + padding, 0);
    }
    bytes.resize(bytes.len() + 1_024, 0);
    bytes
}

fn write_octal(field: &mut [u8], value: u64) {
    let encoded = format!("{:0width$o}\0", value, width = field.len() - 1);
    field.copy_from_slice(encoded.as_bytes());
}

fn write_checksum(field: &mut [u8], value: u64) {
    let encoded = format!("{value:06o}\0 ");
    field.copy_from_slice(encoded.as_bytes());
}

fn manifest(bytes: &[u8], unpacked_bytes: u64) -> VoiceModelManifest {
    VoiceModelManifest {
        id: "sherpa-stt-en-v1".into(),
        kind: VoiceModelKind::SpeechToText,
        url: "fixture://model.zip".into(),
        sha256: sha256_hex(bytes),
        unpacked_bytes,
    }
}

#[test]
fn sha256_matches_standard_test_vector() {
    assert_eq!(
        sha256_hex(b"abc"),
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
}

#[tokio::test]
async fn installer_verifies_unpacks_and_atomically_updates_active_pointer() {
    let target = TestDir::new("success");
    let bytes = archive(&[("encoder.onnx", b"encoder"), ("tokens.txt", b"tokens")]);
    let downloader = BytesDownloader {
        bytes: Arc::new(bytes.clone()),
    };

    let installed = install_voice_model_with_downloader(
        &manifest(&bytes, 13),
        target.path(),
        CancellationToken::new(),
        &downloader,
    )
    .await
    .expect("install fixture model");

    assert_eq!(
        fs::read(installed.join("encoder.onnx")).unwrap(),
        b"encoder"
    );
    assert_eq!(fs::read(installed.join("tokens.txt")).unwrap(), b"tokens");
    let active = fs::read_to_string(target.path().join("active")).unwrap();
    assert_eq!(
        target.path().join("versions").join(active.trim()),
        installed
    );
    assert!(!target.path().join(".install.part").exists());
    assert!(!target.path().join(".install.staging").exists());
}

#[tokio::test]
async fn checksum_failure_removes_partial_and_staging_files() {
    let target = TestDir::new("checksum");
    let bytes = archive(&[("model.onnx", b"model")]);
    let downloader = BytesDownloader {
        bytes: Arc::new(bytes.clone()),
    };
    let mut bad_manifest = manifest(&bytes, 5);
    bad_manifest.sha256 = "00".repeat(32);

    let err = install_voice_model_with_downloader(
        &bad_manifest,
        target.path(),
        CancellationToken::new(),
        &downloader,
    )
    .await
    .unwrap_err();
    assert!(matches!(err, VoiceModelError::ChecksumMismatch { .. }));
    assert!(!target.path().join("active").exists());
    assert!(!target.path().join(".install.part").exists());
    assert!(!target.path().join(".install.staging").exists());
}

#[tokio::test]
async fn cancellation_removes_partial_download() {
    let target = TestDir::new("cancel");
    let wrote_partial = Arc::new(AtomicBool::new(false));
    let downloader = Arc::new(CancellingDownloader {
        wrote_partial: wrote_partial.clone(),
    });
    let bytes = archive(&[("model.onnx", b"model")]);
    let cancel = CancellationToken::new();
    let task_cancel = cancel.clone();
    let task_target = target.path().to_path_buf();
    let task_manifest = manifest(&bytes, 5);
    let task_downloader = downloader.clone();
    let task = tokio::spawn(async move {
        install_voice_model_with_downloader(
            &task_manifest,
            &task_target,
            task_cancel,
            task_downloader.as_ref(),
        )
        .await
    });
    while !wrote_partial.load(Ordering::SeqCst) {
        tokio::task::yield_now().await;
    }
    cancel.cancel();

    assert!(matches!(
        task.await.unwrap(),
        Err(VoiceModelError::Cancelled)
    ));
    assert!(!target.path().join(".install.part").exists());
    assert!(!target.path().join(".install.staging").exists());
}

#[tokio::test]
async fn cancellation_unblocks_a_downloader_that_ignores_its_token() {
    let target = TestDir::new("forced-cancel");
    let bytes = archive(&[("model.onnx", b"model")]);
    let cancel = CancellationToken::new();
    let task_cancel = cancel.clone();
    let task_target = target.path().to_path_buf();
    let task_manifest = manifest(&bytes, 5);
    let task = tokio::spawn(async move {
        install_voice_model_with_downloader(
            &task_manifest,
            &task_target,
            task_cancel,
            &IgnoringDownloader,
        )
        .await
    });
    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    cancel.cancel();

    let result = tokio::time::timeout(std::time::Duration::from_millis(200), task)
        .await
        .expect("installer cancellation should unblock")
        .unwrap();
    assert!(matches!(result, Err(VoiceModelError::Cancelled)));
    assert!(!target.path().join(".install.part").exists());
}
