use chrono::{TimeZone, Utc};
use lumo_storage::{AgentEventInsert, AgentRunRow, McpServerRow, McpToolRow, Repo, StorageError};
use serde_json::json;

fn run(id: &str) -> AgentRunRow {
    AgentRunRow {
        id: id.into(),
        profile_id: Some("profile-1".into()),
        utterance: Some("open the dashboard".into()),
        plan_json: Some(json!({"nodes": ["open", "inspect"]})),
        approval_json: Some(json!({"decision": "approved"})),
        state: "running".into(),
        started_at: Utc.timestamp_millis_opt(1_720_000_000_123).unwrap(),
        finished_at: None,
        error: None,
    }
}

fn server(name: &str, health: &str) -> McpServerRow {
    McpServerRow {
        id: "server-1".into(),
        name: name.into(),
        transport: "stdio".into(),
        config: json!({"command": "demo-mcp", "args": ["--stdio"]}),
        enabled: true,
        health: health.into(),
        created_at: Utc.timestamp_millis_opt(1_720_000_000_000).unwrap(),
        updated_at: Utc.timestamp_millis_opt(1_720_000_001_000).unwrap(),
    }
}

fn tool(name: &str, version_hash: &str) -> McpToolRow {
    McpToolRow {
        server_id: "server-1".into(),
        name: name.into(),
        description: format!("{name} description"),
        input_schema: json!({"type": "object", "required": ["query"]}),
        output_schema: Some(json!({"type": "object"})),
        risk: "L1".into(),
        enabled: true,
        version_hash: version_hash.into(),
        discovered_at: Utc.timestamp_millis_opt(1_720_000_002_000).unwrap(),
    }
}

#[test]
fn agent_events_round_trip_json_nodes_timestamps_and_sequence_order() {
    let repo = Repo::open_in_memory().unwrap();
    repo.create_agent_run(&run("run-1")).unwrap();

    let second_payload = json!({"result": [1, 2, 3]});
    repo.append_agent_event(AgentEventInsert {
        run_id: "run-1",
        seq: 2,
        kind: "node_finished",
        node_id: None,
        parent_node_id: Some("root"),
        payload: &second_payload,
    })
    .unwrap();
    let first_payload = json!({"input": {"url": "https://example.test"}});
    repo.append_agent_event(AgentEventInsert {
        run_id: "run-1",
        seq: 1,
        kind: "node_started",
        node_id: Some("open"),
        parent_node_id: None,
        payload: &first_payload,
    })
    .unwrap();

    let events = repo.list_agent_events("run-1", 0).unwrap();
    assert_eq!(events.len(), 2);
    assert_eq!(events[0].seq, 1);
    assert_eq!(events[0].kind, "node_started");
    assert_eq!(events[0].node_id.as_deref(), Some("open"));
    assert_eq!(events[0].parent_node_id, None);
    assert_eq!(events[0].payload, first_payload);
    assert!(events[0].created_at.timestamp_millis() > 0);
    assert_eq!(events[1].seq, 2);
    assert_eq!(events[1].node_id, None);
    assert_eq!(events[1].parent_node_id.as_deref(), Some("root"));
    assert_eq!(events[1].payload, second_payload);

    let after_first = repo.list_agent_events("run-1", 1).unwrap();
    assert_eq!(after_first.len(), 1);
    assert_eq!(after_first[0].seq, 2);
}

#[test]
fn duplicate_agent_event_sequence_is_rejected() {
    let repo = Repo::open_in_memory().unwrap();
    repo.create_agent_run(&run("run-1")).unwrap();
    let payload = json!({});
    let insert = || AgentEventInsert {
        run_id: "run-1",
        seq: 1,
        kind: "started",
        node_id: None,
        parent_node_id: None,
        payload: &payload,
    };

    repo.append_agent_event(insert()).unwrap();
    assert!(matches!(
        repo.append_agent_event(insert()),
        Err(StorageError::Sqlite(_))
    ));
}

#[test]
fn upsert_mcp_server_inserts_then_updates_same_id() {
    let repo = Repo::open_in_memory().unwrap();
    repo.upsert_mcp_server(&server("Original", "unknown"))
        .unwrap();
    repo.upsert_mcp_server(&server("Updated", "healthy"))
        .unwrap();

    let stored = repo.get_mcp_server("server-1").unwrap().unwrap();
    assert_eq!(stored.id, "server-1");
    assert_eq!(stored.name, "Updated");
    assert_eq!(stored.health, "healthy");
    assert_eq!(
        stored.config,
        json!({"command": "demo-mcp", "args": ["--stdio"]})
    );
}

#[test]
fn list_mcp_servers_returns_all_servers_by_name_then_id() {
    let repo = Repo::open_in_memory().unwrap();
    let mut zeta = server("Zeta", "unknown");
    zeta.id = "server-z".into();
    let mut alpha_two = server("Alpha", "healthy");
    alpha_two.id = "server-b".into();
    let mut alpha_one = server("Alpha", "unhealthy");
    alpha_one.id = "server-a".into();

    repo.upsert_mcp_server(&zeta).unwrap();
    repo.upsert_mcp_server(&alpha_two).unwrap();
    repo.upsert_mcp_server(&alpha_one).unwrap();

    let listed = repo.list_mcp_servers().unwrap();
    assert_eq!(
        listed
            .iter()
            .map(|row| (row.name.as_str(), row.id.as_str()))
            .collect::<Vec<_>>(),
        [
            ("Alpha", "server-a"),
            ("Alpha", "server-b"),
            ("Zeta", "server-z")
        ]
    );
    assert_eq!(listed[0].health, "unhealthy");
}

#[test]
fn replace_mcp_tools_replaces_atomically_and_lists_by_name() {
    let repo = Repo::open_in_memory().unwrap();
    repo.upsert_mcp_server(&server("Server", "healthy"))
        .unwrap();
    repo.replace_mcp_tools("server-1", &[tool("zeta", "v1"), tool("alpha", "v1")])
        .unwrap();
    repo.replace_mcp_tools("server-1", &[tool("middle", "v2"), tool("alpha", "v2")])
        .unwrap();

    let tools = repo.list_mcp_tools("server-1").unwrap();
    assert_eq!(
        tools
            .iter()
            .map(|tool| tool.name.as_str())
            .collect::<Vec<_>>(),
        ["alpha", "middle"]
    );
    assert_eq!(tools[0].version_hash, "v2");
    assert_eq!(
        tools[0].input_schema,
        json!({"type": "object", "required": ["query"]})
    );
    assert_eq!(tools[0].output_schema, Some(json!({"type": "object"})));

    let duplicate_names = [tool("duplicate", "v3"), tool("duplicate", "v4")];
    assert!(repo
        .replace_mcp_tools("server-1", &duplicate_names)
        .is_err());
    let preserved = repo.list_mcp_tools("server-1").unwrap();
    assert_eq!(
        preserved
            .iter()
            .map(|tool| tool.name.as_str())
            .collect::<Vec<_>>(),
        ["alpha", "middle"],
        "a failed replacement must roll back the initial delete"
    );
}

#[test]
fn replace_mcp_tools_rejects_missing_server_without_rows() {
    let repo = Repo::open_in_memory().unwrap();
    assert!(repo
        .replace_mcp_tools("missing", &[tool("alpha", "v1")])
        .is_err());
    assert!(repo.list_mcp_tools("missing").unwrap().is_empty());
}

#[test]
fn replace_mcp_tools_rejects_mismatched_server_id_and_preserves_existing_tools() {
    let repo = Repo::open_in_memory().unwrap();
    repo.upsert_mcp_server(&server("Server", "healthy"))
        .unwrap();
    repo.replace_mcp_tools("server-1", &[tool("original", "v1")])
        .unwrap();

    let mut mismatched = tool("replacement", "v2");
    mismatched.server_id = "server-2".into();
    assert!(repo.replace_mcp_tools("server-1", &[mismatched]).is_err());

    let preserved = repo.list_mcp_tools("server-1").unwrap();
    assert_eq!(preserved.len(), 1);
    assert_eq!(preserved[0].name, "original");
    assert_eq!(preserved[0].server_id, "server-1");
}

#[test]
fn deleting_mcp_server_cascades_to_tools() {
    let repo = Repo::open_in_memory().unwrap();
    repo.upsert_mcp_server(&server("Server", "healthy"))
        .unwrap();
    repo.replace_mcp_tools("server-1", &[tool("alpha", "v1")])
        .unwrap();

    assert!(repo.delete_mcp_server("server-1").unwrap());
    assert!(repo.get_mcp_server("server-1").unwrap().is_none());
    assert!(repo.list_mcp_tools("server-1").unwrap().is_empty());
    assert!(!repo.delete_mcp_server("server-1").unwrap());
}

#[test]
fn repository_reopen_preserves_agent_and_mcp_rows() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("lumo.db");
    {
        let repo = Repo::open(&path).unwrap();
        repo.create_agent_run(&run("run-1")).unwrap();
        let payload = json!({"durable": true});
        repo.append_agent_event(AgentEventInsert {
            run_id: "run-1",
            seq: 1,
            kind: "checkpoint",
            node_id: Some("node-1"),
            parent_node_id: None,
            payload: &payload,
        })
        .unwrap();
        repo.upsert_mcp_server(&server("Server", "healthy"))
            .unwrap();
        repo.replace_mcp_tools("server-1", &[tool("alpha", "v1")])
            .unwrap();
    }

    let reopened = Repo::open(&path).unwrap();
    assert_eq!(reopened.list_agent_events("run-1", 0).unwrap().len(), 1);
    assert!(reopened.get_mcp_server("server-1").unwrap().is_some());
    assert_eq!(reopened.list_mcp_tools("server-1").unwrap().len(), 1);
}

#[test]
fn malformed_mcp_server_config_returns_storage_error() {
    let repo = Repo::open_in_memory().unwrap();
    repo.upsert_mcp_server(&server("Server", "healthy"))
        .unwrap();
    repo.with_raw(|conn| {
        conn.execute(
            "UPDATE mcp_servers SET config_json='{' WHERE id='server-1'",
            [],
        )
    })
    .unwrap();

    assert!(matches!(
        repo.get_mcp_server("server-1"),
        Err(StorageError::Sqlite(_))
    ));
}

#[test]
fn malformed_agent_event_payload_returns_storage_error() {
    let repo = Repo::open_in_memory().unwrap();
    repo.create_agent_run(&run("run-1")).unwrap();
    let payload = json!({"valid": true});
    repo.append_agent_event(AgentEventInsert {
        run_id: "run-1",
        seq: 1,
        kind: "checkpoint",
        node_id: None,
        parent_node_id: None,
        payload: &payload,
    })
    .unwrap();
    repo.with_raw(|conn| {
        conn.execute(
            "UPDATE agent_events SET payload='{' WHERE run_id='run-1' AND seq=1",
            [],
        )
    })
    .unwrap();

    assert!(matches!(
        repo.list_agent_events("run-1", 0),
        Err(StorageError::Sqlite(_))
    ));
}
