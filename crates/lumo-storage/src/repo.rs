//! Repository: thin sync wrapper around `rusqlite` for the cli/vm.
//!
//! Concurrency: SQLite + WAL allows many readers + one writer. We expose a
//! `Repo { conn: Mutex<Connection> }`; for the CLI/single-worker MVP that's
//! plenty. M3 will introduce a connection pool / writer task.

use crate::{
    error::StorageError,
    schema,
    types::{FlowRunRow, StepRunRow},
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
