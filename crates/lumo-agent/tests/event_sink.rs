use std::sync::{Arc, Mutex};

use lumo_agent::{
    AgentEvent, AgentEventDraft, AgentEventKind, AgentEventRepository, EventSink, EventSinkError,
};

struct Repository {
    fail: bool,
    events: Mutex<Vec<AgentEvent>>,
}

impl AgentEventRepository for Repository {
    fn last_sequence(&self, run_id: &str) -> Result<i64, String> {
        Ok(self
            .events
            .lock()
            .unwrap()
            .iter()
            .filter(|event| event.run_id == run_id)
            .map(|event| event.seq)
            .max()
            .unwrap_or(0))
    }

    fn append_agent_event(&self, event: &AgentEvent) -> Result<(), String> {
        if self.fail {
            return Err("disk unavailable".into());
        }
        self.events.lock().unwrap().push(event.clone());
        Ok(())
    }
}

#[tokio::test]
async fn persistence_failure_never_broadcasts() {
    let repository = Arc::new(Repository {
        fail: true,
        events: Mutex::new(vec![]),
    });
    let sink = EventSink::new(repository, 16);
    let mut receiver = sink.subscribe();

    assert!(matches!(
        sink.publish(AgentEventDraft::new("run-1", AgentEventKind::RunStarted))
            .await,
        Err(EventSinkError::Persistence(_))
    ));
    assert!(matches!(
        receiver.try_recv(),
        Err(tokio::sync::broadcast::error::TryRecvError::Empty)
    ));
}

#[tokio::test]
async fn concurrent_publishers_get_unique_monotonic_sequences() {
    let repository = Arc::new(Repository {
        fail: false,
        events: Mutex::new(vec![]),
    });
    let sink = Arc::new(EventSink::new(repository.clone(), 16));
    let mut tasks = Vec::new();
    for _ in 0..8 {
        let sink = sink.clone();
        tasks.push(tokio::spawn(async move {
            sink.publish(AgentEventDraft::new("run-1", AgentEventKind::NodeQueued))
                .await
                .unwrap()
        }));
    }
    let mut sequences = Vec::new();
    for task in tasks {
        sequences.push(task.await.unwrap().seq);
    }
    sequences.sort_unstable();
    assert_eq!(sequences, (1..=8).collect::<Vec<_>>());
}
