//! SQLite read / write actions (`db.*`).
//!
//! Reads honor `fs.read`; writes honor `fs.write`. Each call opens the file
//! fresh — single-flow scripts don't need pooling, and not holding a long-
//! lived connection plays nicer with concurrent flows.

use async_trait::async_trait;
use lumo_core::error::StepError;
use lumo_core::{Action, ActionRegistry, ActionResult, ResourceFactory, RunTeardown, StepCtx};
use lumo_dsl::ResourceDecl;
use once_cell::sync::Lazy;
use parking_lot::Mutex;
use rusqlite::{params_from_iter, types::ValueRef, Connection, OpenFlags};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Map, Value};
use std::path::PathBuf;
use std::sync::Arc;

use crate::resource_store::ResourceStore;

pub fn register(r: &mut ActionRegistry) {
    r.register(SqliteQueryAction);
    r.register(SqliteExecAction);
    r.register(SqliteBatchAction);
    // T3: a `spec.resources.<name>` of kind `sqlite` is one connection opened
    // once per run and reused by every step that binds to it, then reclaimed at
    // run end. Unbound steps keep opening a fresh connection per call (back-compat).
    r.register_teardown(Arc::new(SqliteTeardown));
    r.register_resource_factory(Arc::new(SqliteFactory));
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

/// Run a prepared SELECT and collect up to `limit` rows as JSON objects. Shared
/// by the bound (reused resource connection) and unbound (per-call) query paths.
fn run_query(
    conn: &Connection,
    sql: &str,
    binds: Vec<rusqlite::types::Value>,
    limit: usize,
) -> Result<Vec<Value>, StepError> {
    let mut stmt = conn
        .prepare(sql)
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
}

/// Run `prepared` statements in ONE transaction (rollback on any error). Shared
/// by the bound and unbound batch paths; needs `&mut Connection` for the tx.
fn run_batch(
    conn: &mut Connection,
    prepared: Vec<(String, Vec<rusqlite::types::Value>)>,
) -> Result<Vec<usize>, StepError> {
    let tx = conn
        .transaction()
        .map_err(|e| StepError::msg(format!("begin tx: {e}")))?;
    let mut counts = Vec::with_capacity(prepared.len());
    for (i, (sql, binds)) in prepared.into_iter().enumerate() {
        let n = tx
            .execute(&sql, params_from_iter(binds))
            // Err 提前 return ⇒ `tx` drop ⇒ 自动 ROLLBACK,整批不落盘。
            .map_err(|e| StepError::msg(format!("statement {i}: {e}")))?;
        counts.push(n);
    }
    tx.commit()
        .map_err(|e| StepError::msg(format!("commit: {e}")))?;
    Ok(counts)
}

// ─── T3: `sqlite` resource — one connection per run, reused across steps ───────
//
// A `spec.resources.<name>: {kind: sqlite, path: ...}` is opened once on the
// first step that binds to it (`Step.resource`), kept in `SQLITE_CONNS` keyed by
// `(run_id, name)`, reused by every later bound step, and dropped at run end by
// [`SqliteTeardown`] (rusqlite closes the connection on `Drop`). A step that
// binds nothing is UNBOUND and keeps the original behavior — a fresh connection
// per call — which is deliberate: concurrent flows must not share a handle, and
// reuse here is per-run-scoped, so distinct runs still get distinct connections.
//
// The shared connection is read-write (it serves both query and exec/batch
// steps). To keep `db.sqlite_query`'s read-only guarantee on it — a query must
// never write, even against the RW handle — each bound op sets `PRAGMA
// query_only` to the value it needs (queries ON, writes OFF) while holding the
// connection lock; the `Mutex` serializes use, so the toggle can't leak across
// concurrent ops.

const SQLITE_KIND: &str = "sqlite";

/// Connections opened for `sqlite` resources, keyed `(run_id, resource name)`.
/// `Mutex<Connection>` because `Connection` is `Send` but not `Sync`, and the
/// blocking SQL runs while holding the guard inside `spawn_blocking`.
static SQLITE_CONNS: ResourceStore<Mutex<Connection>> = ResourceStore::new();

/// The `sqlite` resource the step binds to, or `None` when it binds nothing (or
/// binds a non-`sqlite` / undeclared resource) — in which case the step runs
/// unbound (per-call), exactly as before. `Some(name)` selects the shared
/// connection at that slot.
fn sqlite_slot(ctx: &StepCtx) -> Option<String> {
    let name = ctx.current_resource()?;
    match ctx.resource_decl(&name) {
        Ok(decl) if decl.kind == SQLITE_KIND => Some(name),
        _ => None,
    }
}

/// The database path declared by a `sqlite` resource (`path:` under the resource,
/// flattened into `config`). Errors if absent — a `sqlite` resource with no path
/// can't be opened.
fn sqlite_path_from_decl(decl: &ResourceDecl) -> Result<PathBuf, StepError> {
    decl.config
        .get("path")
        .and_then(|v| v.as_str())
        .map(PathBuf::from)
        .ok_or_else(|| StepError::msg("sqlite resource requires a `path`"))
}

/// Resolve `(slot, path)` for a db step: a step bound to a `sqlite` resource uses
/// the resource's declared path (its own `db` field, if any, is ignored); an
/// unbound step uses its `db` field, which is then required. `slot.is_some()` ⇒
/// reuse the shared connection; `None` ⇒ open per-call.
fn resolve_target(
    ctx: &StepCtx,
    step_db: Option<PathBuf>,
) -> Result<(Option<String>, PathBuf), StepError> {
    match sqlite_slot(ctx) {
        Some(name) => {
            let path = sqlite_path_from_decl(&ctx.resource_decl(&name)?)?;
            Ok((Some(name), path))
        }
        None => {
            let path = step_db.ok_or_else(|| {
                StepError::msg("db.sqlite_*: `db` is required (or bind a `sqlite` resource)")
            })?;
            Ok((None, path))
        }
    }
}

/// Open (once) and return the shared connection for a `sqlite` resource at
/// `(run_id, slot)`, reusing it on later calls. Idempotent: on a concurrent open
/// the first connection wins and the loser is dropped (closed) by the store.
async fn ensure_conn(
    run_id: &str,
    slot: &str,
    path: PathBuf,
) -> Result<Arc<Mutex<Connection>>, StepError> {
    if let Some(conn) = SQLITE_CONNS.get(run_id, slot) {
        return Ok(conn);
    }
    // rusqlite is blocking — open off the async executor.
    let opened = tokio::task::spawn_blocking(move || {
        Connection::open(&path).map_err(|e| StepError::msg(format!("open {}: {e}", path.display())))
    })
    .await
    .map_err(|e| StepError::msg(format!("sqlite open task: {e}")))??;
    Ok(SQLITE_CONNS.get_or_put(run_id, slot, Arc::new(Mutex::new(opened))))
}

/// Drop every `sqlite` connection opened for `run_id` (rusqlite closes each on
/// `Drop`). Idempotent — a no-op when the run opened none. This is the end-of-run
/// teardown body; also exposed for tests.
#[doc(hidden)]
pub fn close_run_connections(run_id: &str) {
    let _ = SQLITE_CONNS.take_run(run_id);
}

/// Whether any `sqlite` resource connection is currently open for `run_id`.
/// Exposed for tests; not part of the action surface.
#[doc(hidden)]
pub fn sqlite_conn_open(run_id: &str) -> bool {
    SQLITE_CONNS.has_run(run_id)
}

/// End-of-run hook: closes every `sqlite` resource connection for the run so a
/// failing or forgetful flow can't leak file handles.
struct SqliteTeardown;

#[async_trait]
impl RunTeardown for SqliteTeardown {
    async fn teardown(&self, run_id: &str) {
        close_run_connections(run_id);
    }
}

/// T3 resource factory for `sqlite`: opening a declared db resource is
/// `ensure_conn` keyed by the resource name; the live connection stays in
/// `SQLITE_CONNS`, reclaimed by [`SqliteTeardown`].
struct SqliteFactory;

#[async_trait]
impl ResourceFactory for SqliteFactory {
    fn kind(&self) -> &str {
        SQLITE_KIND
    }

    async fn open(&self, decl: &ResourceDecl, run_id: &str, name: &str) -> Result<(), StepError> {
        let path = sqlite_path_from_decl(decl)?;
        let _ = ensure_conn(run_id, name, path).await?;
        Ok(())
    }
}

pub struct SqliteQueryAction;
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct QueryIn {
    /// SQLite file to read. Omit when the step binds a `sqlite` resource — the
    /// resource's declared `path` is used (and a reused connection at that).
    #[serde(default)]
    db: Option<PathBuf>,
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
        static S: Lazy<Value> = Lazy::new(crate::schema::derive::<QueryIn>);
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
        let (slot, path) = resolve_target(ctx, db)?;
        ctx.ensure_fs_read(&path)?;
        let binds = bind_params(&args)?;
        let rows = match slot {
            // Bound: reuse the run's shared connection. Keep the query read-only
            // on the RW handle (`query_only` under the lock) so a query can't write.
            Some(name) => {
                let conn = ensure_conn(ctx.run_id(), &name, path).await?;
                tokio::task::spawn_blocking(move || -> Result<Vec<Value>, StepError> {
                    let guard = conn.lock();
                    guard
                        .execute_batch("PRAGMA query_only = ON;")
                        .map_err(|e| StepError::msg(format!("query_only: {e}")))?;
                    run_query(&guard, &sql, binds, limit)
                })
                .await
                .map_err(|e| StepError::msg(format!("sqlite task: {e}")))??
            }
            // Unbound: a fresh read-only connection per call, exactly as before.
            None => tokio::task::spawn_blocking(move || -> Result<Vec<Value>, StepError> {
                let conn = Connection::open_with_flags(&path, OpenFlags::SQLITE_OPEN_READ_ONLY)
                    .map_err(|e| StepError::msg(format!("open {}: {e}", path.display())))?;
                run_query(&conn, &sql, binds, limit)
            })
            .await
            .map_err(|e| StepError::msg(format!("sqlite task: {e}")))??,
        };
        let truncated = rows.len() == limit;
        Ok(ActionResult::from(serde_json::json!({
            "rows": rows,
            "count": rows.len(),
            "truncated": truncated,
        })))
    }
}

pub struct SqliteExecAction;
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ExecIn {
    /// SQLite file to write. Omit when the step binds a `sqlite` resource.
    #[serde(default)]
    db: Option<PathBuf>,
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
        static S: Lazy<Value> = Lazy::new(crate::schema::derive::<ExecIn>);
        &S
    }
    async fn execute(&self, ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let ExecIn { db, sql, args } = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("db.sqlite_exec invalid: {e}")))?;
        let (slot, path) = resolve_target(ctx, db)?;
        ctx.ensure_fs_write(&path)?;
        let binds = bind_params(&args)?;
        let n = match slot {
            // Bound: reuse the run's shared connection (writes ⇒ query_only OFF).
            Some(name) => {
                let conn = ensure_conn(ctx.run_id(), &name, path).await?;
                tokio::task::spawn_blocking(move || -> Result<usize, StepError> {
                    let guard = conn.lock();
                    guard
                        .execute_batch("PRAGMA query_only = OFF;")
                        .map_err(|e| StepError::msg(format!("query_only: {e}")))?;
                    guard
                        .execute(&sql, params_from_iter(binds))
                        .map_err(|e| StepError::msg(format!("exec: {e}")))
                })
                .await
                .map_err(|e| StepError::msg(format!("sqlite task: {e}")))??
            }
            // Unbound: a fresh connection per call, exactly as before.
            None => tokio::task::spawn_blocking(move || -> Result<usize, StepError> {
                let conn = Connection::open(&path)
                    .map_err(|e| StepError::msg(format!("open {}: {e}", path.display())))?;
                conn.execute(&sql, params_from_iter(binds))
                    .map_err(|e| StepError::msg(format!("exec: {e}")))
            })
            .await
            .map_err(|e| StepError::msg(format!("sqlite task: {e}")))??,
        };
        Ok(ActionResult::from(serde_json::json!({
            "rows_affected": n,
        })))
    }
}

// ─── db.sqlite_batch ──────────────────────────────────────────────────────────
// 在单个事务里顺序执行多条参数化语句:任一失败则整体回滚(返回 Err 时 `tx` 被丢弃,
// rusqlite 默认 DROP 即 ROLLBACK),全部成功才 commit。写操作走 fs.write 闸门。

pub struct SqliteBatchAction;

/// 一条批处理语句:SQL + 其参数。
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct BatchStmt {
    sql: String,
    #[serde(default)]
    args: Vec<Value>,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct BatchIn {
    /// SQLite file to write. Omit when the step binds a `sqlite` resource.
    #[serde(default)]
    db: Option<PathBuf>,
    /// 顺序执行的语句列表;同一事务内,任一失败全部回滚。
    statements: Vec<BatchStmt>,
}

#[async_trait]
impl Action for SqliteBatchAction {
    fn id(&self) -> &'static str {
        "db.sqlite_batch"
    }
    fn summary(&self) -> &'static str {
        "Run many parameterized statements in ONE transaction (rollback on error)"
    }
    fn schema(&self) -> &'static Value {
        static S: Lazy<Value> = Lazy::new(crate::schema::derive::<BatchIn>);
        &S
    }
    async fn execute(&self, ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let BatchIn { db, statements } = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("db.sqlite_batch invalid: {e}")))?;
        let (slot, path) = resolve_target(ctx, db)?;
        ctx.ensure_fs_write(&path)?;
        if statements.is_empty() {
            return Err(StepError::msg(
                "db.sqlite_batch requires at least one statement",
            ));
        }
        // 预先绑定每条语句的参数(在阻塞前做类型校验,任一坏参数直接失败)。
        let mut prepared: Vec<(String, Vec<rusqlite::types::Value>)> =
            Vec::with_capacity(statements.len());
        for s in statements {
            prepared.push((s.sql, bind_params(&s.args)?));
        }
        let affected = match slot {
            // Bound: reuse the run's shared connection (writes ⇒ query_only OFF).
            Some(name) => {
                let conn = ensure_conn(ctx.run_id(), &name, path).await?;
                tokio::task::spawn_blocking(move || -> Result<Vec<usize>, StepError> {
                    let mut guard = conn.lock();
                    guard
                        .execute_batch("PRAGMA query_only = OFF;")
                        .map_err(|e| StepError::msg(format!("query_only: {e}")))?;
                    run_batch(&mut guard, prepared)
                })
                .await
                .map_err(|e| StepError::msg(format!("sqlite task: {e}")))??
            }
            // Unbound: a fresh connection per call, exactly as before.
            None => tokio::task::spawn_blocking(move || -> Result<Vec<usize>, StepError> {
                let mut conn = Connection::open(&path)
                    .map_err(|e| StepError::msg(format!("open {}: {e}", path.display())))?;
                run_batch(&mut conn, prepared)
            })
            .await
            .map_err(|e| StepError::msg(format!("sqlite task: {e}")))??,
        };
        let total: usize = affected.iter().sum();
        Ok(ActionResult::from(serde_json::json!({
            "rows_affected": affected,
            "total_affected": total,
            "statements": affected.len(),
        })))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn decl(yaml: &str) -> ResourceDecl {
        serde_yaml::from_str(yaml).expect("valid ResourceDecl yaml")
    }

    /// A ctx carrying `spec.resources` (name → YAML decl) and an optional
    /// current-step `resource:` binding — the inputs the T3 slot resolution reads.
    fn ctx_with(resources: &[(&str, &str)], current: Option<&str>) -> StepCtx {
        let map: BTreeMap<String, ResourceDecl> = resources
            .iter()
            .map(|(name, yaml)| (name.to_string(), decl(yaml)))
            .collect();
        let ctx = StepCtx::new(
            "run-db".into(),
            "flow-db".into(),
            ActionRegistry::new(),
            None,
            Value::Null,
            lumo_dsl::Capabilities::default(),
            Vec::new(),
        )
        .with_resources(map);
        ctx.set_current_resource(current);
        ctx
    }

    #[test]
    fn sqlite_slot_selects_only_sqlite_kind_bindings() {
        let resources = &[
            ("mydb", "kind: sqlite\npath: /tmp/x.db\n"),
            ("web", "kind: chromium.cdp\n"),
        ];
        // Bound to a sqlite resource ⇒ its name is the slot (shared connection).
        assert_eq!(
            sqlite_slot(&ctx_with(resources, Some("mydb"))).as_deref(),
            Some("mydb")
        );
        // Unbound ⇒ None ⇒ the step opens per-call (back-compat).
        assert_eq!(sqlite_slot(&ctx_with(resources, None)), None);
        // Bound to a non-sqlite kind ⇒ None (never key a db by a browser's name).
        assert_eq!(sqlite_slot(&ctx_with(resources, Some("web"))), None);
        // Bound to an undeclared name ⇒ None.
        assert_eq!(sqlite_slot(&ctx_with(resources, Some("ghost"))), None);
    }

    #[test]
    fn sqlite_path_from_decl_reads_path_or_errors() {
        let p = sqlite_path_from_decl(&decl("kind: sqlite\npath: ./data/app.db\n")).unwrap();
        assert_eq!(p, PathBuf::from("./data/app.db"));
        // A sqlite resource without a `path` can't be opened.
        let err = sqlite_path_from_decl(&decl("kind: sqlite\n")).unwrap_err();
        assert!(err.to_string().contains("path"), "got: {err}");
    }

    #[test]
    fn sqlite_factory_kind_matches() {
        assert_eq!(SqliteFactory.kind(), SQLITE_KIND);
        assert_eq!(SQLITE_KIND, "sqlite");
    }
}
