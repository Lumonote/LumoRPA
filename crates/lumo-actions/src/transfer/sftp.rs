//! OpenSSH SFTP in non-interactive key-only mode. No shell, agent, user SSH
//! configuration, automatic host-key acceptance, password argv or prompt fallback.

use async_trait::async_trait;
use lumo_core::error::StepError;
use lumo_core::{Action, ActionRegistry, ActionResult, StepCtx};
use once_cell::sync::Lazy;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;
use tokio::io::AsyncWriteExt;
use tokio::process::{Child, Command};

pub fn register(registry: &mut ActionRegistry) {
    registry.register(Upload);
    registry.register(Download);
}

fn port() -> u16 {
    22
}
pub(super) fn timeout() -> u64 {
    60_000
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct Input {
    host: String,
    #[serde(default = "port")]
    port: u16,
    username: String,
    /// OpenSSH private key file. Encrypted keys requiring a prompt are rejected.
    identity_file: PathBuf,
    /// Independently verified OpenSSH known_hosts file. Unknown/changed keys fail.
    known_hosts: PathBuf,
    remote_path: String,
    local_path: PathBuf,
    #[serde(default = "super::default_max_bytes")]
    max_bytes: u64,
    #[serde(default = "timeout")]
    timeout_ms: u64,
}

pub struct Upload;
pub struct Download;

macro_rules! action {
    ($kind:ident, $id:literal, $upload:literal, $description:literal) => {
        #[async_trait]
        impl Action for $kind {
            fn id(&self) -> &'static str {
                $id
            }
            fn summary(&self) -> &'static str {
                $description
            }
            fn schema(&self) -> &'static Value {
                static SCHEMA: Lazy<Value> = Lazy::new(crate::schema::derive::<Input>);
                &SCHEMA
            }
            async fn execute(
                &self,
                ctx: &mut StepCtx,
                input: Value,
            ) -> Result<ActionResult, StepError> {
                let input: Input = serde_json::from_value(input)
                    .map_err(|e| StepError::msg(format!("{} input invalid: {e}", $id)))?;
                transfer(ctx, input, $upload).await
            }
        }
    };
}
action!(
    Upload,
    "sftp.upload",
    true,
    "Upload using OpenSSH key authentication and a pinned known_hosts file"
);
action!(
    Download,
    "sftp.download",
    false,
    "Atomically download using OpenSSH key authentication and pinned host keys"
);

fn validate(input: &Input) -> Result<(), StepError> {
    let host = input.host.trim_matches(['[', ']']);
    let valid_host = !host.is_empty()
        && !host.starts_with('-')
        && (host.parse::<std::net::IpAddr>().is_ok()
            || host
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-')));
    if !valid_host || input.port == 0 {
        return Err(StepError::msg("sftp: invalid host or port"));
    }
    if input.username.is_empty()
        || !input
            .username
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
    {
        return Err(StepError::msg(
            "sftp: username must contain only letters, digits, _, - or .",
        ));
    }
    if input.remote_path.is_empty() || input.remote_path.chars().any(char::is_control) {
        return Err(StepError::msg(
            "sftp: remote_path must be nonempty and contain no control characters",
        ));
    }
    if input.max_bytes == 0 || !(1..=86_400_000).contains(&input.timeout_ms) {
        return Err(StepError::msg(
            "sftp: max_bytes must be positive and timeout_ms must be 1..86400000",
        ));
    }
    Ok(())
}

async fn transfer(ctx: &StepCtx, input: Input, upload: bool) -> Result<ActionResult, StepError> {
    validate(&input)?;
    if ctx.is_cancelled() {
        return Err(StepError::msg("sftp transfer cancelled"));
    }
    if upload {
        ctx.ensure_fs_read(&input.local_path)?;
    } else {
        ctx.ensure_fs_write(&input.local_path)?;
    }
    ctx.ensure_fs_read(&input.identity_file)?;
    ctx.ensure_fs_read(&input.known_hosts)?;
    let host = input.host.trim_matches(['[', ']']);
    let url_host = if host.contains(':') {
        format!("[{host}]")
    } else {
        host.to_owned()
    };
    ctx.ensure_network_url(&format!("sftp://{url_host}:{}", input.port))?;
    let identity = regular_file(&input.identity_file, "identity_file").await?;
    let known_hosts = regular_file(&input.known_hosts, "known_hosts").await?;
    if tokio::fs::metadata(&known_hosts)
        .await
        .map_err(io_error)?
        .len()
        == 0
    {
        return Err(StepError::msg(
            "sftp: known_hosts is empty; add an independently verified host key before connecting",
        ));
    }
    let local = if upload {
        let local = regular_file(&input.local_path, "local_path").await?;
        if tokio::fs::metadata(&local).await.map_err(io_error)?.len() > input.max_bytes {
            return Err(StepError::msg("sftp.upload: file exceeds max_bytes"));
        }
        local
    } else {
        absolute_destination(&input.local_path).await?
    };
    // These canonical paths are the exact objects passed to OpenSSH. Re-check
    // them after canonicalization so a symlink cannot widen a grant.
    ctx.ensure_fs_read(&identity)?;
    ctx.ensure_fs_read(&known_hosts)?;
    if upload {
        ctx.ensure_fs_read(&local)?;
    } else {
        ctx.ensure_fs_write(&local)?;
    }
    let staging = if upload {
        None
    } else {
        Some(LocalStage::create(&local).await?)
    };
    if let Some(stage) = &staging {
        ctx.ensure_fs_write(&stage.path)?;
    }
    let remote_temporary = format!("{}.lumo-{}", input.remote_path, uuid::Uuid::new_v4());
    let batch = if upload {
        format!(
            "put {} {}\nrename {} {}\n",
            quote_path(path_text(&local)?)?,
            quote_path(&remote_temporary)?,
            quote_path(&remote_temporary)?,
            quote_path(&input.remote_path)?
        )
    } else {
        format!(
            "get {} {}\n",
            quote_path(&input.remote_path)?,
            quote_path(path_text(&staging.as_ref().expect("download stage").path)?)?
        )
    };
    let mut command = Command::new("sftp");
    command
        .args([
            "-q",
            "-F",
            "none",
            "-b",
            "-",
            "-P",
            &input.port.to_string(),
            "-R",
            "1",
            "-B",
            "32768",
        ])
        .args([
            "-oBatchMode=yes",
            "-oIdentitiesOnly=yes",
            "-oStrictHostKeyChecking=yes",
            "-oGlobalKnownHostsFile=none",
            "-oIdentityAgent=none",
            "-oAddKeysToAgent=no",
            "-oPasswordAuthentication=no",
            "-oKbdInteractiveAuthentication=no",
            "-oPreferredAuthentications=publickey",
            "-oProxyCommand=none",
            "-oProxyJump=none",
            "-oClearAllForwardings=yes",
            "-oPermitLocalCommand=no",
            "-oUpdateHostKeys=no",
            "-oConnectionAttempts=1",
            "-oConnectTimeout=10",
            "-oLogLevel=ERROR",
        ])
        .arg(format!("-oUser={}", input.username))
        .arg(format!(
            "-oUserKnownHostsFile={}",
            ssh_option_path(&known_hosts)?
        ))
        .arg("-i")
        .arg(&identity)
        .arg(host)
        .env_remove("SSH_AUTH_SOCK")
        .env_remove("SSH_ASKPASS")
        .env("SSH_ASKPASS_REQUIRE", "never")
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.as_std_mut().process_group(0);
    }
    let child = command.spawn().map_err(|e| {
        StepError::msg(format!(
            "sftp: cannot start system OpenSSH sftp ({e}); install openssh-client"
        ))
    })?;
    let mut process = ChildGuard {
        pid: child.id(),
        child: Some(child),
        cleanup: staging.as_ref().map(|stage| stage.path.clone()),
    };
    let child = process.child.as_mut().expect("owned child");
    let child_pid = child.id();
    let mut stdin = child.stdin.take().expect("piped batch input");
    tokio::time::timeout(
        Duration::from_millis(input.timeout_ms),
        stdin.write_all(batch.as_bytes()),
    )
    .await
    .map_err(|_| StepError::msg("sftp transfer timed out while submitting batch"))?
    .map_err(io_error)?;
    drop(stdin);
    let started = tokio::time::Instant::now();
    let cancel = ctx.cancel_token();
    let outcome = loop {
        if let Some(status) = child.try_wait().map_err(io_error)? {
            break Ok(status);
        }
        if cancel.as_ref().is_some_and(|token| token.is_cancelled()) {
            break Err(StepError::msg("sftp transfer cancelled"));
        }
        if started.elapsed() >= Duration::from_millis(input.timeout_ms) {
            break Err(StepError::msg("sftp transfer timed out"));
        }
        if let Some(stage) = &staging {
            if tokio::fs::metadata(&stage.path)
                .await
                .map_err(io_error)?
                .len()
                > input.max_bytes
            {
                break Err(StepError::msg("sftp.download: response exceeds max_bytes"));
            }
        } else if tokio::fs::metadata(&local).await.map_err(io_error)?.len() > input.max_bytes {
            break Err(StepError::msg("sftp.upload: source grew beyond max_bytes"));
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    };
    match outcome {
        Ok(status) if status.success() => {},
        Ok(_) => return Err(StepError::msg("sftp transfer failed; verify the pinned host key, key authentication, remote path and server permissions")),
        Err(error) => { terminate_tree(child_pid); let _ = child.kill().await; let _ = child.wait().await; return Err(error); }
    }
    process.pid = None;
    let bytes = if let Some(stage) = staging {
        let bytes = tokio::fs::metadata(&stage.path)
            .await
            .map_err(io_error)?
            .len();
        if bytes > input.max_bytes {
            return Err(StepError::msg("sftp.download: response exceeds max_bytes"));
        }
        stage.publish(&local).await?;
        bytes
    } else {
        let bytes = tokio::fs::metadata(&local).await.map_err(io_error)?.len();
        if bytes > input.max_bytes {
            return Err(StepError::msg(
                "sftp.upload: source changed beyond max_bytes during transfer",
            ));
        }
        bytes
    };
    Ok(ActionResult::from(
        json!({"host": host, "remote_path": input.remote_path, "local_path": local, "bytes": bytes, "host_key_verified": true}),
    ))
}

struct ChildGuard {
    pid: Option<u32>,
    child: Option<Child>,
    cleanup: Option<PathBuf>,
}
impl Drop for ChildGuard {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            terminate_tree(self.pid);
            let cleanup = self.cleanup.take();
            let _ = child.start_kill();
            // VM cancellation drops the action future. Reap the owned child on
            // the runtime rather than leaving a zombie or authenticated session.
            if let Ok(runtime) = tokio::runtime::Handle::try_current() {
                runtime.spawn(async move {
                    let _ = child.wait().await;
                    if let Some(path) = cleanup {
                        let _ = tokio::fs::remove_file(path).await;
                    }
                });
            }
        }
    }
}

pub(super) struct LocalStage {
    pub(super) path: PathBuf,
}
impl LocalStage {
    pub(super) async fn create(destination: &Path) -> Result<Self, StepError> {
        let parent = destination
            .parent()
            .ok_or_else(|| StepError::msg("sftp: destination has no parent"))?;
        let path = parent.join(format!(".lumo-sftp-{}.part", uuid::Uuid::new_v4()));
        let mut options = tokio::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        options.mode(0o600);
        options.open(&path).await.map_err(io_error)?;
        Ok(Self { path })
    }
    pub(super) async fn publish(self, destination: &Path) -> Result<(), StepError> {
        // rename on the same filesystem preserves the old destination on failure.
        tokio::fs::rename(&self.path, destination)
            .await
            .map_err(io_error)
    }
}
impl Drop for LocalStage {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

async fn regular_file(path: &Path, label: &str) -> Result<PathBuf, StepError> {
    let canonical = tokio::fs::canonicalize(path)
        .await
        .map_err(|e| StepError::msg(format!("sftp {label}: {e}")))?;
    if !tokio::fs::metadata(&canonical)
        .await
        .map_err(io_error)?
        .is_file()
    {
        return Err(StepError::msg(format!(
            "sftp {label} must be a regular file"
        )));
    }
    Ok(canonical)
}
pub(super) async fn absolute_destination(path: &Path) -> Result<PathBuf, StepError> {
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let parent = tokio::fs::canonicalize(parent).await.map_err(io_error)?;
    let name = path
        .file_name()
        .ok_or_else(|| StepError::msg("sftp: destination must name a file"))?;
    let destination = parent.join(name);
    if tokio::fs::symlink_metadata(&destination)
        .await
        .is_ok_and(|m| m.file_type().is_symlink())
    {
        return Err(StepError::msg("sftp: destination cannot be a symlink"));
    }
    Ok(destination)
}
fn path_text(path: &Path) -> Result<&str, StepError> {
    path.to_str()
        .ok_or_else(|| StepError::msg("sftp: paths must be valid UTF-8"))
}
fn ssh_option_path(path: &Path) -> Result<String, StepError> {
    let text = path_text(path)?;
    if text.chars().any(char::is_control) {
        return Err(StepError::msg(
            "sftp: paths must contain no control characters",
        ));
    }
    Ok(format!(
        "\"{}\"",
        text.replace('\\', "\\\\").replace('"', "\\\"")
    ))
}

fn terminate_tree(pid: Option<u32>) {
    let Some(pid) = pid else {
        return;
    };
    #[cfg(unix)]
    {
        extern "C" {
            fn kill(pid: i32, signal: i32) -> i32;
        }
        // The sftp/ssh process tree owns a fresh group, never the app's group.
        unsafe {
            kill(-(pid as i32), 9);
        }
    }
    #[cfg(windows)]
    {
        let _ = std::process::Command::new("taskkill")
            .args(["/PID", &pid.to_string(), "/T", "/F"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
}
fn quote_path(path: &str) -> Result<String, StepError> {
    if path.is_empty() || path.chars().any(char::is_control) {
        return Err(StepError::msg(
            "sftp: paths must contain no control characters",
        ));
    }
    let path = if path.starts_with('-') {
        format!("./{path}")
    } else {
        path.to_owned()
    };
    let mut quoted = String::from("\"");
    for c in path.chars() {
        if matches!(c, '\\' | '"' | '*' | '?' | '[' | ']') {
            quoted.push('\\');
        }
        quoted.push(c);
    }
    quoted.push('"');
    Ok(quoted)
}
fn io_error(error: std::io::Error) -> StepError {
    StepError::msg(format!("sftp I/O: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn batch_paths_cannot_inject_commands_or_options() {
        assert!(quote_path("file\n!rm -rf /").is_err());
        assert_eq!(quote_path("-r").unwrap(), "\"./-r\"");
        assert_eq!(quote_path("a b\"c*").unwrap(), "\"a b\\\"c\\*\"");
    }
}
