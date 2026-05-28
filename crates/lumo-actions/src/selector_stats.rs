//! Persistent success/failure stats for the Self-Healing Router (A-14).
//!
//! Every time `resolve_element` tries a selector, we record per-strategy
//! outcomes against the canonical fingerprint hash of the spec. The next
//! resolve pulls the rate back through `MultiSelector::ordered_for_runtime`
//! and demotes strategies that have failed repeatedly for that exact spec.
//!
//! Storage is a JSON file under `$LUMO_HOME/selector-stats.json` (default
//! `~/.lumorpa/selector-stats.json`). Lightweight, no schema migration; we
//! can promote to SQLite once we wire `lumo-storage` in for shared state.
//! Writes are best-effort — if the file is unwritable we keep the in-memory
//! cache and move on.

use once_cell::sync::Lazy;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Per-strategy outcome counter at a single fingerprint hash.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct StrategyRecord {
    #[serde(default)]
    pub success: u32,
    #[serde(default)]
    pub fail: u32,
    #[serde(default)]
    pub last_ms: i64,
}

impl StrategyRecord {
    pub fn total(&self) -> u32 {
        self.success + self.fail
    }
    pub fn success_rate(&self) -> Option<f32> {
        let total = self.total();
        if total == 0 {
            None
        } else {
            Some(self.success as f32 / total as f32)
        }
    }
    pub fn fail_rate(&self) -> Option<f32> {
        self.success_rate().map(|sr| 1.0 - sr)
    }
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct StatsData {
    /// `fingerprint_hash` → `strategy_name` → record.
    #[serde(default)]
    pub by_hash: HashMap<String, HashMap<String, StrategyRecord>>,
    /// Recovery transitions: when `from_strategy` failed and `to_strategy`
    /// succeeded for the same fingerprint, increment `[hash][from][to]`. The
    /// Self-Healing Router (A-14) uses these counts to bias `ordered_for_runtime`
    /// toward strategies with proven recovery history.
    #[serde(default)]
    pub transitions: HashMap<String, HashMap<String, HashMap<String, u32>>>,
}

pub struct SelectorStats {
    path: PathBuf,
    inner: Mutex<StatsData>,
}

impl SelectorStats {
    /// Lazily-initialized process-wide store. Reads the on-disk JSON on first
    /// access and caches the deserialized form; subsequent records write back
    /// to disk on every update.
    pub fn global() -> &'static SelectorStats {
        static G: Lazy<SelectorStats> = Lazy::new(|| {
            let path = default_path();
            let data = load_from(&path).unwrap_or_default();
            SelectorStats {
                path,
                inner: Mutex::new(data),
            }
        });
        &G
    }

    /// Build a stats store backed by a specific file. Mostly used by tests so
    /// they don't smash the user's real stats; production code uses `global()`.
    pub fn at_path(path: PathBuf) -> Self {
        let data = load_from(&path).unwrap_or_default();
        Self {
            path,
            inner: Mutex::new(data),
        }
    }

    /// Record that `strategy` was tried at `hash` with outcome `ok`. Persists
    /// to disk best-effort; if the write fails we still update the cache so
    /// in-process ordering decisions stay current.
    pub fn record(&self, hash: &str, strategy: &str, ok: bool) {
        let now = chrono::Utc::now().timestamp_millis();
        let mut g = self.inner.lock();
        let rec = g
            .by_hash
            .entry(hash.to_string())
            .or_default()
            .entry(strategy.to_string())
            .or_default();
        if ok {
            rec.success += 1;
        } else {
            rec.fail += 1;
        }
        rec.last_ms = now;
        let snapshot = g.clone();
        drop(g);
        let _ = save_to(&self.path, &snapshot);
    }

    /// Look up the current record for `(hash, strategy)`.
    pub fn record_for(&self, hash: &str, strategy: &str) -> Option<StrategyRecord> {
        let g = self.inner.lock();
        g.by_hash.get(hash).and_then(|m| m.get(strategy)).cloned()
    }

    /// Adjusted cost helper: returns a multiplier ≥ 1.0 that penalizes
    /// strategies with observed failures. Unseen `(hash, strategy)` pairs
    /// return 1.0 so they stay at their base cost.
    pub fn history_penalty(&self, hash: &str, strategy: &str) -> f32 {
        match self.record_for(hash, strategy) {
            None => 1.0,
            Some(rec) => match rec.fail_rate() {
                None => 1.0,
                // 1× when zero failures, up to 3× when every attempt failed.
                Some(fr) => 1.0 + 2.0 * fr,
            },
        }
    }

    /// Record that `to` succeeded for `hash` after `from` failed. Used by the
    /// router to weight transitions in greedy fall-back ordering (A-14).
    pub fn record_transition(&self, hash: &str, from: &str, to: &str) {
        let mut g = self.inner.lock();
        let counter = g
            .transitions
            .entry(hash.to_string())
            .or_default()
            .entry(from.to_string())
            .or_default()
            .entry(to.to_string())
            .or_default();
        *counter += 1;
        let snapshot = g.clone();
        drop(g);
        let _ = save_to(&self.path, &snapshot);
    }

    /// Observed count of recoveries `from → to` at this fingerprint. Zero
    /// when the transition has never been seen.
    pub fn transition_count(&self, hash: &str, from: &str, to: &str) -> u32 {
        let g = self.inner.lock();
        g.transitions
            .get(hash)
            .and_then(|m| m.get(from))
            .and_then(|m| m.get(to))
            .copied()
            .unwrap_or(0)
    }

    /// Probability `to` is the right recovery for `from` at this fingerprint,
    /// computed as `count(from→to) / Σ count(from→*)`. Returns 0.0 when there
    /// is no recovery history for `from`.
    pub fn transition_score(&self, hash: &str, from: &str, to: &str) -> f32 {
        let g = self.inner.lock();
        let Some(per_from) = g.transitions.get(hash).and_then(|m| m.get(from)) else {
            return 0.0;
        };
        let total: u32 = per_from.values().sum();
        if total == 0 {
            return 0.0;
        }
        let hit = per_from.get(to).copied().unwrap_or(0);
        hit as f32 / total as f32
    }
}

fn default_path() -> PathBuf {
    if let Some(home) = std::env::var_os("LUMO_HOME") {
        return PathBuf::from(home).join("selector-stats.json");
    }
    let base = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(|h| PathBuf::from(h).join(".lumorpa"))
        .unwrap_or_else(|| PathBuf::from(".lumorpa"));
    base.join("selector-stats.json")
}

fn load_from(path: &Path) -> Option<StatsData> {
    let raw = std::fs::read(path).ok()?;
    serde_json::from_slice(&raw).ok()
}

fn save_to(path: &Path, data: &StatsData) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let body = serde_json::to_vec_pretty(data).unwrap_or_else(|_| b"{}".to_vec());
    std::fs::write(path, body)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn temp_stats() -> (TempDir, SelectorStats) {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("stats.json");
        (dir, SelectorStats::at_path(path))
    }

    #[test]
    fn record_persists_and_reloads() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("stats.json");
        {
            let s = SelectorStats::at_path(path.clone());
            s.record("abc", "css", true);
            s.record("abc", "css", true);
            s.record("abc", "xpath", false);
        }
        let s2 = SelectorStats::at_path(path);
        let css = s2.record_for("abc", "css").unwrap();
        let xp = s2.record_for("abc", "xpath").unwrap();
        assert_eq!(css.success, 2);
        assert_eq!(css.fail, 0);
        assert_eq!(xp.success, 0);
        assert_eq!(xp.fail, 1);
    }

    #[test]
    fn history_penalty_unseen_is_one() {
        let (_dir, s) = temp_stats();
        assert!((s.history_penalty("ghost", "css") - 1.0).abs() < 1e-6);
    }

    #[test]
    fn history_penalty_grows_with_fail_rate() {
        let (_dir, s) = temp_stats();
        s.record("h", "css", true);
        let p_low = s.history_penalty("h", "css");
        for _ in 0..10 {
            s.record("h", "xpath", false);
        }
        let p_high = s.history_penalty("h", "xpath");
        assert!(p_low < 1.5);
        assert!(p_high > 2.5);
    }

    #[test]
    fn success_rate_after_mixed_outcomes() {
        let (_dir, s) = temp_stats();
        s.record("h", "css", true);
        s.record("h", "css", true);
        s.record("h", "css", false);
        let rec = s.record_for("h", "css").unwrap();
        assert_eq!(rec.total(), 3);
        let sr = rec.success_rate().unwrap();
        assert!((sr - (2.0 / 3.0)).abs() < 1e-6);
    }

    #[test]
    fn transition_count_starts_at_zero() {
        let (_dir, s) = temp_stats();
        assert_eq!(s.transition_count("h", "id", "css"), 0);
        assert!((s.transition_score("h", "id", "css") - 0.0).abs() < 1e-6);
    }

    #[test]
    fn record_transition_increments_and_normalizes() {
        let (_dir, s) = temp_stats();
        s.record_transition("h", "id", "css");
        s.record_transition("h", "id", "css");
        s.record_transition("h", "id", "xpath");
        assert_eq!(s.transition_count("h", "id", "css"), 2);
        assert_eq!(s.transition_count("h", "id", "xpath"), 1);
        // 2 of 3 from→* are css → 0.666…
        assert!((s.transition_score("h", "id", "css") - 2.0 / 3.0).abs() < 1e-6);
        assert!((s.transition_score("h", "id", "xpath") - 1.0 / 3.0).abs() < 1e-6);
    }

    #[test]
    fn transition_persists_across_reload() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("stats.json");
        {
            let s = SelectorStats::at_path(path.clone());
            s.record_transition("hash", "id", "data_testid");
        }
        let s2 = SelectorStats::at_path(path);
        assert_eq!(s2.transition_count("hash", "id", "data_testid"), 1);
    }
}
