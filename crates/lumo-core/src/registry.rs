use crate::action::ActionRef;
use dashmap::DashMap;
use std::sync::Arc;

#[derive(Default, Clone)]
pub struct ActionRegistry {
    inner: Arc<DashMap<String, ActionRef>>,
}

impl ActionRegistry {
    pub fn new() -> Self { Self::default() }

    pub fn register<A: crate::action::Action>(&mut self, action: A) {
        let id = action.id().to_string();
        self.inner.insert(id, Arc::new(action));
    }

    pub fn get(&self, id: &str) -> Option<ActionRef> {
        self.inner.get(id).map(|r| r.value().clone())
    }

    pub fn iter_ids(&self) -> impl Iterator<Item = String> {
        self.inner
            .iter()
            .map(|kv| kv.key().clone())
            .collect::<Vec<_>>()
            .into_iter()
    }
}
