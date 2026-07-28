//! Path & directory helpers: LUMO_HOME layout, P0-3 path-sandbox
//! confinement for webview-driven reads/writes, and OS reveal-in-file-manager.
//! Pure move out of `lib.rs`; semantics unchanged.

use super::*;


pub(crate) fn reveal_path(path: &Path) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    let mut command = {
        let mut c = Command::new("open");
        c.arg("-R").arg(path);
        c
    };

    #[cfg(target_os = "windows")]
    let mut command = {
        let mut c = Command::new("explorer");
        c.arg(format!("/select,{}", path.display()));
        c
    };

    #[cfg(all(not(target_os = "macos"), not(target_os = "windows")))]
    let mut command = {
        let mut c = Command::new("xdg-open");
        c.arg(path.parent().unwrap_or(path));
        c
    };

    let status = command
        .status()
        .map_err(|e| format!("open file location for {}: {e}", path.display()))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!(
            "open file location for {} failed with status {status}",
            path.display()
        ))
    }
}

pub(crate) fn app_home(app: &AppHandle) -> Result<PathBuf, String> {
    let dir = app.path().app_data_dir().map_err(|e| e.to_string())?;
    std::fs::create_dir_all(&dir).map_err(|e| format!("create {}: {e}", dir.display()))?;
    Ok(dir)
}

/// Roots the webview is allowed to *read* flow files from: the user's
/// LUMO_HOME (user flows + recordings + artifacts) and the read-only bundled
/// examples directory. Each is canonicalized; unreadable roots are skipped (P0-3).
pub(crate) fn flow_read_roots(app: &AppHandle) -> Vec<PathBuf> {
    let mut roots = Vec::new();
    if let Ok(home) = app_home(app) {
        if let Ok(canon) = home.canonicalize() {
            roots.push(canon);
        }
    }
    if let Some(ex) = examples_dir(app) {
        if let Ok(canon) = ex.canonicalize() {
            roots.push(canon);
        }
    }
    roots
}

/// Canonicalize `requested` and confirm it resolves inside one of `roots`
/// (each already canonicalized). The path must exist. Confines webview-driven
/// file *reads* to the flow library + examples so a crafted `..`/symlink path
/// can't exfiltrate arbitrary files (P0-3).
pub(crate) fn resolve_within(requested: &str, roots: &[PathBuf]) -> Result<PathBuf, String> {
    let canonical = Path::new(requested)
        .canonicalize()
        .map_err(|e| format!("resolve {requested}: {e}"))?;
    if roots.iter().any(|root| canonical.starts_with(root)) {
        Ok(canonical)
    } else {
        Err(format!(
            "refused: {} is outside the allowed flow directories",
            canonical.display()
        ))
    }
}

/// Resolve a *write* target for `requested`, confining it to `home`
/// (LUMO_HOME). The file need not exist yet, so its parent directory is
/// canonicalized (it must exist and resolve under `home`) and the file name is
/// re-appended. Bundled examples live outside `home` and are thus read-only (P0-3).
pub(crate) fn resolve_write_within(requested: &str, home: &Path) -> Result<PathBuf, String> {
    let requested_path = Path::new(requested);
    let file_name = requested_path
        .file_name()
        .ok_or_else(|| format!("invalid write path: {requested}"))?;
    let parent = requested_path.parent().unwrap_or_else(|| Path::new(""));
    let home_canon = home
        .canonicalize()
        .map_err(|e| format!("resolve LUMO_HOME: {e}"))?;
    let parent_canon = if parent.as_os_str().is_empty() {
        home_canon.clone()
    } else {
        parent
            .canonicalize()
            .map_err(|e| format!("resolve {}: {e}", parent.display()))?
    };
    if !parent_canon.starts_with(&home_canon) {
        return Err(format!(
            "refused: {} is outside LUMO_HOME",
            parent_canon.display()
        ));
    }
    Ok(parent_canon.join(file_name))
}

/// A `FlowSummary` for a path the webview isn't allowed to read — surfaced as
/// an invalid entry instead of leaking file metadata for arbitrary paths (P0-3).
pub(crate) fn refused_summary(path: &str, reason: String) -> FlowSummary {
    FlowSummary {
        path: path.to_string(),
        file_name: Path::new(path)
            .file_name()
            .and_then(|s| s.to_str())
            .map(str::to_string)
            .unwrap_or_default(),
        valid: false,
        error: Some(reason),
        ..Default::default()
    }
}

pub(crate) fn open_repo(app: &AppHandle) -> Result<Repo, String> {
    Repo::open(app_home(app)?.join("lumo.db")).map_err(|e| e.to_string())
}

pub(crate) fn examples_dir(app: &AppHandle) -> Option<PathBuf> {
    if let Ok(resource_dir) = app.path().resource_dir() {
        let bundled = resource_dir.join("examples");
        if bundled.exists() {
            return Some(bundled);
        }
    }
    let dev = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../examples");
    if dev.exists() {
        return Some(dev);
    }
    None
}

/// User-owned flows. Lives under `$LUMO_HOME/flows`, created on first save.
pub(crate) fn user_flows_dir(app: &AppHandle) -> Result<PathBuf, String> {
    let dir = app_home(app)?.join("flows");
    std::fs::create_dir_all(&dir).map_err(|e| format!("create {}: {e}", dir.display()))?;
    Ok(dir)
}

/// Recorder output drop zone. Each `recorder_stop_and_save` call writes one
/// `.lumoflow.yaml` here so the user can pick it up from the library.
pub(crate) fn recordings_dir(app: &AppHandle) -> Result<PathBuf, String> {
    let dir = app_home(app)?.join("recordings");
    std::fs::create_dir_all(&dir).map_err(|e| format!("create {}: {e}", dir.display()))?;
    Ok(dir)
}

pub(crate) fn exports_dir(app: &AppHandle) -> Result<PathBuf, String> {
    let dir = app_home(app)?.join("exports");
    std::fs::create_dir_all(&dir).map_err(|e| format!("create {}: {e}", dir.display()))?;
    Ok(dir)
}

#[cfg(test)]
mod path_sandbox_tests {
    //! P0-3: the webview file IPC must confine reads to the flow library +
    //! bundled examples and confine writes to LUMO_HOME, so a crafted path
    //! (`../`, absolute, symlink) can't exfiltrate or tamper with arbitrary
    //! files on disk.
    use super::{resolve_within, resolve_write_within};
    use std::fs;

    #[test]
    fn resolve_within_allows_file_inside_root() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        let f = root.join("a.lumoflow.yaml");
        fs::write(&f, "x").unwrap();
        let got = resolve_within(f.to_str().unwrap(), std::slice::from_ref(&root)).unwrap();
        assert!(got.starts_with(&root));
    }

    #[test]
    fn resolve_within_rejects_dotdot_escape() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("flows");
        fs::create_dir_all(&root).unwrap();
        let outside = tmp.path().join("secret.txt");
        fs::write(&outside, "secret").unwrap();
        let root_canon = root.canonicalize().unwrap();
        let escape = root.join("../secret.txt");
        let err = resolve_within(escape.to_str().unwrap(), &[root_canon]).unwrap_err();
        assert!(err.contains("outside"), "got: {err}");
    }

    #[test]
    fn resolve_within_rejects_nonexistent_path() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        let err = resolve_within(root.join("nope.yaml").to_str().unwrap(), &[root]).unwrap_err();
        assert!(err.contains("resolve"), "got: {err}");
    }

    #[test]
    fn resolve_write_within_allows_new_file_under_home() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().canonicalize().unwrap();
        let flows = home.join("flows");
        fs::create_dir_all(&flows).unwrap();
        // target does not exist yet — must still resolve via its parent
        let target = flows.join("new.lumoflow.yaml");
        let got = resolve_write_within(target.to_str().unwrap(), &home).unwrap();
        assert_eq!(got, flows.join("new.lumoflow.yaml"));
    }

    #[test]
    fn resolve_write_within_rejects_escape_above_home() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let flows = home.join("flows");
        fs::create_dir_all(&flows).unwrap();
        let home_canon = home.canonicalize().unwrap();
        // home/flows/../../evil.yaml resolves to tmp/evil.yaml — outside home
        let escape = flows.join("../../evil.lumoflow.yaml");
        let err = resolve_write_within(escape.to_str().unwrap(), &home_canon).unwrap_err();
        assert!(err.contains("outside"), "got: {err}");
    }
}
