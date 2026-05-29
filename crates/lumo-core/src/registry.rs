use crate::action::ActionRef;
use async_trait::async_trait;
use dashmap::DashMap;
use parking_lot::Mutex;
use std::sync::Arc;

/// Hook invoked once per run, after the flow finishes, regardless of whether it
/// succeeded or failed. Lets an action crate reclaim resources it keyed by
/// `run_id` (e.g. a launched browser process) so a failing flow can't leak them
/// (P1-2). Registered via [`ActionRegistry::register_teardown`].
#[async_trait]
pub trait RunTeardown: Send + Sync + 'static {
    /// Reclaim any resources associated with `run_id`. Must be idempotent and
    /// must not panic when there is nothing to clean up.
    async fn teardown(&self, run_id: &str);
}

#[derive(Default, Clone)]
pub struct ActionRegistry {
    inner: Arc<DashMap<String, ActionRef>>,
    teardowns: Arc<Mutex<Vec<Arc<dyn RunTeardown>>>>,
}

impl ActionRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register<A: crate::action::Action>(&mut self, action: A) {
        let id = action.id().to_string();
        self.inner.insert(id, Arc::new(action));
    }

    /// Register an end-of-run teardown hook (see [`RunTeardown`]). The VM calls
    /// every registered hook with the run id once the flow finishes.
    pub fn register_teardown(&mut self, teardown: Arc<dyn RunTeardown>) {
        self.teardowns.lock().push(teardown);
    }

    pub fn get(&self, id: &str) -> Option<ActionRef> {
        self.inner.get(id).map(|r| r.value().clone())
    }

    /// Snapshot of the registered teardown hooks. Used by the VM at end-of-run.
    pub(crate) fn teardowns(&self) -> Vec<Arc<dyn RunTeardown>> {
        self.teardowns.lock().clone()
    }

    pub fn iter_ids(&self) -> impl Iterator<Item = String> {
        self.inner
            .iter()
            .map(|kv| kv.key().clone())
            .collect::<Vec<_>>()
            .into_iter()
    }
}
