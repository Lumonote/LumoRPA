use crate::action::ActionRef;
use crate::resource::ResourceFactory;
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
    /// T3: resource factories keyed by [`ResourceFactory::kind`]. The VM/actions
    /// look one up to lazily open a declared `spec.resources` entry of that kind
    /// on first reference (see [`Self::register_resource_factory`]).
    factories: Arc<DashMap<String, Arc<dyn ResourceFactory>>>,
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

    /// T3: register a [`ResourceFactory`] under its [`kind`](ResourceFactory::kind).
    /// The VM/actions look it up to lazily open a declared `spec.resources` entry
    /// of that kind on first reference. A later registration for the same kind
    /// replaces the earlier one (mirrors [`register`](Self::register) for actions).
    pub fn register_resource_factory(&mut self, factory: Arc<dyn ResourceFactory>) {
        let kind = factory.kind().to_string();
        self.factories.insert(kind, factory);
    }

    /// T3: the resource factory registered for `kind`, if any.
    pub fn resource_factory(&self, kind: &str) -> Option<Arc<dyn ResourceFactory>> {
        self.factories.get(kind).map(|r| r.value().clone())
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
