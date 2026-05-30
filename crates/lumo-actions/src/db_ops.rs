//! SQLite read / write actions (`db.*`).
//!
//! Reads honor `fs.read`; writes honor `fs.write`. Each call opens the file
//! fresh — single-flow scripts don't need pooling, and not holding a long-
//! lived connection plays nicer with concurrent flows.

use async_trait::async_trait;
use lumo_core::error::StepError;
use lumo_core::{Action, ActionRegistry, ActionResult, StepCtx};
use once_cell::sync::Lazy;
use rusqlite::{params_from_iter, types::ValueRef, Connection, OpenFlags};
use serde::Deserialize;
use serde_json::{Map, Value};
use std::path::PathBuf;

pub fn register(r: &mut ActionRegistry) {
    r.register(SqliteQueryAction);
    r.register(SqliteExecAction);
}

fn bind_params(args: &[Value]) -> Result<Vec<rusqlite::types::Value>, StepError> {
    let mut out = Vec::with_capacity(args.len());
    for a in args {
        out.push(match a {
            Value::Null => rusqlite::types::Value::Null,
            Value::Bool(b) => rusqlite::types::Value::Integer(if *b { 1 } else { 0 }),
            Value::Number(n) => {
                if let Some(i) = n.as_i64() {
                    rusqlite::types::Value::Integer(i)
                } else if let Some(f) = n.as_f64() {
                    rusqlite::types::Value::Real(f)
                } else {
                    return Err(StepError::msg("db param: unsupported number"));
                }
            }
            Value::String(s) => rusqlite::types::Value::Text(s.clone()),
            other => rusqlite::types::Value::Text(other.to_string()),
        });
    }
    Ok(out)
}

fn row_to_value(row: &rusqlite::Row<'_>, columns: &[String]) -> rusqlite::Result<Value> {
    let mut m = Map::new();
    for (i, name) in columns.iter().enumerate() {
        let v: Value = match row.get_ref(i)? {
            ValueRef::Null => Value::Null,
            ValueRef::Integer(n) => Value::from(n),
            ValueRef::Real(f) => serde_json::Number::from_f64(f)
                .map(Value::Number)
                .unwrap_or(Value::Null),
            ValueRef::Text(t) => Value::String(String::from_utf8_lossy(t).into_owned()),
            ValueRef::Blob(_) => Value::String("<blob>".into()),
        };
        m.insert(name.clone(), v);
    }
    Ok(Value::Object(m))
}

pub struct SqliteQueryAction;
#[derive(Deserialize)]
struct QueryIn {
    db: PathBuf,
    sql: String,
    #[serde(default)]
    args: Vec<Value>,
    #[serde(default = "default_limit")]
    limit: usize,
}
fn default_limit() -> usize {
    1_000
}

#[async_trait]
impl Action for SqliteQueryAction {
    fn id(&self) -> &'static str {
        "db.sqlite_query"
    }
    fn summary(&self) -> &'static str {
        "Run a SELECT against a SQLite file; rows returned as JSON"
    }
    fn schema(&self) -> &'static Value {
        static S: Lazy<Value> = Lazy::new(|| {
            serde_json::json!({
                "type": "object",
                "required": ["db", "sql"],
                "properties": {
                    "db":    { "type": "string" },
                    "sql":   { "type": "string" },
                    "args":  { "type": "array" },
                    "limit": { "type": "integer", "minimum": 1, "default": 1000 }
                },
                "additionalProperties": false
            })
        });
        &S
    }
    async fn execute(&self, ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let QueryIn {
            db,
            sql,
            args,
            limit,
        } = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("db.sqlite_query invalid: {e}")))?;
        ctx.ensure_fs_read(&db)?;
        let path = db.clone();
        let binds = bind_params(&args)?;
        let rows = tokio::task::spawn_blocking(move || -> Result<Vec<Value>, StepError> {
            let conn = Connection::open_with_flags(&path, OpenFlags::SQLITE_OPEN_READ_ONLY)
                .map_err(|e| StepError::msg(format!("open {}: {e}", path.display())))?;
            let mut stmt = conn
                .prepare(&sql)
                .map_err(|e| StepError::msg(format!("prepare: {e}")))?;
            let columns: Vec<String> = stmt
                .column_names()
                .into_iter()
                .map(str::to_string)
                .collect();
            let cols_for_map = columns.clone();
            let mut iter = stmt
                .query_map(params_from_iter(binds), move |row| {
                    row_to_value(row, &cols_for_map)
                })
                .map_err(|e| StepError::msg(format!("query: {e}")))?;
            let mut out = Vec::new();
            for _ in 0..limit {
                match iter.next() {
                    Some(Ok(v)) => out.push(v),
                    Some(Err(e)) => return Err(StepError::msg(format!("row: {e}"))),
                    None => break,
                }
            }
            Ok(out)
        })
        .await
        .map_err(|e| StepError::msg(format!("sqlite task: {e}")))??;
        let truncated = rows.len() == limit;
        Ok(ActionResult::from(serde_json::json!({
            "rows": rows,
            "count": rows.len(),
            "truncated": truncated,
        })))
    }
}

pub struct SqliteExecAction;
#[derive(Deserialize)]
struct ExecIn {
    db: PathBuf,
    sql: String,
    #[serde(default)]
    args: Vec<Value>,
}
#[async_trait]
impl Action for SqliteExecAction {
    fn id(&self) -> &'static str {
        "db.sqlite_exec"
    }
    fn summary(&self) -> &'static str {
        "Run an INSERT/UPDATE/DELETE/DDL against a SQLite file"
    }
    fn schema(&self) -> &'static Value {
        static S: Lazy<Value> = Lazy::new(|| {
            serde_json::json!({
                "type": "object",
                "required": ["db", "sql"],
                "properties": {
                    "db":   { "type": "string" },
                    "sql":  { "type": "string" },
                    "args": { "type": "array" }
                },
                "additionalProperties": false
            })
        });
        &S
    }
    async fn execute(&self, ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let ExecIn { db, sql, args } = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("db.sqlite_exec invalid: {e}")))?;
        ctx.ensure_fs_write(&db)?;
        let path = db.clone();
        let binds = bind_params(&args)?;
        let n = tokio::task::spawn_blocking(move || -> Result<usize, StepError> {
            let conn = Connection::open(&path)
                .map_err(|e| StepError::msg(format!("open {}: {e}", path.display())))?;
            conn.execute(&sql, params_from_iter(binds))
                .map_err(|e| StepError::msg(format!("exec: {e}")))
        })
        .await
        .map_err(|e| StepError::msg(format!("sqlite task: {e}")))??;
        Ok(ActionResult::from(serde_json::json!({
            "rows_affected": n,
        })))
    }
}
