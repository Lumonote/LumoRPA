//! 流程文件保存历史（批次 C）：每次覆盖保存前，把磁盘上的旧内容滚动快照到
//! `$LUMO_HOME/flow-history/<key>/<millis>.yaml`，只保留最近 [`FLOW_HISTORY_KEEP`]
//! 版。key = 文件名 stem（人眼可识别）+ 绝对路径 FNV 哈希（防同名冲突）。

use std::io;
use std::path::{Path, PathBuf};

pub(crate) const FLOW_HISTORY_KEEP: usize = 20;

fn fnv1a(bytes: &[u8]) -> u32 {
    let mut hash: u32 = 0x811c_9dc5;
    for byte in bytes {
        hash ^= u32::from(*byte);
        hash = hash.wrapping_mul(0x0100_0193);
    }
    hash
}

pub(crate) fn history_key(flow_path: &Path) -> String {
    let stem = flow_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("flow");
    let sanitized: String = stem
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    format!(
        "{sanitized}-{:08x}",
        fnv1a(flow_path.to_string_lossy().as_bytes())
    )
}

pub(crate) fn history_dir(home: &Path, flow_path: &Path) -> PathBuf {
    home.join("flow-history").join(history_key(flow_path))
}

/// 按时间戳降序列出 `(millis, path)` 快照；目录不存在视为空。
pub(crate) fn list_entries(dir: &Path) -> io::Result<Vec<(i64, PathBuf)>> {
    let mut entries = Vec::new();
    let read_dir = match std::fs::read_dir(dir) {
        Ok(read_dir) => read_dir,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(entries),
        Err(error) => return Err(error),
    };
    for entry in read_dir {
        let path = entry?.path();
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        let Some(millis) = name
            .strip_suffix(".yaml")
            .and_then(|stem| stem.parse::<i64>().ok())
        else {
            continue;
        };
        entries.push((millis, path));
    }
    entries.sort_by_key(|(millis, _)| std::cmp::Reverse(*millis));
    Ok(entries)
}

/// 只保留最新 `keep` 个快照。
pub(crate) fn rotate(dir: &Path, keep: usize) -> io::Result<()> {
    for (_, path) in list_entries(dir)?.into_iter().skip(keep) {
        std::fs::remove_file(path)?;
    }
    Ok(())
}

/// 保存前快照：flow 文件已存在且内容与将写入的不同时，归档旧内容并轮转。
pub(crate) fn snapshot_before_save(
    home: &Path,
    flow_path: &Path,
    next_source: &str,
    now_ms: i64,
) -> io::Result<()> {
    let previous = match std::fs::read_to_string(flow_path) {
        Ok(previous) => previous,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    if previous == next_source {
        return Ok(());
    }
    let dir = history_dir(home, flow_path);
    std::fs::create_dir_all(&dir)?;
    std::fs::write(dir.join(format!("{now_ms}.yaml")), previous)?;
    rotate(&dir, FLOW_HISTORY_KEEP)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root(tag: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("lumo-flow-history-{tag}-{}", ulid::Ulid::new()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn history_key_is_stable_and_sanitized() {
        let a = history_key(Path::new("/home/u/flows/日报 flow.lumoflow.yaml"));
        let b = history_key(Path::new("/home/u/flows/日报 flow.lumoflow.yaml"));
        let c = history_key(Path::new("/elsewhere/日报 flow.lumoflow.yaml"));
        assert_eq!(a, b);
        assert_ne!(a, c, "same stem in another dir must get a distinct key");
        assert!(!a.contains(' '), "spaces are sanitized: {a}");
    }

    #[test]
    fn snapshot_only_when_disk_content_differs() {
        let root = temp_root("diff");
        let flow = root.join("f.lumoflow.yaml");
        std::fs::write(&flow, "v1").unwrap();
        // identical content → no snapshot
        snapshot_before_save(&root, &flow, "v1", 1_000).unwrap();
        assert!(list_entries(&history_dir(&root, &flow)).unwrap().is_empty());
        // changed content → snapshot of the OLD version
        snapshot_before_save(&root, &flow, "v2", 2_000).unwrap();
        let entries = list_entries(&history_dir(&root, &flow)).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].0, 2_000);
        assert_eq!(std::fs::read_to_string(&entries[0].1).unwrap(), "v1");
        // missing flow file (first save) → no snapshot, no error
        let fresh = root.join("new.lumoflow.yaml");
        snapshot_before_save(&root, &fresh, "v1", 3_000).unwrap();
        assert!(list_entries(&history_dir(&root, &fresh))
            .unwrap()
            .is_empty());
    }

    #[test]
    fn rotate_keeps_only_newest_snapshots() {
        let root = temp_root("rotate");
        let dir = root.join("hist");
        std::fs::create_dir_all(&dir).unwrap();
        for ms in [1_000i64, 2_000, 3_000, 4_000, 5_000] {
            std::fs::write(dir.join(format!("{ms}.yaml")), ms.to_string()).unwrap();
        }
        rotate(&dir, 3).unwrap();
        let entries = list_entries(&dir).unwrap();
        assert_eq!(
            entries.iter().map(|(ms, _)| *ms).collect::<Vec<_>>(),
            vec![5_000, 4_000, 3_000]
        );
    }

    #[test]
    fn list_entries_ignores_foreign_files_and_missing_dir() {
        let root = temp_root("list");
        let dir = root.join("hist");
        assert!(list_entries(&dir).unwrap().is_empty());
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("2000.yaml"), "b").unwrap();
        std::fs::write(dir.join("1000.yaml"), "a").unwrap();
        std::fs::write(dir.join("notes.txt"), "x").unwrap();
        let entries = list_entries(&dir).unwrap();
        assert_eq!(
            entries.iter().map(|(ms, _)| *ms).collect::<Vec<_>>(),
            vec![2_000, 1_000]
        );
    }
}
