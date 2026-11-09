//! Editor recovery data lives separately from runnable flow files. SQLite
//! commits provide atomic replacement on every supported desktop platform.
use super::{app_home, flow_read_roots, AppHandle};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    path::{Component, Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Mutex, OnceLock,
    },
};
use tauri::Emitter;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct EditorDraft {
    pub version: u32,
    pub path: Option<String>,
    pub source: String,
    pub base_source: String,
    pub updated_at: i64,
}

struct DraftStore {
    home: PathBuf,
    roots: Vec<PathBuf>,
}
impl DraftStore {
    fn new(home: PathBuf, roots: Vec<PathBuf>) -> Self {
        let roots = roots
            .into_iter()
            .map(|root| root.canonicalize().unwrap_or(root))
            .collect();
        Self { home, roots }
    }
    fn connection(&self) -> Result<rusqlite::Connection, String> {
        let path = self.home.join("editor-drafts.sqlite3");
        let connection = rusqlite::Connection::open(&path).map_err(|error| error.to_string())?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))
                .map_err(|error| error.to_string())?;
        }
        connection
            .busy_timeout(std::time::Duration::from_secs(5))
            .map_err(|error| error.to_string())?;
        connection
            .execute_batch(
                "CREATE TABLE IF NOT EXISTS drafts (id TEXT PRIMARY KEY, body TEXT NOT NULL)",
            )
            .map_err(|error| error.to_string())?;
        Ok(connection)
    }
    fn validate_path(&self, path: Option<&str>) -> Result<(), String> {
        let Some(path) = path else {
            return Ok(());
        };
        let path = Path::new(path);
        if !path.is_absolute()
            || path
                .components()
                .any(|part| matches!(part, Component::ParentDir | Component::CurDir))
        {
            return Err("draft path must be an absolute flow path without traversal".into());
        }
        let existing = path
            .ancestors()
            .find(|parent| parent.exists())
            .ok_or("draft path has no existing ancestor")?;
        let canonical = existing.canonicalize().map_err(|error| error.to_string())?;
        if self.roots.iter().any(|root| canonical.starts_with(root)) {
            Ok(())
        } else {
            Err("draft path is outside the flow library".into())
        }
    }
    fn validate(&self, draft: &EditorDraft) -> Result<(), String> {
        if draft.version != 1
            || draft.source.len().saturating_add(draft.base_source.len()) > 8 * 1024 * 1024
        {
            return Err("unsupported or oversized editor draft".into());
        }
        self.validate_path(draft.path.as_deref())
    }
    fn key(path: Option<&str>) -> String {
        format!(
            "{:x}",
            Sha256::digest(path.unwrap_or("__untitled__").as_bytes())
        )
    }
    fn upsert(&self, draft: &EditorDraft) -> Result<(), String> {
        self.validate(draft)?;
        let body = serde_json::to_string(draft).map_err(|error| error.to_string())?;
        self.connection()?.execute("INSERT INTO drafts(id,body) VALUES (?,?) ON CONFLICT(id) DO UPDATE SET body=excluded.body", rusqlite::params![Self::key(draft.path.as_deref()),body]).map_err(|error| error.to_string())?;
        Ok(())
    }
    fn list(&self) -> Result<Vec<EditorDraft>, String> {
        let connection = self.connection()?;
        let mut statement = connection
            .prepare("SELECT id,body FROM drafts")
            .map_err(|error| error.to_string())?;
        let rows = statement
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
            .map_err(|error| error.to_string())?;
        let mut drafts = Vec::new();
        for row in rows {
            let (key, body) = row.map_err(|error| error.to_string())?;
            let draft: EditorDraft = serde_json::from_str(&body)
                .map_err(|error| format!("草稿损坏，原数据已保留：{error}"))?;
            self.validate(&draft)?;
            if key != Self::key(draft.path.as_deref()) {
                return Err("draft identity does not match its path".into());
            }
            drafts.push(draft);
        }
        drafts.sort_by_key(|draft| std::cmp::Reverse(draft.updated_at));
        Ok(drafts)
    }
    fn delete(&self, path: Option<&str>) -> Result<(), String> {
        self.validate_path(path)?;
        self.connection()?
            .execute("DELETE FROM drafts WHERE id=?", [Self::key(path)])
            .map_err(|error| error.to_string())?;
        Ok(())
    }
}
fn store(app: &AppHandle) -> Result<DraftStore, String> {
    Ok(DraftStore::new(app_home(app)?, flow_read_roots(app)))
}
#[tauri::command]
pub(super) fn editor_draft_list(app: AppHandle) -> Result<Vec<EditorDraft>, String> {
    store(&app)?.list()
}
#[tauri::command]
pub(super) fn editor_draft_upsert(app: AppHandle, draft: EditorDraft) -> Result<(), String> {
    store(&app)?.upsert(&draft)
}
#[tauri::command]
pub(super) fn editor_draft_delete(app: AppHandle, path: Option<String>) -> Result<(), String> {
    store(&app)?.delete(path.as_deref())
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct EditorExitRequest {
    request_id: String,
    kind: String,
}
#[derive(Default)]
struct EditorExitRuntime {
    pending: Option<EditorExitRequest>,
}
impl EditorExitRuntime {
    fn request(&mut self, kind: &str) -> EditorExitRequest {
        self.pending
            .get_or_insert_with(|| EditorExitRequest {
                request_id: ulid::Ulid::new().to_string(),
                kind: kind.into(),
            })
            .clone()
    }
    fn confirm(&mut self, request_id: &str) -> Result<String, String> {
        if self
            .pending
            .as_ref()
            .is_none_or(|request| request.request_id != request_id)
        {
            return Err("exit request is stale".into());
        }
        Ok(self.pending.take().expect("checked request").kind)
    }
    fn cancel(&mut self, request_id: &str) {
        if self
            .pending
            .as_ref()
            .is_some_and(|request| request.request_id == request_id)
        {
            self.pending = None;
        }
    }
}
static EXIT_STATE: OnceLock<Mutex<EditorExitRuntime>> = OnceLock::new();
static EXIT_ALLOWED: AtomicBool = AtomicBool::new(false);
pub(super) fn consume_exit_permission() -> bool {
    EXIT_ALLOWED.swap(false, Ordering::SeqCst)
}
pub(super) fn request_editor_exit(app: &AppHandle, kind: &str) -> Result<(), String> {
    let request = EXIT_STATE
        .get_or_init(Default::default)
        .lock()
        .map_err(|_| "exit state unavailable")?
        .request(kind);
    super::show_main_window(app);
    app.emit_to("main", "lumo://editor-exit-request", request)
        .map_err(|error| error.to_string())
}
#[tauri::command]
pub(super) fn editor_confirm_exit(app: AppHandle, request_id: String) -> Result<(), String> {
    let kind = EXIT_STATE
        .get_or_init(Default::default)
        .lock()
        .map_err(|_| "exit state unavailable")?
        .confirm(&request_id)?;
    EXIT_ALLOWED.store(true, Ordering::SeqCst);
    if kind == "restart" {
        app.request_restart();
    } else {
        app.exit(0);
    }
    Ok(())
}
#[tauri::command]
pub(super) fn editor_cancel_exit(request_id: String) -> Result<(), String> {
    EXIT_STATE
        .get_or_init(Default::default)
        .lock()
        .map_err(|_| "exit state unavailable")?
        .cancel(&request_id);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn entry(path: Option<&std::path::Path>, source: &str) -> EditorDraft {
        EditorDraft {
            version: 1,
            path: path.map(|path| path.to_string_lossy().into_owned()),
            source: source.into(),
            base_source: "disk".into(),
            updated_at: 10,
        }
    }
    #[test]
    fn invalid_yaml_round_trips_atomically_and_missing_original_is_recoverable() {
        let root = tempfile::tempdir().unwrap();
        let store = DraftStore::new(root.path().into(), vec![root.path().into()]);
        let path = root.path().join("removed/flow.yaml");
        store.upsert(&entry(Some(&path), "spec: [")).unwrap();
        assert_eq!(store.list().unwrap()[0].source, "spec: [");
        store.upsert(&entry(Some(&path), "newest")).unwrap();
        assert_eq!(store.list().unwrap().len(), 1);
        assert_eq!(store.list().unwrap()[0].source, "newest");
        store.delete(Some(path.to_str().unwrap())).unwrap();
        assert!(store.list().unwrap().is_empty());
    }
    #[test]
    fn outside_paths_and_corrupt_drafts_fail_without_deleting_existing_data() {
        let root = tempfile::tempdir().unwrap();
        let store = DraftStore::new(root.path().into(), vec![root.path().into()]);
        assert!(store
            .upsert(&entry(
                Some(std::path::Path::new("/outside/flow.yaml")),
                "bad"
            ))
            .is_err());
        assert!(store
            .upsert(&entry(Some(&root.path().join("../escape.yaml")), "bad"))
            .is_err());
        store.upsert(&entry(None, "untitled")).unwrap();
        store
            .connection()
            .unwrap()
            .execute("UPDATE drafts SET body='{broken'", [])
            .unwrap();
        assert!(store.list().is_err());
        assert_eq!(
            store
                .connection()
                .unwrap()
                .query_row("SELECT body FROM drafts", [], |row| row.get::<_, String>(0))
                .unwrap(),
            "{broken"
        );
    }
    #[test]
    fn exit_handshake_requires_the_matching_request_and_is_consumed_once() {
        let mut exit = EditorExitRuntime::default();
        let first = exit.request("quit");
        assert_eq!(exit.request("quit").request_id, first.request_id);
        assert!(exit.confirm("wrong").is_err());
        assert_eq!(exit.confirm(&first.request_id).unwrap(), "quit");
        assert!(exit.confirm(&first.request_id).is_err());
        let second = exit.request("restart");
        exit.cancel(&second.request_id);
        assert!(exit.confirm(&second.request_id).is_err());
    }
}
