use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Component, Path, PathBuf};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_util::sync::CancellationToken;

const PART_FILE: &str = ".install.part";
const STAGING_DIR: &str = ".install.staging";
const ACTIVE_TMP: &str = ".active.tmp";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VoiceModelKind {
    WakeWord,
    SpeechToText,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VoiceModelManifest {
    pub id: String,
    pub kind: VoiceModelKind,
    pub url: String,
    pub sha256: String,
    pub unpacked_bytes: u64,
}

#[derive(Debug, thiserror::Error)]
pub enum VoiceModelError {
    #[error("model installation cancelled")]
    Cancelled,
    #[error("model download scheme is unavailable in the core crate: {scheme}")]
    DownloadUnavailable { scheme: String },
    #[error("invalid model manifest: {message}")]
    InvalidManifest { message: String },
    #[error("model checksum mismatch: expected {expected}, got {actual}")]
    ChecksumMismatch { expected: String, actual: String },
    #[error("model archive is invalid: {message}")]
    InvalidArchive { message: String },
    #[error("model unpacked size mismatch: expected {expected}, got {actual}")]
    SizeMismatch { expected: u64, actual: u64 },
    #[error("model installer task failed: {message}")]
    Task { message: String },
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

#[async_trait]
pub trait ModelDownloader: Send + Sync {
    async fn download(
        &self,
        url: &str,
        destination: &Path,
        cancel: CancellationToken,
    ) -> Result<(), VoiceModelError>;
}

/// Dependency-free source used by the core crate. Desktop hosts can inject an
/// HTTPS downloader through [`install_voice_model_with_downloader`].
pub struct FileModelDownloader;

#[async_trait]
impl ModelDownloader for FileModelDownloader {
    async fn download(
        &self,
        url: &str,
        destination: &Path,
        cancel: CancellationToken,
    ) -> Result<(), VoiceModelError> {
        let source = if let Some(path) = url.strip_prefix("file://") {
            PathBuf::from(path)
        } else if !url.contains("://") {
            PathBuf::from(url)
        } else {
            let scheme = url.split(':').next().unwrap_or("unknown").to_string();
            return Err(VoiceModelError::DownloadUnavailable { scheme });
        };
        let mut input = tokio::fs::File::open(source).await?;
        let mut output = tokio::fs::File::create(destination).await?;
        let mut buffer = vec![0_u8; 64 * 1024];
        loop {
            let read = tokio::select! {
                biased;
                _ = cancel.cancelled() => return Err(VoiceModelError::Cancelled),
                read = input.read(&mut buffer) => read?,
            };
            if read == 0 {
                break;
            }
            tokio::select! {
                biased;
                _ = cancel.cancelled() => return Err(VoiceModelError::Cancelled),
                result = output.write_all(&buffer[..read]) => result?,
            }
        }
        output.flush().await?;
        output.sync_all().await?;
        Ok(())
    }
}

pub async fn install_voice_model(
    manifest: &VoiceModelManifest,
    target: &Path,
    cancel: CancellationToken,
) -> Result<(), VoiceModelError> {
    install_voice_model_with_downloader(manifest, target, cancel, &FileModelDownloader)
        .await
        .map(|_| ())
}

pub async fn install_voice_model_with_downloader<D: ModelDownloader + ?Sized>(
    manifest: &VoiceModelManifest,
    target: &Path,
    cancel: CancellationToken,
    downloader: &D,
) -> Result<PathBuf, VoiceModelError> {
    validate_manifest(manifest)?;
    tokio::fs::create_dir_all(target).await?;
    let part = target.join(PART_FILE);
    let staging = target.join(STAGING_DIR);
    let active_tmp = target.join(ACTIVE_TMP);
    cleanup_path(&part).await;
    cleanup_path(&staging).await;
    cleanup_path(&active_tmp).await;

    let result = install_inner(
        manifest,
        target,
        &part,
        &staging,
        &active_tmp,
        cancel,
        downloader,
    )
    .await;
    if result.is_err() {
        cleanup_path(&part).await;
        cleanup_path(&staging).await;
        cleanup_path(&active_tmp).await;
    }
    result
}

async fn install_inner<D: ModelDownloader + ?Sized>(
    manifest: &VoiceModelManifest,
    target: &Path,
    part: &Path,
    staging: &Path,
    active_tmp: &Path,
    cancel: CancellationToken,
    downloader: &D,
) -> Result<PathBuf, VoiceModelError> {
    if cancel.is_cancelled() {
        return Err(VoiceModelError::Cancelled);
    }
    let download_cancel = cancel.child_token();
    tokio::select! {
        biased;
        _ = cancel.cancelled() => {
            download_cancel.cancel();
            return Err(VoiceModelError::Cancelled);
        }
        result = downloader.download(&manifest.url, part, download_cancel.clone()) => result?,
    }
    let actual_sha = sha256_file(part, &cancel).await?;
    if !actual_sha.eq_ignore_ascii_case(&manifest.sha256) {
        return Err(VoiceModelError::ChecksumMismatch {
            expected: manifest.sha256.to_ascii_lowercase(),
            actual: actual_sha,
        });
    }
    if cancel.is_cancelled() {
        return Err(VoiceModelError::Cancelled);
    }

    tokio::fs::create_dir_all(staging).await?;
    let archive = part.to_path_buf();
    let destination = staging.to_path_buf();
    let expected_size = manifest.unpacked_bytes;
    let extract_cancel = cancel.clone();
    tokio::task::spawn_blocking(move || {
        extract_tar(&archive, &destination, expected_size, &extract_cancel)
    })
    .await
    .map_err(|error| VoiceModelError::Task {
        message: error.to_string(),
    })??;
    if cancel.is_cancelled() {
        return Err(VoiceModelError::Cancelled);
    }

    let version_id = format!("{}-{}", manifest.id, &actual_sha[..12]);
    let versions = target.join("versions");
    tokio::fs::create_dir_all(&versions).await?;
    let installed = versions.join(&version_id);
    if tokio::fs::metadata(&installed).await.is_ok() {
        cleanup_path(staging).await;
    } else {
        tokio::fs::rename(staging, &installed).await?;
    }
    cleanup_path(part).await;

    let mut pointer = tokio::fs::File::create(active_tmp).await?;
    pointer.write_all(version_id.as_bytes()).await?;
    pointer.write_all(b"\n").await?;
    pointer.flush().await?;
    pointer.sync_all().await?;
    drop(pointer);
    tokio::fs::rename(active_tmp, target.join("active")).await?;
    Ok(installed)
}

fn validate_manifest(manifest: &VoiceModelManifest) -> Result<(), VoiceModelError> {
    if manifest.id.is_empty()
        || !manifest
            .id
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_'))
    {
        return Err(VoiceModelError::InvalidManifest {
            message: "id must contain only ASCII letters, digits, '-' or '_'".into(),
        });
    }
    if manifest.sha256.len() != 64 || !manifest.sha256.bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        return Err(VoiceModelError::InvalidManifest {
            message: "sha256 must be 64 hexadecimal characters".into(),
        });
    }
    Ok(())
}

async fn cleanup_path(path: &Path) {
    let Ok(metadata) = tokio::fs::symlink_metadata(path).await else {
        return;
    };
    if metadata.is_dir() {
        let _ = tokio::fs::remove_dir_all(path).await;
    } else {
        let _ = tokio::fs::remove_file(path).await;
    }
}

async fn sha256_file(path: &Path, cancel: &CancellationToken) -> Result<String, VoiceModelError> {
    let mut file = tokio::fs::File::open(path).await?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0_u8; 64 * 1024];
    loop {
        let read = tokio::select! {
            biased;
            _ = cancel.cancelled() => return Err(VoiceModelError::Cancelled),
            read = file.read(&mut buffer) => read?,
        };
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hex_bytes(&hasher.finalize()))
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hex_bytes(&hasher.finalize())
}

fn hex_bytes(bytes: &[u8; 32]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(64);
    for byte in bytes {
        output.push(char::from(HEX[usize::from(byte >> 4)]));
        output.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    output
}

fn extract_tar(
    archive: &Path,
    destination: &Path,
    expected_size: u64,
    cancel: &CancellationToken,
) -> Result<(), VoiceModelError> {
    let mut archive = std::fs::File::open(archive)?;
    let mut total = 0_u64;
    loop {
        if cancel.is_cancelled() {
            return Err(VoiceModelError::Cancelled);
        }
        let mut header = [0_u8; 512];
        archive
            .read_exact(&mut header)
            .map_err(|error| VoiceModelError::InvalidArchive {
                message: format!("truncated header: {error}"),
            })?;
        if header.iter().all(|byte| *byte == 0) {
            break;
        }
        validate_tar_checksum(&header)?;
        let size = parse_octal(&header[124..136], "size")?;
        let path = tar_path(&header)?;
        let output = destination.join(&path);
        if !safe_relative_path(&path) {
            return Err(VoiceModelError::InvalidArchive {
                message: format!("unsafe archive path `{}`", path.display()),
            });
        }
        match header[156] {
            0 | b'0' => {
                if let Some(parent) = output.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                let mut file = std::fs::File::create(output)?;
                let mut remaining = size;
                let mut buffer = [0_u8; 64 * 1024];
                while remaining > 0 {
                    if cancel.is_cancelled() {
                        return Err(VoiceModelError::Cancelled);
                    }
                    let wanted = usize::try_from(remaining.min(buffer.len() as u64)).unwrap();
                    archive.read_exact(&mut buffer[..wanted]).map_err(|error| {
                        VoiceModelError::InvalidArchive {
                            message: format!("truncated file data: {error}"),
                        }
                    })?;
                    file.write_all(&buffer[..wanted])?;
                    remaining -= wanted as u64;
                }
                total = total
                    .checked_add(size)
                    .ok_or_else(|| VoiceModelError::InvalidArchive {
                        message: "unpacked size overflow".into(),
                    })?;
            }
            b'5' => std::fs::create_dir_all(output)?,
            kind => {
                return Err(VoiceModelError::InvalidArchive {
                    message: format!("unsupported tar entry type {kind}"),
                });
            }
        }
        let padding = (512 - size % 512) % 512;
        archive.seek(SeekFrom::Current(i64::try_from(padding).unwrap()))?;
    }
    if total != expected_size {
        return Err(VoiceModelError::SizeMismatch {
            expected: expected_size,
            actual: total,
        });
    }
    Ok(())
}

fn safe_relative_path(path: &Path) -> bool {
    !path.as_os_str().is_empty()
        && path
            .components()
            .all(|component| matches!(component, Component::Normal(_)))
}

fn tar_path(header: &[u8; 512]) -> Result<PathBuf, VoiceModelError> {
    let name = tar_text(&header[..100], "name")?;
    let prefix = tar_text(&header[345..500], "prefix")?;
    Ok(if prefix.is_empty() {
        PathBuf::from(name)
    } else {
        Path::new(&prefix).join(name)
    })
}

fn tar_text(field: &[u8], name: &str) -> Result<String, VoiceModelError> {
    let end = field
        .iter()
        .position(|byte| *byte == 0)
        .unwrap_or(field.len());
    std::str::from_utf8(&field[..end])
        .map(str::to_owned)
        .map_err(|error| VoiceModelError::InvalidArchive {
            message: format!("invalid {name}: {error}"),
        })
}

fn parse_octal(field: &[u8], name: &str) -> Result<u64, VoiceModelError> {
    let text = tar_text(field, name)?;
    let text = text.trim_matches(|character: char| character == '\0' || character == ' ');
    if text.is_empty() {
        return Ok(0);
    }
    u64::from_str_radix(text, 8).map_err(|error| VoiceModelError::InvalidArchive {
        message: format!("invalid {name}: {error}"),
    })
}

fn validate_tar_checksum(header: &[u8; 512]) -> Result<(), VoiceModelError> {
    let expected = parse_octal(&header[148..156], "checksum")?;
    let actual: u64 = header
        .iter()
        .enumerate()
        .map(|(index, byte)| {
            if (148..156).contains(&index) {
                u64::from(b' ')
            } else {
                u64::from(*byte)
            }
        })
        .sum();
    if expected != actual {
        return Err(VoiceModelError::InvalidArchive {
            message: format!("tar checksum mismatch: expected {expected}, got {actual}"),
        });
    }
    Ok(())
}

struct Sha256 {
    state: [u32; 8],
    block: [u8; 64],
    block_len: usize,
    total_len: u64,
}

impl Sha256 {
    fn new() -> Self {
        Self {
            state: [
                0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
                0x5be0cd19,
            ],
            block: [0; 64],
            block_len: 0,
            total_len: 0,
        }
    }

    fn update(&mut self, mut input: &[u8]) {
        self.total_len = self.total_len.wrapping_add(input.len() as u64);
        if self.block_len > 0 {
            let take = (64 - self.block_len).min(input.len());
            self.block[self.block_len..self.block_len + take].copy_from_slice(&input[..take]);
            self.block_len += take;
            input = &input[take..];
            if self.block_len == 64 {
                let block = self.block;
                self.compress(&block);
                self.block_len = 0;
            }
        }
        while input.len() >= 64 {
            let block: &[u8; 64] = input[..64].try_into().unwrap();
            self.compress(block);
            input = &input[64..];
        }
        self.block[..input.len()].copy_from_slice(input);
        self.block_len = input.len();
    }

    fn finalize(mut self) -> [u8; 32] {
        let bit_len = self.total_len.wrapping_mul(8);
        self.block[self.block_len] = 0x80;
        self.block_len += 1;
        if self.block_len > 56 {
            self.block[self.block_len..].fill(0);
            let block = self.block;
            self.compress(&block);
            self.block = [0; 64];
        } else {
            self.block[self.block_len..56].fill(0);
        }
        self.block[56..].copy_from_slice(&bit_len.to_be_bytes());
        let block = self.block;
        self.compress(&block);
        let mut output = [0_u8; 32];
        for (chunk, value) in output.chunks_exact_mut(4).zip(self.state) {
            chunk.copy_from_slice(&value.to_be_bytes());
        }
        output
    }

    fn compress(&mut self, block: &[u8; 64]) {
        const K: [u32; 64] = [
            0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4,
            0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe,
            0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f,
            0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
            0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc,
            0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b,
            0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116,
            0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
            0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
            0xc67178f2,
        ];
        let mut words = [0_u32; 64];
        for (index, chunk) in block.chunks_exact(4).enumerate() {
            words[index] = u32::from_be_bytes(chunk.try_into().unwrap());
        }
        for index in 16..64 {
            let s0 = words[index - 15].rotate_right(7)
                ^ words[index - 15].rotate_right(18)
                ^ (words[index - 15] >> 3);
            let s1 = words[index - 2].rotate_right(17)
                ^ words[index - 2].rotate_right(19)
                ^ (words[index - 2] >> 10);
            words[index] = words[index - 16]
                .wrapping_add(s0)
                .wrapping_add(words[index - 7])
                .wrapping_add(s1);
        }
        let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut h] = self.state;
        for index in 0..64 {
            let big_s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let choice = (e & f) ^ ((!e) & g);
            let temp1 = h
                .wrapping_add(big_s1)
                .wrapping_add(choice)
                .wrapping_add(K[index])
                .wrapping_add(words[index]);
            let big_s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let majority = (a & b) ^ (a & c) ^ (b & c);
            let temp2 = big_s0.wrapping_add(majority);
            h = g;
            g = f;
            f = e;
            e = d.wrapping_add(temp1);
            d = c;
            c = b;
            b = a;
            a = temp1.wrapping_add(temp2);
        }
        for (state, value) in self.state.iter_mut().zip([a, b, c, d, e, f, g, h]) {
            *state = state.wrapping_add(value);
        }
    }
}
