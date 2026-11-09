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

#[tokio::test]
async fn durable_sequence_lookup_does_not_decode_history_or_mix_runs() {
    let repo = lumo_storage::Repo::open_in_memory().unwrap();
    assert_eq!(repo.last_sequence("empty").unwrap(), 0);
    for id in ["run-1", "other-run"] {
        repo.create_agent_run(&lumo_storage::AgentRunRow {
            id: id.into(),
            profile_id: None,
            utterance: None,
            plan_json: None,
            approval_json: None,
            state: "running".into(),
            started_at: chrono::Utc::now(),
            finished_at: None,
            error: None,
        })
        .unwrap();
    }
    for (id, seq) in [("run-1", 3), ("run-1", 1), ("other-run", 99)] {
        repo.append_agent_event(lumo_storage::AgentEventInsert {
            run_id: id,
            seq,
            kind: "run.started",
            node_id: None,
            parent_node_id: None,
            payload: &serde_json::json!({}),
        })
        .unwrap();
    }
    // A damaged historical payload should remain visible to a reader, while
    // sequence allocation only needs the indexed sequence column.
    repo.with_raw(|conn| {
        conn.execute(
            "UPDATE agent_events SET payload='{' WHERE run_id='run-1' AND seq=1",
            [],
        )
    })
    .unwrap();
    assert!(repo.list_agent_events("run-1", 0).is_err());
    assert_eq!(repo.last_sequence("run-1").unwrap(), 3);
    let sink = EventSink::new(Arc::new(repo.clone()), 8);
    let event = sink
        .publish(AgentEventDraft::new("run-1", AgentEventKind::RunCompleted))
        .await
        .unwrap();
    assert_eq!(event.seq, 4);
    assert_eq!(repo.list_agent_events("run-1", 3).unwrap()[0].seq, 4);
}
