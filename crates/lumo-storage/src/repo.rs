//! Repository: thin sync wrapper around `rusqlite` for the cli/vm.
//!
//! Concurrency: SQLite + WAL allows many readers + one writer. We expose a
//! `Repo { conn: Mutex<Connection> }`; for the CLI/single-worker MVP that's
//! plenty. M3 will introduce a connection pool / writer task.

use crate::{
    error::StorageError,
    schema,
    types::{AiCallInsert, AiCallRow, ArtifactRow, FlowRunRow, StepRunRow},
};
use chrono::{DateTime, TimeZone, Utc};
use parking_lot::Mutex;
use rusqlite::{params, Connection, OptionalExtension};
use std::{path::Path, sync::Arc};

#[derive(Clone)]
pub struct Repo {
    inner: Arc<Mutex<Connection>>,
}

impl Repo {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, StorageError> {
        if let Some(parent) = path.as_ref().parent() {
            std::fs::create_dir_all(parent)?;
        }
        let conn = Connection::open(&path)?;
        migrate_step_runs(&conn)?;
        conn.execute_batch(schema::DDL)?;
        Ok(Self {
            inner: Arc::new(Mutex::new(conn)),
        })
    }

    pub fn open_in_memory() -> Result<Self, StorageError> {
        let conn = Connection::open_in_memory()?;
        migrate_step_runs(&conn)?;
        conn.execute_batch(schema::DDL)?;
        Ok(Self {
            inner: Arc::new(Mutex::new(conn)),
        })
    }

    // ─── flows ──────────────────────────────────────────────────────────────
    pub fn upsert_flow(
        &self,
        id: &str,
        version: &str,
        yaml: &str,
        hash: &[u8],
        tags: &[String],
    ) -> Result<(), StorageError> {
        let now = Utc::now().timestamp();
        let tags_json = serde_json::to_string(tags)?;
        let c = self.inner.lock();
        c.execute(
            "INSERT INTO flows(id,version,yaml,hash,created_at,updated_at,tags) \
             VALUES (?,?,?,?,?,?,?) \
             ON CONFLICT(id,version) DO UPDATE SET \
               yaml=excluded.yaml, hash=excluded.hash, \
               updated_at=excluded.updated_at, tags=excluded.tags",
            params![id, version, yaml, hash, now, now, tags_json],
        )?;
        Ok(())
    }

    // ─── flow_runs ──────────────────────────────────────────────────────────
    pub fn create_run(&self, row: &FlowRunRow) -> Result<(), StorageError> {
        let c = self.inner.lock();
        c.execute(
            "INSERT INTO flow_runs(id,flow_id,flow_version,trigger_kind,inputs,outputs,state,worker_id,started_at,finished_at,cost_token,cost_usd_micro,trace_id) \
             VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?)",
            params![
                row.id, row.flow_id, row.flow_version, row.trigger_kind,
                serde_json::to_string(&row.inputs)?,
                row.outputs.as_ref().map(serde_json::to_string).transpose()?,
                row.state, row.worker_id,
                row.started_at.map(|t| t.timestamp_millis()),
                row.finished_at.map(|t| t.timestamp_millis()),
                row.cost_token, row.cost_usd_micro, row.trace_id,
            ],
        )?;
        Ok(())
    }

    pub fn finish_run(
        &self,
        run_id: &str,
        state: &str,
        outputs: Option<&serde_json::Value>,
    ) -> Result<(), StorageError> {
        let c = self.inner.lock();
        c.execute(
            "UPDATE flow_runs SET state=?, finished_at=?, outputs=? WHERE id=?",
            params![
                state,
                Utc::now().timestamp_millis(),
                outputs.map(serde_json::to_string).transpose()?,
                run_id,
            ],
        )?;
        Ok(())
    }

    pub fn list_runs(&self, limit: u32) -> Result<Vec<FlowRunRow>, StorageError> {
        let c = self.inner.lock();
        let mut stmt = c.prepare(
            "SELECT id,flow_id,flow_version,trigger_kind,inputs,outputs,state,worker_id,started_at,finished_at,cost_token,cost_usd_micro,trace_id \
             FROM flow_runs ORDER BY started_at DESC NULLS LAST LIMIT ?",
        )?;
        let rows = stmt.query_map([limit], row_to_flow_run)?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    pub fn get_run(&self, id: &str) -> Result<Option<FlowRunRow>, StorageError> {
        let c = self.inner.lock();
        let mut stmt = c.prepare(
            "SELECT id,flow_id,flow_version,trigger_kind,inputs,outputs,state,worker_id,started_at,finished_at,cost_token,cost_usd_micro,trace_id \
             FROM flow_runs WHERE id=?",
        )?;
        let row = stmt.query_row([id], row_to_flow_run).optional()?;
        Ok(row)
    }

    // ─── step_runs ──────────────────────────────────────────────────────────
    pub fn insert_step(&self, row: &StepRunRow) -> Result<(), StorageError> {
        let c = self.inner.lock();
        c.execute(
            "INSERT INTO step_runs(flow_run_id,seq,path,parent_path,depth,step_id,idx,state,attempt,input_hash,output_json,error,started_at,finished_at,span_id) \
             VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)",
            params![
                row.flow_run_id,
                row.seq,
                row.path,
                row.parent_path,
                row.depth,
                row.step_id,
                row.idx,
                row.state,
                row.attempt,
                row.input_hash,
                row.output_json.as_ref().map(serde_json::to_string).transpose()?,
                row.error,
                row.started_at.map(|t| t.timestamp_millis()),
                row.finished_at.map(|t| t.timestamp_millis()),
                row.span_id,
            ],
        )?;
        Ok(())
    }

    pub fn list_steps(&self, run_id: &str) -> Result<Vec<StepRunRow>, StorageError> {
        let c = self.inner.lock();
        let mut stmt = c.prepare(
            "SELECT flow_run_id,seq,path,parent_path,depth,step_id,idx,state,attempt,input_hash,output_json,error,started_at,finished_at,span_id \
             FROM step_runs WHERE flow_run_id=? ORDER BY seq ASC",
        )?;
        let rows = stmt.query_map([run_id], row_to_step_run)?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    // ─── artifacts (X-06 / X-07: screenshots, DOM, HAR) ────────────────────
    pub fn insert_artifact(&self, row: &ArtifactRow) -> Result<(), StorageError> {
        let c = self.inner.lock();
        c.execute(
            "INSERT INTO artifacts(id,flow_run_id,step_id,kind,mime,size,blob_path,sha256,created_at) \
             VALUES (?,?,?,?,?,?,?,?,?)",
            params![
                row.id,
                row.flow_run_id,
                row.step_id,
                row.kind,
                row.mime,
                row.size,
                row.blob_path,
                row.sha256,
                row.created_at.timestamp_millis(),
            ],
        )?;
        Ok(())
    }

    pub fn list_artifacts(&self, run_id: &str) -> Result<Vec<ArtifactRow>, StorageError> {
        let c = self.inner.lock();
        let mut stmt = c.prepare(
            "SELECT id,flow_run_id,step_id,kind,mime,size,blob_path,sha256,created_at \
             FROM artifacts WHERE flow_run_id=? ORDER BY created_at ASC, id ASC",
        )?;
        let rows = stmt.query_map([run_id], row_to_artifact)?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    /// Fetch a single artifact row by its id. Used by the desktop replay panel
    /// to resolve a `blob_path` before streaming the bytes back to the webview.
    pub fn get_artifact(&self, id: &str) -> Result<Option<ArtifactRow>, StorageError> {
        let c = self.inner.lock();
        let row = c
            .query_row(
                "SELECT id,flow_run_id,step_id,kind,mime,size,blob_path,sha256,created_at \
                 FROM artifacts WHERE id=?",
                [id],
                row_to_artifact,
            )
            .optional()?;
        Ok(row)
    }

    // ─── ai_calls (X-10) ────────────────────────────────────────────────────
    pub fn record_ai_call(&self, row: AiCallInsert<'_>) -> Result<(), StorageError> {
        let c = self.inner.lock();
        c.execute(
            "INSERT INTO ai_calls(flow_run_id,step_id,helper,provider,model,input_tokens,output_tokens,latency_ms,cost_usd_micro,created_at) \
             VALUES (?,?,?,?,?,?,?,?,?,?)",
            params![
                row.flow_run_id,
                row.step_id,
                row.helper,
                row.provider,
                row.model,
                row.input_tokens,
                row.output_tokens,
                row.latency_ms,
                row.cost_usd_micro,
                Utc::now().timestamp_millis(),
            ],
        )?;
        Ok(())
    }

    pub fn list_ai_calls(&self, run_id: &str) -> Result<Vec<AiCallRow>, StorageError> {
        let c = self.inner.lock();
        let mut stmt = c.prepare(
            "SELECT id,flow_run_id,step_id,helper,provider,model,input_tokens,output_tokens,latency_ms,cost_usd_micro,created_at \
             FROM ai_calls WHERE flow_run_id=? ORDER BY id ASC",
        )?;
        let rows = stmt.query_map([run_id], row_to_ai_call)?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    /// Roll up the ai_calls bucket into the flow_runs cost columns. Called by
    /// the VM when the flow finishes so `lumo runs list` reflects the right
    /// totals without recomputing per call.
    pub fn rollup_run_cost(&self, run_id: &str) -> Result<(i64, i64), StorageError> {
        let c = self.inner.lock();
        let (tokens, cost): (Option<i64>, Option<i64>) = c.query_row(
            "SELECT SUM(input_tokens + output_tokens), SUM(cost_usd_micro) FROM ai_calls WHERE flow_run_id=?",
            [run_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        let tokens = tokens.unwrap_or(0);
        let cost = cost.unwrap_or(0);
        c.execute(
            "UPDATE flow_runs SET cost_token=?, cost_usd_micro=? WHERE id=?",
            params![tokens, cost, run_id],
        )?;
        Ok((tokens, cost))
    }
}

fn row_to_ai_call(row: &rusqlite::Row<'_>) -> rusqlite::Result<AiCallRow> {
    Ok(AiCallRow {
        id: row.get(0)?,
        flow_run_id: row.get(1)?,
        step_id: row.get(2)?,
        helper: row.get(3)?,
        provider: row.get(4)?,
        model: row.get(5)?,
        input_tokens: row.get(6)?,
        output_tokens: row.get(7)?,
        latency_ms: row.get(8)?,
        cost_usd_micro: row.get(9)?,
        created_at: Utc
            .timestamp_millis_opt(row.get::<_, i64>(10)?)
            .single()
            .unwrap_or_else(Utc::now),
    })
}

fn row_to_artifact(row: &rusqlite::Row<'_>) -> rusqlite::Result<ArtifactRow> {
    Ok(ArtifactRow {
        id: row.get(0)?,
        flow_run_id: row.get(1)?,
        step_id: row.get(2)?,
        kind: row.get(3)?,
        mime: row.get(4)?,
        size: row.get(5)?,
        blob_path: row.get(6)?,
        sha256: row.get(7)?,
        created_at: Utc
            .timestamp_millis_opt(row.get::<_, i64>(8)?)
            .single()
            .unwrap_or_else(Utc::now),
    })
}

fn row_to_flow_run(row: &rusqlite::Row<'_>) -> rusqlite::Result<FlowRunRow> {
    Ok(FlowRunRow {
        id: row.get(0)?,
        flow_id: row.get(1)?,
        flow_version: row.get(2)?,
        trigger_kind: row.get(3)?,
        inputs: json_col(row, 4)?,
        outputs: json_opt(row, 5)?,
        state: row.get(6)?,
        worker_id: row.get(7)?,
        started_at: ts_opt(row, 8)?,
        finished_at: ts_opt(row, 9)?,
        cost_token: row.get(10)?,
        cost_usd_micro: row.get(11)?,
        trace_id: row.get(12)?,
    })
}

fn row_to_step_run(row: &rusqlite::Row<'_>) -> rusqlite::Result<StepRunRow> {
    Ok(StepRunRow {
        flow_run_id: row.get(0)?,
        seq: row.get(1)?,
        path: row.get(2)?,
        parent_path: row.get(3)?,
        depth: row.get(4)?,
        step_id: row.get(5)?,
        idx: row.get(6)?,
        state: row.get(7)?,
        attempt: row.get(8)?,
        input_hash: row.get(9)?,
        output_json: json_opt(row, 10)?,
        error: row.get(11)?,
        started_at: ts_opt(row, 12)?,
        finished_at: ts_opt(row, 13)?,
        span_id: row.get(14)?,
    })
}

fn migrate_step_runs(conn: &Connection) -> Result<(), StorageError> {
    let exists: Option<String> = conn
        .query_row(
            "SELECT name FROM sqlite_master WHERE type='table' AND name='step_runs'",
            [],
            |row| row.get(0),
        )
        .optional()?;
    if exists.is_none() {
        return Ok(());
    }
    let mut stmt = conn.prepare("PRAGMA table_info(step_runs)")?;
    let cols = stmt.query_map([], |row| row.get::<_, String>(1))?;
    let mut has_seq = false;
    for col in cols {
        if col? == "seq" {
            has_seq = true;
            break;
        }
    }
    if has_seq {
        return Ok(());
    }

    conn.execute_batch(
        r#"
        ALTER TABLE step_runs RENAME TO step_runs_old;
        CREATE TABLE step_runs (
          flow_run_id   TEXT NOT NULL,
          seq           INTEGER NOT NULL,
          path          TEXT NOT NULL,
          parent_path   TEXT,
          depth         INTEGER NOT NULL DEFAULT 0,
          step_id       TEXT NOT NULL,
          idx           INTEGER NOT NULL,
          state         TEXT NOT NULL,
          attempt       INTEGER NOT NULL DEFAULT 1,
          input_hash    BLOB NOT NULL,
          output_json   TEXT,
          error         TEXT,
          started_at    INTEGER,
          finished_at   INTEGER,
          span_id       TEXT,
          PRIMARY KEY (flow_run_id, seq),
          FOREIGN KEY (flow_run_id) REFERENCES flow_runs(id) ON DELETE CASCADE
        );
        INSERT INTO step_runs(flow_run_id,seq,path,parent_path,depth,step_id,idx,state,attempt,input_hash,output_json,error,started_at,finished_at,span_id)
        SELECT flow_run_id,rowid,step_id,NULL,0,step_id,idx,state,attempt,input_hash,output_json,error,started_at,finished_at,span_id
          FROM step_runs_old;
        DROP TABLE step_runs_old;
        CREATE INDEX IF NOT EXISTS idx_step_runs_flow_path
          ON step_runs(flow_run_id, path);
        "#,
    )?;
    Ok(())
}

fn json_col(row: &rusqlite::Row<'_>, idx: usize) -> rusqlite::Result<serde_json::Value> {
    let s: String = row.get(idx)?;
    serde_json::from_str(&s).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(idx, rusqlite::types::Type::Text, Box::new(e))
    })
}

fn json_opt(row: &rusqlite::Row<'_>, idx: usize) -> rusqlite::Result<Option<serde_json::Value>> {
    let s: Option<String> = row.get(idx)?;
    s.map(|s| serde_json::from_str(&s))
        .transpose()
        .map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(idx, rusqlite::types::Type::Text, Box::new(e))
        })
}

fn ts_opt(row: &rusqlite::Row<'_>, idx: usize) -> rusqlite::Result<Option<DateTime<Utc>>> {
    let v: Option<i64> = row.get(idx)?;
    Ok(v.and_then(|ms| Utc.timestamp_millis_opt(ms).single()))
}
