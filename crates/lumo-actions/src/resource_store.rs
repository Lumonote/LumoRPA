//! T3 — a generic per-`(run_id, slot)` store of live resource handles, shared by
//! the stateful action families (db, http, ftp, smtp).
//!
//! Each family keeps a `static ResourceStore<H>` of its open handles, created
//! once per declared `spec.resources.<name>` and reused across every step that
//! binds to it (`Step.resource`), then reaped at run end via a
//! [`RunTeardown`](lumo_core::RunTeardown) hook. The *slot* is the bound resource
//! name, or [`DEFAULT_SLOT`] for a step that binds nothing — so unbound steps
//! keep each family's prior per-call behavior and `run_id` stays the fallback.
//!
//! The live handle lives here, never in `StepCtx` (the context only carries
//! declarations). This centralizes the nested-map bookkeeping — the error-prone
//! part — in one tested place; a family only writes its open + teardown logic.
//! (The browser action predates this and keeps an equivalent bespoke map; it can
//! adopt this later.)
//!
//! Unlike the browser (whose handle is an external Chrome *process*), these
//! handles — a sqlite `Connection`, a `reqwest::Client`, an FTP stream, an SMTP
//! transport — release their OS resource on `Drop`. So the loser of a concurrent
//! open race can usually just be dropped; [`ResourceStore::get_or_put`] keeps the
//! first winner under one lock and drops the rest, with no orphaned process. A
//! family whose loser needs a graceful *protocol* close (FTP `QUIT`, which a bare
//! socket drop skips) instead calls [`ResourceStore::get_or_put_reclaiming_loser`]
//! to get the loser handed back and shut it down itself.

use parking_lot::Mutex;
use std::collections::hash_map::Entry;
use std::collections::HashMap;
use std::sync::Arc;

/// The run's default slot — used by any step that does not bind a declared
/// resource (preserves each family's prior per-call / per-run behavior).
pub const DEFAULT_SLOT: &str = "";

/// Per-run → per-slot live-handle table: `run_id → (slot → handle)`.
type SlotTable<H> = HashMap<String, HashMap<String, Arc<H>>>;

/// A `static` store of live handles of type `H`, keyed by `(run_id, slot)`.
/// Construct in a `static` via the `const` [`ResourceStore::new`].
pub struct ResourceStore<H> {
    inner: once_cell::sync::OnceCell<Mutex<SlotTable<H>>>,
}

impl<H> ResourceStore<H> {
    pub const fn new() -> Self {
        Self {
            inner: once_cell::sync::OnceCell::new(),
        }
    }

    fn map(&self) -> &Mutex<SlotTable<H>> {
        self.inner.get_or_init(|| Mutex::new(HashMap::new()))
    }

    /// The handle open at `(run_id, slot)`, if any. The fast reuse path — callers
    /// check this before opening so an already-open resource isn't re-created.
    pub fn get(&self, run_id: &str, slot: &str) -> Option<Arc<H>> {
        self.map()
            .lock()
            .get(run_id)
            .and_then(|run| run.get(slot).cloned())
    }

    /// Store `handle` at `(run_id, slot)` iff that slot is still empty, and return
    /// the handle now in force. On a concurrent open race the first writer wins
    /// and the later `handle`(s) are dropped here (closing their socket / file /
    /// connection — no orphaned process), so the contract is "opened once,
    /// reused after" even under contention. All under one lock ⇒ no TOCTOU.
    pub fn get_or_put(&self, run_id: &str, slot: &str, handle: Arc<H>) -> Arc<H> {
        // Delegate to the loser-reclaiming primitive and let the loser (if any)
        // drop here: for handles that fully release on `Drop` (sqlite / http /
        // smtp) that bare drop IS the close. A family that needs a graceful
        // protocol close on the loser (FTP `QUIT`) calls the primitive directly.
        self.get_or_put_reclaiming_loser(run_id, slot, handle).0
    }

    /// Like [`get_or_put`](Self::get_or_put), but hands back the handle that *lost*
    /// a concurrent open race so the caller can shut it down gracefully rather than
    /// by a bare `Drop`. Returns `(in_force, loser)`: `in_force` is the handle now
    /// held for the slot (the first writer's), and `loser` is `Some(handle)` when
    /// **this** call lost — the slot was already filled, so the passed `handle` was
    /// *not* stored — or `None` when this call won and stored its handle. One lock
    /// ⇒ no TOCTOU.
    ///
    /// Exists for FTP: a dropped `AsyncFtpStream` closes its socket but skips the
    /// protocol `QUIT`, leaving the server's authenticated control connection to
    /// idle out. The reclaimed loser lets the caller `quit().await` it instead.
    pub fn get_or_put_reclaiming_loser(
        &self,
        run_id: &str,
        slot: &str,
        handle: Arc<H>,
    ) -> (Arc<H>, Option<Arc<H>>) {
        let mut g = self.map().lock();
        match g
            .entry(run_id.to_string())
            .or_default()
            .entry(slot.to_string())
        {
            // Slot already filled: keep the incumbent, hand our handle back as the
            // loser for the caller to close.
            Entry::Occupied(e) => (e.get().clone(), Some(handle)),
            // Slot empty: we win — store and return ours, no loser.
            Entry::Vacant(e) => (e.insert(handle).clone(), None),
        }
    }

    /// Remove and return the handle at `(run_id, slot)`, dropping the run bucket
    /// once its last slot is gone (keeps [`has_run`](Self::has_run) accurate).
    ///
    /// Reserved for a per-slot close (an explicit `*.close` action, or when the
    /// browser adopts this store — see the module note). The Phase 3 families only
    /// need [`take_run`](Self::take_run) at end of run, so nothing calls this yet.
    pub fn take_slot(&self, run_id: &str, slot: &str) -> Option<Arc<H>> {
        let mut g = self.map().lock();
        let removed = g.get_mut(run_id).and_then(|run| run.remove(slot));
        if g.get(run_id).is_some_and(|run| run.is_empty()) {
            g.remove(run_id);
        }
        removed
    }

    /// Remove and return every handle for `run_id` (end-of-run teardown).
    pub fn take_run(&self, run_id: &str) -> Vec<Arc<H>> {
        self.map()
            .lock()
            .remove(run_id)
            .map(|run| run.into_values().collect())
            .unwrap_or_default()
    }

    /// Whether any handle is open for `run_id` (any slot). For tests.
    pub fn has_run(&self, run_id: &str) -> bool {
        self.map()
            .lock()
            .get(run_id)
            .is_some_and(|run| !run.is_empty())
    }
}

impl<H> Default for ResourceStore<H> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn get_put_take_slot_isolates_slots_and_drops_empty_bucket() {
        let store: ResourceStore<i32> = ResourceStore::new();
        assert_eq!(store.get("run", "a"), None);
        // First put at a slot wins and is returned.
        assert_eq!(*store.get_or_put("run", "a", Arc::new(1)), 1);
        assert_eq!(*store.get_or_put("run", "b", Arc::new(2)), 2);
        assert_eq!(store.get("run", "a").as_deref(), Some(&1));

        // Removing one slot leaves the sibling and keeps the run alive.
        assert_eq!(store.take_slot("run", "a").as_deref(), Some(&1));
        assert!(store.has_run("run"));
        assert_eq!(store.get("run", "a"), None);
        // Removing the last slot drops the whole run bucket.
        assert_eq!(store.take_slot("run", "b").as_deref(), Some(&2));
        assert!(!store.has_run("run"));
        // Idempotent: removing absent run/slot is a clean None.
        assert_eq!(store.take_slot("run", "gone"), None);
    }

    #[test]
    fn get_or_put_keeps_first_and_drops_later_losers() {
        // The race contract: first writer wins; later handles are dropped (their
        // resource closes), and every caller observes the SAME winner.
        static DROPS: AtomicUsize = AtomicUsize::new(0);
        struct Counted(#[allow(dead_code)] u32);
        impl Drop for Counted {
            fn drop(&mut self) {
                DROPS.fetch_add(1, Ordering::SeqCst);
            }
        }
        let store: ResourceStore<Counted> = ResourceStore::new();
        let winner = store.get_or_put("run", "x", Arc::new(Counted(1)));
        // A second open of the same slot: loser is dropped here, winner returned.
        let again = store.get_or_put("run", "x", Arc::new(Counted(2)));
        assert!(Arc::ptr_eq(&winner, &again), "both callers see the same handle");
        assert_eq!(
            DROPS.load(Ordering::SeqCst),
            1,
            "the loser (Counted(2)) must be dropped immediately"
        );
        // Only the winner remains; teardown reaps it.
        assert_eq!(store.take_run("run").len(), 1);
    }

    #[test]
    fn take_run_drains_all_slots_and_leaves_other_runs() {
        let store: ResourceStore<i32> = ResourceStore::new();
        store.get_or_put("run", "a", Arc::new(1));
        store.get_or_put("run", "b", Arc::new(2));
        store.get_or_put("other", "a", Arc::new(9));
        let mut drained: Vec<i32> = store.take_run("run").iter().map(|h| **h).collect();
        drained.sort();
        assert_eq!(drained, vec![1, 2], "end-of-run teardown reaps every slot");
        assert!(!store.has_run("run"));
        assert!(store.has_run("other"), "a different run is untouched");
        assert!(store.take_run("run").is_empty(), "second drain is empty");
    }

    #[test]
    fn get_or_put_reclaiming_loser_hands_the_loser_back_keeping_the_first_winner() {
        let store: ResourceStore<i32> = ResourceStore::new();
        // First writer wins: stored and returned, with no loser to reclaim.
        let (w1, loser1) = store.get_or_put_reclaiming_loser("run", "x", Arc::new(1));
        assert_eq!(*w1, 1);
        assert!(loser1.is_none(), "the first writer keeps no loser");
        // Second writer loses: the slot still holds the first winner, and OUR handle
        // is handed back (not silently dropped) so we can close it gracefully — this
        // is what FTP uses to `QUIT` the redundant session.
        let (w2, loser2) = store.get_or_put_reclaiming_loser("run", "x", Arc::new(2));
        assert!(Arc::ptr_eq(&w1, &w2), "the slot still holds the first winner");
        assert_eq!(
            loser2.as_deref(),
            Some(&2),
            "the losing handle is returned for graceful teardown"
        );
        // The plain wrapper keeps its old semantics (loser dropped, winner kept).
        let w3 = store.get_or_put("run", "x", Arc::new(3));
        assert!(Arc::ptr_eq(&w1, &w3));
        assert_eq!(store.take_run("run").len(), 1, "only ever one handle in the slot");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn concurrent_opens_of_one_slot_converge_to_a_single_shared_handle() {
        // The contract parallel branches (`control.parallel`) rely on: many tasks
        // may race to open the SAME resource slot, but all must converge to ONE
        // handle (which they then serialize use of via its own Mutex). Hammer one
        // slot from many tasks across real threads and assert a single winner.
        let store = Arc::new(ResourceStore::<u64>::new());
        let tasks: Vec<_> = (0..64u64)
            .map(|i| {
                let s = store.clone();
                tokio::spawn(async move { *s.get_or_put("run", "slot", Arc::new(i)) })
            })
            .collect();
        let mut winners = std::collections::HashSet::new();
        for t in tasks {
            winners.insert(t.await.unwrap());
        }
        assert_eq!(
            winners.len(),
            1,
            "every racer must observe the same single winner, got {winners:?}"
        );
        assert_eq!(store.take_run("run").len(), 1, "exactly one handle is stored");
    }
}
