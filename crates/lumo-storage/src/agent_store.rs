use crate::{
    types::{AgentEventInsert, AgentEventRow, AgentRunRow, McpServerRow, McpToolRow},
    Repo, StorageError,
};
use chrono::{TimeZone, Utc};
use rusqlite::{params, OptionalExtension};

impl Repo {
    pub fn create_agent_run(&self, row: &AgentRunRow) -> Result<(), StorageError> {
        let conn = self.inner.lock();
        conn.execute(
            "INSERT INTO agent_runs(id,profile_id,utterance,plan_json,approval_json,state,started_at,finished_at,error) \
             VALUES (?,?,?,?,?,?,?,?,?)",
            params![
                row.id,
                row.profile_id,
                row.utterance,
                row.plan_json
                    .as_ref()
                    .map(serde_json::to_string)
                    .transpose()?,
                row.approval_json
                    .as_ref()
                    .map(serde_json::to_string)
                    .transpose()?,
                row.state,
                row.started_at.timestamp_millis(),
                row.finished_at.map(|time| time.timestamp_millis()),
                row.error,
            ],
        )?;
        Ok(())
    }

    pub fn update_agent_run_state(
        &self,
        id: &str,
        state: &str,
        finished_at: Option<chrono::DateTime<Utc>>,
        error: Option<&str>,
    ) -> Result<(), StorageError> {
        let conn = self.inner.lock();
        let updated = conn.execute(
            "UPDATE agent_runs SET state=?, finished_at=?, error=? WHERE id=?",
            params![
                state,
                finished_at.map(|time| time.timestamp_millis()),
                error,
                id,
            ],
        )?;
        if updated == 0 {
            return Err(StorageError::NotFound(format!("agent run {id}")));
        }
        Ok(())
    }

    pub fn append_agent_event(&self, row: AgentEventInsert<'_>) -> Result<(), StorageError> {
        let conn = self.inner.lock();
        conn.execute(
            "INSERT INTO agent_events(run_id,seq,kind,node_id,parent_node_id,payload,created_at) \
             VALUES (?,?,?,?,?,?,?)",
            params![
                row.run_id,
                row.seq,
                row.kind,
                row.node_id,
                row.parent_node_id,
                serde_json::to_string(row.payload)?,
                Utc::now().timestamp_millis(),
            ],
        )?;
        Ok(())
    }

    pub fn list_agent_events(
        &self,
        run_id: &str,
        after_seq: i64,
    ) -> Result<Vec<AgentEventRow>, StorageError> {
        let conn = self.inner.lock();
        let mut stmt = conn.prepare(
            "SELECT run_id,seq,kind,node_id,parent_node_id,payload,created_at \
             FROM agent_events WHERE run_id=? AND seq>? ORDER BY seq ASC",
        )?;
        let rows = stmt.query_map(params![run_id, after_seq], |row| {
            Ok(AgentEventRow {
                run_id: row.get(0)?,
                seq: row.get(1)?,
                kind: row.get(2)?,
                node_id: row.get(3)?,
                parent_node_id: row.get(4)?,
                payload: json_value(row, 5)?,
                created_at: timestamp(row, 6)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    pub fn upsert_mcp_server(&self, row: &McpServerRow) -> Result<(), StorageError> {
        let conn = self.inner.lock();
        conn.execute(
            "INSERT INTO mcp_servers(id,name,transport,config_json,enabled,health,created_at,updated_at) \
             VALUES (?,?,?,?,?,?,?,?) \
             ON CONFLICT(id) DO UPDATE SET \
               name=excluded.name, transport=excluded.transport, config_json=excluded.config_json, \
               enabled=excluded.enabled, health=excluded.health, updated_at=excluded.updated_at",
            params![
                row.id,
                row.name,
                row.transport,
                serde_json::to_string(&row.config)?,
                row.enabled,
                row.health,
                row.created_at.timestamp_millis(),
                row.updated_at.timestamp_millis(),
            ],
        )?;
        Ok(())
    }

    pub fn get_mcp_server(&self, id: &str) -> Result<Option<McpServerRow>, StorageError> {
        let conn = self.inner.lock();
        conn.query_row(
            "SELECT id,name,transport,config_json,enabled,health,created_at,updated_at \
             FROM mcp_servers WHERE id=?",
            [id],
            |row| {
                Ok(McpServerRow {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    transport: row.get(2)?,
                    config: json_value(row, 3)?,
                    enabled: row.get(4)?,
                    health: row.get(5)?,
                    created_at: timestamp(row, 6)?,
                    updated_at: timestamp(row, 7)?,
                })
            },
        )
        .optional()
        .map_err(Into::into)
    }

    pub fn list_mcp_servers(&self) -> Result<Vec<McpServerRow>, StorageError> {
        let conn = self.inner.lock();
        let mut stmt = conn.prepare(
            "SELECT id,name,transport,config_json,enabled,health,created_at,updated_at \
             FROM mcp_servers ORDER BY name COLLATE NOCASE ASC, id ASC",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(McpServerRow {
                id: row.get(0)?,
                name: row.get(1)?,
                transport: row.get(2)?,
                config: json_value(row, 3)?,
                enabled: row.get(4)?,
                health: row.get(5)?,
                created_at: timestamp(row, 6)?,
                updated_at: timestamp(row, 7)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    pub fn delete_mcp_server(&self, id: &str) -> Result<bool, StorageError> {
        let conn = self.inner.lock();
        Ok(conn.execute("DELETE FROM mcp_servers WHERE id=?", [id])? > 0)
    }

    pub fn replace_mcp_tools(
        &self,
        server_id: &str,
        tools: &[McpToolRow],
    ) -> Result<(), StorageError> {
        if let Some(tool) = tools.iter().find(|tool| tool.server_id != server_id) {
            return Err(rusqlite::Error::InvalidParameterName(format!(
                "tool {} belongs to server {}, expected {server_id}",
                tool.name, tool.server_id
            ))
            .into());
        }

        let mut conn = self.inner.lock();
        let tx = conn.transaction()?;
        let server_exists = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM mcp_servers WHERE id=?)",
            [server_id],
            |row| row.get::<_, bool>(0),
        )?;
        if !server_exists {
            return Err(StorageError::NotFound(format!("MCP server {server_id}")));
        }

        tx.execute("DELETE FROM mcp_tools WHERE server_id=?", [server_id])?;
        {
            let mut insert = tx.prepare(
                "INSERT INTO mcp_tools(server_id,name,description,input_schema,output_schema,risk,enabled,version_hash,discovered_at) \
                 VALUES (?,?,?,?,?,?,?,?,?)",
            )?;
            for tool in tools {
                insert.execute(params![
                    tool.server_id,
                    tool.name,
                    tool.description,
                    serde_json::to_string(&tool.input_schema)?,
                    tool.output_schema
                        .as_ref()
                        .map(serde_json::to_string)
                        .transpose()?,
                    tool.risk,
                    tool.enabled,
                    tool.version_hash,
                    tool.discovered_at.timestamp_millis(),
                ])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    pub fn list_mcp_tools(&self, server_id: &str) -> Result<Vec<McpToolRow>, StorageError> {
        let conn = self.inner.lock();
        let mut stmt = conn.prepare(
            "SELECT server_id,name,description,input_schema,output_schema,risk,enabled,version_hash,discovered_at \
             FROM mcp_tools WHERE server_id=? ORDER BY name ASC",
        )?;
        let rows = stmt.query_map([server_id], |row| {
            Ok(McpToolRow {
                server_id: row.get(0)?,
                name: row.get(1)?,
                description: row.get(2)?,
                input_schema: json_value(row, 3)?,
                output_schema: json_optional(row, 4)?,
                risk: row.get(5)?,
                enabled: row.get(6)?,
                version_hash: row.get(7)?,
                discovered_at: timestamp(row, 8)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }
}

fn json_value(row: &rusqlite::Row<'_>, index: usize) -> rusqlite::Result<serde_json::Value> {
    let value: String = row.get(index)?;
    serde_json::from_str(&value).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(
            index,
            rusqlite::types::Type::Text,
            Box::new(error),
        )
    })
}

fn json_optional(
    row: &rusqlite::Row<'_>,
    index: usize,
) -> rusqlite::Result<Option<serde_json::Value>> {
    let value: Option<String> = row.get(index)?;
    value
        .map(|value| serde_json::from_str(&value))
        .transpose()
        .map_err(|error| {
            rusqlite::Error::FromSqlConversionFailure(
                index,
                rusqlite::types::Type::Text,
                Box::new(error),
            )
        })
}

fn timestamp(row: &rusqlite::Row<'_>, index: usize) -> rusqlite::Result<chrono::DateTime<Utc>> {
    let millis: i64 = row.get(index)?;
    Utc.timestamp_millis_opt(millis).single().ok_or_else(|| {
        rusqlite::Error::FromSqlConversionFailure(
            index,
            rusqlite::types::Type::Integer,
            "timestamp out of range".into(),
        )
    })
}
