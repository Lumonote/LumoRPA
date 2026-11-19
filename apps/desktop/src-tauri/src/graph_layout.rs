//! Versioned presentation sidecar, adjacent to a user-owned flow. It is never
//! interpreted by the execution engine and uses the same path confinement.
use super::{app_home, flow_read_roots, resolve_within, resolve_write_within, AppHandle};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

const MAX_BYTES: u64 = 8 * 1024 * 1024;

fn sidecar(flow: &Path) -> PathBuf {
    let mut name = flow.as_os_str().to_os_string();
    name.push(".layout.json");
    PathBuf::from(name)
}

fn validate(layout: &Value) -> Result<(), String> {
    if layout.get("version").and_then(Value::as_u64) != Some(1)
        || !layout.get("positions").is_some_and(Value::is_object)
    {
        return Err("Unsupported graph layout; expected version 1 and positions object".into());
    }
    Ok(())
}

fn read_at(path: &Path) -> Result<Option<Value>, String> {
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.to_string()),
    };
    if metadata.file_type().is_symlink() || !metadata.is_file() || metadata.len() > MAX_BYTES {
        return Err("Graph layout must be a regular file under 8 MiB".into());
    }
    let data = std::fs::read(path).map_err(|error| error.to_string())?;
    let layout: Value = serde_json::from_slice(&data).map_err(|error| error.to_string())?;
    validate(&layout)?;
    Ok(Some(layout))
}

fn write_at(path: &Path, layout: &Value) -> Result<(), String> {
    validate(layout)?;
    let data = serde_json::to_vec_pretty(layout).map_err(|error| error.to_string())?;
    if data.len() as u64 > MAX_BYTES {
        return Err("Graph layout exceeds 8 MiB".into());
    }
    if std::fs::symlink_metadata(path).is_ok_and(|meta| meta.file_type().is_symlink()) {
        return Err("Refused symlink graph layout".into());
    }
    let temporary = path.with_extension(format!("{}.tmp", ulid::Ulid::new()));
    let result = (|| {
        use std::io::Write;
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
            .map_err(|error| error.to_string())?;
        file.write_all(&data).map_err(|error| error.to_string())?;
        file.sync_all().map_err(|error| error.to_string())?;
        drop(file);
        std::fs::rename(&temporary, path).map_err(|error| error.to_string())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    result
}

#[tauri::command]
pub(crate) fn graph_layout_read(app: AppHandle, path: String) -> Result<Value, String> {
    let safe = resolve_within(&path, &flow_read_roots(&app))?;
    let home = app_home(&app)?;
    let writable = resolve_write_within(safe.to_string_lossy().as_ref(), &home).is_ok();
    Ok(json!({ "writable": writable, "layout": read_at(&sidecar(&safe))? }))
}

#[tauri::command]
pub(crate) fn graph_layout_write(
    app: AppHandle,
    path: String,
    layout: Value,
) -> Result<(), String> {
    let safe = resolve_within(&path, &flow_read_roots(&app))?;
    let home = app_home(&app)?;
    let target = resolve_write_within(sidecar(&safe).to_string_lossy().as_ref(), &home)?;
    write_at(&target, &layout)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn round_trip_versioned_presentation_and_reject_future_versions() {
        let tmp = tempfile::tempdir().unwrap();
        let path = sidecar(&tmp.path().join("example.yaml"));
        assert!(read_at(&path).unwrap().is_none());
        let layout = json!({ "version": 1, "positions": { "id:a": { "x": -20, "y": 33 }, "__studio": { "version": 1, "groups": [] } } });
        write_at(&path, &layout).unwrap();
        assert_eq!(read_at(&path).unwrap(), Some(layout));
        assert!(write_at(&path, &json!({"version": 2, "positions": {}})).is_err());
    }
    #[cfg(unix)]
    #[test]
    fn refuses_symlinks_without_reading_or_overwriting_target() {
        let tmp = tempfile::tempdir().unwrap();
        let target = tmp.path().join("target");
        let path = tmp.path().join("flow.layout.json");
        std::fs::write(&target, "keep").unwrap();
        std::os::unix::fs::symlink(&target, &path).unwrap();
        assert!(read_at(&path).is_err());
        assert!(write_at(&path, &json!({"version":1,"positions":{}})).is_err());
        assert_eq!(std::fs::read_to_string(target).unwrap(), "keep");
    }
}
