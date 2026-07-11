use std::{collections::HashMap, sync::Arc};

use thiserror::Error;
use tokio::sync::{broadcast, Mutex};

use crate::{AgentEvent, AgentEventDraft};

pub trait AgentEventRepository: Send + Sync {
    fn last_sequence(&self, run_id: &str) -> Result<i64, String>;
    fn append_agent_event(&self, event: &AgentEvent) -> Result<(), String>;
}

impl AgentEventRepository for lumo_storage::Repo {
    fn last_sequence(&self, run_id: &str) -> Result<i64, String> {
        self.list_agent_events(run_id, -1)
            .map_err(|error| error.to_string())
            .map(|events| events.last().map(|event| event.seq).unwrap_or(0))
    }

    fn append_agent_event(&self, event: &AgentEvent) -> Result<(), String> {
        lumo_storage::Repo::append_agent_event(
            self,
            lumo_storage::AgentEventInsert {
                run_id: &event.run_id,
                seq: event.seq,
                kind: event.kind.as_str(),
                node_id: event.node_id.as_deref(),
                parent_node_id: event.parent_node_id.as_deref(),
                payload: &event.payload,
            },
        )
        .map_err(|error| error.to_string())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum EventSinkError {
    #[error("agent event persistence failed: {0}")]
    Persistence(String),
}

#[derive(Clone)]
pub struct EventSink {
    repository: Arc<dyn AgentEventRepository>,
    run_locks: Arc<std::sync::Mutex<HashMap<String, Arc<Mutex<()>>>>>,
    sender: broadcast::Sender<AgentEvent>,
}

impl EventSink {
    pub fn new(repository: Arc<dyn AgentEventRepository>, capacity: usize) -> Self {
        let (sender, _) = broadcast::channel(capacity.max(1));
        Self {
            repository,
            run_locks: Arc::new(std::sync::Mutex::new(HashMap::new())),
            sender,
        }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<AgentEvent> {
        self.sender.subscribe()
    }

    pub async fn publish(&self, draft: AgentEventDraft) -> Result<AgentEvent, EventSinkError> {
        let run_lock = {
            let mut locks = self.run_locks.lock().expect("event sink lock poisoned");
            locks
                .entry(draft.run_id.clone())
                .or_insert_with(|| Arc::new(Mutex::new(())))
                .clone()
        };
        let _guard = run_lock.lock().await;
        let seq = self
            .repository
            .last_sequence(&draft.run_id)
            .map_err(EventSinkError::Persistence)?
            .checked_add(1)
            .ok_or_else(|| EventSinkError::Persistence("event sequence overflow".into()))?;
        let event = draft.stamp(seq);
        self.repository
            .append_agent_event(&event)
            .map_err(EventSinkError::Persistence)?;

        // A run may legitimately have no live subscribers. Persistence is the
        // source of truth, so a closed broadcast channel is not an error.
        let _ = self.sender.send(event.clone());
        Ok(event)
    }
}
