//! SQLite read / write actions (`db.*`).
//!
//! Reads honor `fs.read`; writes honor `fs.write`. Each call opens the file
//! fresh — single-flow scripts don't need pooling, and not holding a long-
//! lived connection plays nicer with concurrent flows.

use async_trait::async_trait;
use lumo_core::error::StepError;
use lumo_core::{
    Action, ActionRegistry, ActionResult, ResourceFactory, RunTeardown, StepCtx, StepInterrupt,
};
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
    // T3: PostgreSQL / MySQL —— DSN(含凭据)来自首个绑定步骤的输入(vault 解析后),
    // 不在静态 decl 里;故工厂 `open` 仅校验、不连库,连接池由首个绑定步骤懒构建。
    register_pg(r);
    register_mysql(r);
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
    interrupt: &StepInterrupt,
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
        // P0-2:每行一个检查点 —— 步骤被判超时/取消后,孤儿阻塞任务在此提前
        // 退出并释放连接锁,不再卡住后续步骤(bound 路径共享同一把 Mutex)。
        if interrupt.is_interrupted() {
            return Err(StepError::msg("db.sqlite_query interrupted"));
        }
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
    interrupt: &StepInterrupt,
) -> Result<Vec<usize>, StepError> {
    let tx = conn
        .transaction()
        .map_err(|e| StepError::msg(format!("begin tx: {e}")))?;
    let mut counts = Vec::with_capacity(prepared.len());
    for (i, (sql, binds)) in prepared.into_iter().enumerate() {
        // P0-2:语句边界检查点 —— Err 提前 return ⇒ `tx` drop ⇒ 自动 ROLLBACK。
        if interrupt.is_interrupted() {
            return Err(StepError::msg("db.sqlite_batch interrupted (rolled back)"));
        }
        let n = tx
            .execute(&sql, params_from_iter(binds))
            // Err 提前 return ⇒ `tx` drop ⇒ 自动 ROLLBACK,整批不落盘。
            .map_err(|e| StepError::msg(format!("statement {i}: {e}")))?;
        counts.push(n);
    }
    // P0-2:commit 是副作用落地的唯一闸口,必查 —— 引擎已把这次 attempt 记成
    // timeout,这里再 commit 就意味着"状态说没写、库里却写了",重跑即重复写入。
    if interrupt.is_interrupted() {
        return Err(StepError::msg("db.sqlite_batch interrupted (rolled back)"));
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

pub(crate) const SQLITE_KIND: &str = "sqlite";

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
        // P0-2:把步级中断句柄带进阻塞闭包(只读路径也要 —— bound 连接共享
        // Mutex,长查询卡锁会饿死后续步骤)。
        let interrupt = ctx.step_interrupt();
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
                    run_query(&guard, &sql, binds, limit, &interrupt)
                })
                .await
                .map_err(|e| StepError::msg(format!("sqlite task: {e}")))??
            }
            // Unbound: a fresh read-only connection per call, exactly as before.
            None => tokio::task::spawn_blocking(move || -> Result<Vec<Value>, StepError> {
                let conn = Connection::open_with_flags(&path, OpenFlags::SQLITE_OPEN_READ_ONLY)
                    .map_err(|e| StepError::msg(format!("open {}: {e}", path.display())))?;
                run_query(&conn, &sql, binds, limit, &interrupt)
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
        // P0-2:单条写没有事务包裹,执行即落盘 —— 执行前是唯一能拦住副作用的
        // 检查点(拿到锁之后再查一次:排队等锁期间步骤可能已被判死)。
        let interrupt = ctx.step_interrupt();
        let n = match slot {
            // Bound: reuse the run's shared connection (writes ⇒ query_only OFF).
            Some(name) => {
                let conn = ensure_conn(ctx.run_id(), &name, path).await?;
                tokio::task::spawn_blocking(move || -> Result<usize, StepError> {
                    let guard = conn.lock();
                    if interrupt.is_interrupted() {
                        return Err(StepError::msg("db.sqlite_exec interrupted"));
                    }
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
                if interrupt.is_interrupted() {
                    return Err(StepError::msg("db.sqlite_exec interrupted"));
                }
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
        // P0-2:中断句柄进闭包;`run_batch` 在语句边界与 commit 前各设检查点。
        let interrupt = ctx.step_interrupt();
        let affected = match slot {
            // Bound: reuse the run's shared connection (writes ⇒ query_only OFF).
            Some(name) => {
                let conn = ensure_conn(ctx.run_id(), &name, path).await?;
                tokio::task::spawn_blocking(move || -> Result<Vec<usize>, StepError> {
                    let mut guard = conn.lock();
                    guard
                        .execute_batch("PRAGMA query_only = OFF;")
                        .map_err(|e| StepError::msg(format!("query_only: {e}")))?;
                    run_batch(&mut guard, prepared, &interrupt)
                })
                .await
                .map_err(|e| StepError::msg(format!("sqlite task: {e}")))??
            }
            // Unbound: a fresh connection per call, exactly as before.
            None => tokio::task::spawn_blocking(move || -> Result<Vec<usize>, StepError> {
                let mut conn = Connection::open(&path)
                    .map_err(|e| StepError::msg(format!("open {}: {e}", path.display())))?;
                run_batch(&mut conn, prepared, &interrupt)
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

// ════════════════════════════════════════════════════════════════════════════
//  PostgreSQL / MySQL —— T3 网络型 db 资源(sqlx,运行期查询)
// ════════════════════════════════════════════════════════════════════════════
//
// 与上面的 sqlite 同一套 T3 范式,但连接标的是「网络」而非「文件」:
//   * 标的不是 path 而是 DSN(`postgres://user:pass@host:5432/db` /
//     `mysql://user:pass@host:3306/db`),DSN 含凭据,来自首个绑定步骤的输入
//     (vault 解析后),**不在静态 decl 里** —— 故工厂 `open` 只校验、不连库
//     (镜像 ftp/smtp 的「机密承载 kind」模型)。
//   * 门禁走 `network`(非 fs):`ctx.ensure_network_url(&dsn)`,DSN 本身就是
//     `scheme://host…` URL,`extract_host` 取得 host —— 与 transfer.rs gate
//     `ftp://{host}`、email.rs gate 其 host 同理。在 open 与每次未绑定调用上都做。
//   * 句柄是 sqlx 连接池(`PgPool`/`MySqlPool`,`Clone + Send + Sync`),不需像
//     sqlite 那样套 `Mutex`;池在 `Drop` 时关闭,异步 teardown 里再 `close().await`
//     做优雅收口(镜像 ftp 的异步 `close_run`)。
//
// 绑定:首个绑定步骤的 DSN 建共享池,后续绑定步骤复用(其自带 DSN 被忽略);
// 未绑定步骤每次按自身 DSN 现连(与 sqlite 的 bound/unbound 二元性完全一致)。
//
// 占位符按用户原样透传:Postgres 用 `$1,$2,…`,MySQL 用 `?` —— 不改写。
// 仅支持运行期 SQL 字符串(`sqlx::query`/`query` + 手动取列),不用 `query!` 宏,
// 故构建期无需 DATABASE_URL。

use sqlx::{Column, Row, TypeInfo, ValueRef as SqlxValueRef};

pub(crate) const PG_KIND: &str = "postgres";
pub(crate) const MYSQL_KIND: &str = "mysql";

/// `postgres` 资源的连接池,键 `(run_id, resource name)`。`PgPool` 本身是
/// `Clone + Send + Sync`(内部 `Arc`),无需 `Mutex`。
static PG_POOLS: ResourceStore<sqlx::PgPool> = ResourceStore::new();
/// `mysql` 资源的连接池,键 `(run_id, resource name)`。
static MYSQL_POOLS: ResourceStore<sqlx::MySqlPool> = ResourceStore::new();

fn register_pg(r: &mut ActionRegistry) {
    r.register(PgQueryAction);
    r.register(PgExecAction);
    r.register_teardown(Arc::new(PgTeardown));
    r.register_resource_factory(Arc::new(PgFactory));
}

fn register_mysql(r: &mut ActionRegistry) {
    r.register(MysqlQueryAction);
    r.register(MysqlExecAction);
    r.register_teardown(Arc::new(MysqlTeardown));
    r.register_resource_factory(Arc::new(MysqlFactory));
}

/// 该步骤绑定的 `postgres` 资源名,或 `None`(未绑定 / 绑定了非 postgres /
/// 未声明的资源)—— 后者走每调用现连(back-compat)。镜像 `sqlite_slot`。
fn pg_slot(ctx: &StepCtx) -> Option<String> {
    let name = ctx.current_resource()?;
    match ctx.resource_decl(&name) {
        Ok(decl) if decl.kind == PG_KIND => Some(name),
        _ => None,
    }
}

/// 该步骤绑定的 `mysql` 资源名,或 `None`(同 `pg_slot`)。
fn mysql_slot(ctx: &StepCtx) -> Option<String> {
    let name = ctx.current_resource()?;
    match ctx.resource_decl(&name) {
        Ok(decl) if decl.kind == MYSQL_KIND => Some(name),
        _ => None,
    }
}

/// 开一次(并复用)`(run_id, slot)` 处的共享 PgPool;首个绑定步骤的 DSN 立池。
/// 幂等:并发开池时第一名胜出,落败者池被 `get_or_put` drop(`Drop` 即关池)。
async fn ensure_pg_pool(
    run_id: &str,
    slot: &str,
    dsn: &str,
) -> Result<Arc<sqlx::PgPool>, StepError> {
    if let Some(p) = PG_POOLS.get(run_id, slot) {
        return Ok(p);
    }
    let pool = sqlx::PgPool::connect(dsn)
        .await
        // 故意不回原始错误:DSN 含凭据,sqlx 错误可能回显连接串片段。
        .map_err(|_| StepError::msg("postgres connect failed"))?;
    Ok(PG_POOLS.get_or_put(run_id, slot, Arc::new(pool)))
}

/// 开一次(并复用)`(run_id, slot)` 处的共享 MySqlPool。语义同 `ensure_pg_pool`。
async fn ensure_mysql_pool(
    run_id: &str,
    slot: &str,
    dsn: &str,
) -> Result<Arc<sqlx::MySqlPool>, StepError> {
    if let Some(p) = MYSQL_POOLS.get(run_id, slot) {
        return Ok(p);
    }
    let pool = sqlx::MySqlPool::connect(dsn)
        .await
        .map_err(|_| StepError::msg("mysql connect failed"))?;
    Ok(MYSQL_POOLS.get_or_put(run_id, slot, Arc::new(pool)))
}

/// 关闭并丢弃 `run_id` 名下所有 postgres 池(`pool.close().await` 优雅收口,
/// 镜像 ftp 的异步 teardown)。幂等 —— 未开池时空跑。也供测试使用。
#[doc(hidden)]
pub async fn close_run_pg_pools(run_id: &str) {
    for pool in PG_POOLS.take_run(run_id) {
        pool.close().await;
    }
}

/// 关闭并丢弃 `run_id` 名下所有 mysql 池。语义同 `close_run_pg_pools`。
#[doc(hidden)]
pub async fn close_run_mysql_pools(run_id: &str) {
    for pool in MYSQL_POOLS.take_run(run_id) {
        pool.close().await;
    }
}

/// 是否有 postgres 池在 `run_id` 名下打开(any slot)。供测试使用。
#[doc(hidden)]
pub fn pg_pool_open(run_id: &str) -> bool {
    PG_POOLS.has_run(run_id)
}

/// 是否有 mysql 池在 `run_id` 名下打开(any slot)。供测试使用。
#[doc(hidden)]
pub fn mysql_pool_open(run_id: &str) -> bool {
    MYSQL_POOLS.has_run(run_id)
}

struct PgTeardown;
#[async_trait]
impl RunTeardown for PgTeardown {
    async fn teardown(&self, run_id: &str) {
        close_run_pg_pools(run_id).await;
    }
}

struct MysqlTeardown;
#[async_trait]
impl RunTeardown for MysqlTeardown {
    async fn teardown(&self, run_id: &str) {
        close_run_mysql_pools(run_id).await;
    }
}

/// T3 工厂 `postgres`:DSN 含凭据、来自步骤输入而非 decl,故 `open` 仅校验
/// (`Ok(())`),不连库 —— 镜像 ftp/smtp 工厂。池由首个绑定步骤经 `ensure_pg_pool`
/// 懒建。
struct PgFactory;
#[async_trait]
impl ResourceFactory for PgFactory {
    fn kind(&self) -> &str {
        PG_KIND
    }
    async fn open(&self, _decl: &ResourceDecl, _run_id: &str, _name: &str) -> Result<(), StepError> {
        Ok(())
    }
}

/// T3 工厂 `mysql`:同 `PgFactory` —— `open` 仅校验、不连库。
struct MysqlFactory;
#[async_trait]
impl ResourceFactory for MysqlFactory {
    fn kind(&self) -> &str {
        MYSQL_KIND
    }
    async fn open(&self, _decl: &ResourceDecl, _run_id: &str, _name: &str) -> Result<(), StepError> {
        Ok(())
    }
}

// ─── 行 → JSON 转换(运行期、无宏)────────────────────────────────────────────
//
// sqlx 不直接给「任意列 → serde_json::Value」,只能按列类型逐个 `try_get` 解码。
// 这里覆盖常见类型(bool/整型/浮点/字符串/json/时间族/NUMERIC/UUID),其余类型
// 回退为该列的字符串文本 —— 既不丢行,又不因冷门类型整查询失败。NULL 列 → Null。
// 注意 sqlx 的 `String` 解码有严格类型兼容检查(仅文本族列),时间/数值/UUID 列
// 必须走显式分支,否则会静默变 Null —— 真取不出来时打 warn,绝不无声吞掉。
//
// 时间族输出:TIMESTAMPTZ → RFC3339;无时区的 TIMESTAMP/DATETIME/DATE/TIME →
// chrono 默认文本(`2024-01-02 03:04:05`)。NUMERIC/DECIMAL → 字符串(保精度,
// 金额列不经过 f64)。

/// 解码失败兜底:带列名+类型名打 warn,返回 Null(可见,而非静默)。
fn unsupported_column(driver: &str, name: &str, ty: &str) -> Value {
    tracing::warn!(column = name, r#type = ty, "{driver} column type not decodable; returning null");
    Value::Null
}

/// Postgres 行 → JSON 对象(列名 → JSON 值)。
fn pg_row_to_json(row: &sqlx::postgres::PgRow) -> Value {
    let mut m = Map::new();
    for (i, col) in row.columns().iter().enumerate() {
        let name = col.name().to_string();
        let ty = col.type_info().name().to_string();
        let v = match row.try_get_raw(i) {
            Ok(raw) if raw.is_null() => Value::Null,
            Ok(_) => match ty.as_str() {
                "BOOL" => row.try_get::<bool, _>(i).map(Value::Bool).unwrap_or(Value::Null),
                "INT2" | "SMALLINT" => row
                    .try_get::<i16, _>(i)
                    .map(|n| Value::from(n as i64))
                    .unwrap_or(Value::Null),
                "INT4" | "INT" | "INTEGER" => row
                    .try_get::<i32, _>(i)
                    .map(|n| Value::from(n as i64))
                    .unwrap_or(Value::Null),
                "INT8" | "BIGINT" => row.try_get::<i64, _>(i).map(Value::from).unwrap_or(Value::Null),
                "FLOAT4" | "REAL" => row
                    .try_get::<f32, _>(i)
                    .ok()
                    .and_then(|f| serde_json::Number::from_f64(f as f64).map(Value::Number))
                    .unwrap_or(Value::Null),
                "FLOAT8" | "DOUBLE PRECISION" => row
                    .try_get::<f64, _>(i)
                    .ok()
                    .and_then(|f| serde_json::Number::from_f64(f).map(Value::Number))
                    .unwrap_or(Value::Null),
                "JSON" | "JSONB" => row.try_get::<Value, _>(i).unwrap_or(Value::Null),
                "TIMESTAMPTZ" => row
                    .try_get::<chrono::DateTime<chrono::Utc>, _>(i)
                    .map(|t| Value::String(t.to_rfc3339()))
                    .unwrap_or_else(|_| unsupported_column("postgres", &name, &ty)),
                "TIMESTAMP" => row
                    .try_get::<chrono::NaiveDateTime, _>(i)
                    .map(|t| Value::String(t.to_string()))
                    .unwrap_or_else(|_| unsupported_column("postgres", &name, &ty)),
                "DATE" => row
                    .try_get::<chrono::NaiveDate, _>(i)
                    .map(|t| Value::String(t.to_string()))
                    .unwrap_or_else(|_| unsupported_column("postgres", &name, &ty)),
                "TIME" => row
                    .try_get::<chrono::NaiveTime, _>(i)
                    .map(|t| Value::String(t.to_string()))
                    .unwrap_or_else(|_| unsupported_column("postgres", &name, &ty)),
                "NUMERIC" => row
                    .try_get::<sqlx::types::BigDecimal, _>(i)
                    .map(|d| Value::String(d.to_string()))
                    .unwrap_or_else(|_| unsupported_column("postgres", &name, &ty)),
                "UUID" => row
                    .try_get::<sqlx::types::Uuid, _>(i)
                    .map(|u| Value::String(u.to_string()))
                    .unwrap_or_else(|_| unsupported_column("postgres", &name, &ty)),
                // TEXT/VARCHAR/BPCHAR/NAME… 文本族取字符串;其余类型 warn + Null。
                _ => row
                    .try_get::<String, _>(i)
                    .map(Value::String)
                    .unwrap_or_else(|_| unsupported_column("postgres", &name, &ty)),
            },
            Err(_) => Value::Null,
        };
        m.insert(name, v);
    }
    Value::Object(m)
}

/// MySQL 行 → JSON 对象(列名 → JSON 值)。
fn mysql_row_to_json(row: &sqlx::mysql::MySqlRow) -> Value {
    let mut m = Map::new();
    for (i, col) in row.columns().iter().enumerate() {
        let name = col.name().to_string();
        let ty = col.type_info().name().to_string();
        let v = match row.try_get_raw(i) {
            Ok(raw) if raw.is_null() => Value::Null,
            Ok(_) => match ty.as_str() {
                "BOOLEAN" | "BOOL" => row.try_get::<bool, _>(i).map(Value::Bool).unwrap_or(Value::Null),
                "TINYINT" | "SMALLINT" | "MEDIUMINT" | "INT" | "INTEGER" | "BIGINT" => row
                    .try_get::<i64, _>(i)
                    .map(Value::from)
                    .unwrap_or(Value::Null),
                "TINYINT UNSIGNED" | "SMALLINT UNSIGNED" | "MEDIUMINT UNSIGNED"
                | "INT UNSIGNED" | "BIGINT UNSIGNED" | "YEAR" => row
                    .try_get::<u64, _>(i)
                    .map(Value::from)
                    .unwrap_or(Value::Null),
                "FLOAT" => row
                    .try_get::<f32, _>(i)
                    .ok()
                    .and_then(|f| serde_json::Number::from_f64(f as f64).map(Value::Number))
                    .unwrap_or(Value::Null),
                "DOUBLE" => row
                    .try_get::<f64, _>(i)
                    .ok()
                    .and_then(|f| serde_json::Number::from_f64(f).map(Value::Number))
                    .unwrap_or(Value::Null),
                "JSON" => row.try_get::<Value, _>(i).unwrap_or(Value::Null),
                // MySQL TIMESTAMP 是 UTC 时刻,DATETIME 是无时区挂钟时间。
                "TIMESTAMP" => row
                    .try_get::<chrono::DateTime<chrono::Utc>, _>(i)
                    .map(|t| Value::String(t.to_rfc3339()))
                    .unwrap_or_else(|_| unsupported_column("mysql", &name, &ty)),
                "DATETIME" => row
                    .try_get::<chrono::NaiveDateTime, _>(i)
                    .map(|t| Value::String(t.to_string()))
                    .unwrap_or_else(|_| unsupported_column("mysql", &name, &ty)),
                "DATE" => row
                    .try_get::<chrono::NaiveDate, _>(i)
                    .map(|t| Value::String(t.to_string()))
                    .unwrap_or_else(|_| unsupported_column("mysql", &name, &ty)),
                "TIME" => row
                    .try_get::<chrono::NaiveTime, _>(i)
                    .map(|t| Value::String(t.to_string()))
                    .unwrap_or_else(|_| unsupported_column("mysql", &name, &ty)),
                "DECIMAL" => row
                    .try_get::<sqlx::types::BigDecimal, _>(i)
                    .map(|d| Value::String(d.to_string()))
                    .unwrap_or_else(|_| unsupported_column("mysql", &name, &ty)),
                // VARCHAR/CHAR/TEXT/ENUM… 文本族取字符串;其余类型 warn + Null。
                _ => row
                    .try_get::<String, _>(i)
                    .map(Value::String)
                    .unwrap_or_else(|_| unsupported_column("mysql", &name, &ty)),
            },
            Err(_) => Value::Null,
        };
        m.insert(name, v);
    }
    Value::Object(m)
}

/// 把一个 JSON 参数绑定到 Postgres 查询。覆盖 null/bool/i64/f64/string,
/// 其余(数组/对象)按其 JSON 文本绑定为字符串。
fn bind_pg<'q>(
    q: sqlx::query::Query<'q, sqlx::Postgres, sqlx::postgres::PgArguments>,
    v: &'q Value,
) -> sqlx::query::Query<'q, sqlx::Postgres, sqlx::postgres::PgArguments> {
    match v {
        Value::Null => q.bind(Option::<String>::None),
        Value::Bool(b) => q.bind(*b),
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                q.bind(i)
            } else if let Some(f) = n.as_f64() {
                q.bind(f)
            } else {
                q.bind(n.to_string())
            }
        }
        Value::String(s) => q.bind(s.clone()),
        other => q.bind(other.to_string()),
    }
}

/// 把一个 JSON 参数绑定到 MySQL 查询。语义同 `bind_pg`。
fn bind_mysql<'q>(
    q: sqlx::query::Query<'q, sqlx::MySql, sqlx::mysql::MySqlArguments>,
    v: &'q Value,
) -> sqlx::query::Query<'q, sqlx::MySql, sqlx::mysql::MySqlArguments> {
    match v {
        Value::Null => q.bind(Option::<String>::None),
        Value::Bool(b) => q.bind(*b),
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                q.bind(i)
            } else if let Some(f) = n.as_f64() {
                q.bind(f)
            } else {
                q.bind(n.to_string())
            }
        }
        Value::String(s) => q.bind(s.clone()),
        other => q.bind(other.to_string()),
    }
}

// ─── db.postgres_query / db.postgres_exec ────────────────────────────────────

/// `postgres_*` 的输入:`dsn`(未绑定时必填)+ `sql` + 位置参数 `params`($1,$2…)。
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct PgIn {
    /// 连接串 `postgres://user:pass@host:5432/db`。绑定 `postgres` 资源时由首个
    /// 绑定步骤的 dsn 建池,本字段在后续绑定步骤里被忽略;未绑定则本字段必填。
    #[serde(default)]
    dsn: Option<String>,
    sql: String,
    /// 位置参数,按 `$1,$2,…` 顺序绑定(占位符原样透传,不改写)。
    #[serde(default)]
    params: Vec<Value>,
    /// 语句执行超时(毫秒,默认 60_000)。挂死的查询/锁等待不该无限阻塞整个 flow。
    #[serde(default = "default_db_timeout_ms")]
    timeout_ms: u64,
    /// 仅 query 生效:返回行数上限(默认 1000,对齐 `db.sqlite_query`),超限时
    /// 输出 `truncated: true` 并停止取行 —— 大表不全量进内存。
    #[serde(default = "default_db_limit")]
    limit: u64,
}

fn default_db_timeout_ms() -> u64 {
    60_000
}
fn default_db_limit() -> u64 {
    1000
}

/// 解析 `(slot, dsn)`:绑定步骤复用共享池(dsn 仅首个绑定步骤需要,这里取其自带
/// dsn 仅用于首次建池,可空—— 复用路径不需要);未绑定步骤必须自带 dsn。
fn resolve_pg(ctx: &StepCtx, step_dsn: Option<String>) -> (Option<String>, Option<String>) {
    match pg_slot(ctx) {
        Some(name) => (Some(name), step_dsn),
        None => (None, step_dsn),
    }
}

fn resolve_mysql(ctx: &StepCtx, step_dsn: Option<String>) -> (Option<String>, Option<String>) {
    match mysql_slot(ctx) {
        Some(name) => (Some(name), step_dsn),
        None => (None, step_dsn),
    }
}

/// 取本步骤可用的 PgPool:绑定步骤复用 `(run_id, slot)` 共享池(没有则用本步
/// dsn 懒建);未绑定步骤建 `max_connections=1` 的单次池并返回 `ephemeral=true`
/// —— 调用方用毕必须 `pool.close().await`:sqlx Pool 裸 drop 不做协议级断开
/// (服务端记 unexpected EOF),且单次路径没必要留惰性回收的后台连接。
async fn pg_pool_for(
    ctx: &StepCtx,
    action: &str,
    step_dsn: Option<String>,
) -> Result<(Arc<sqlx::PgPool>, bool), StepError> {
    let (slot, step_dsn) = resolve_pg(ctx, step_dsn);
    match slot {
        Some(name) => {
            if let Some(p) = PG_POOLS.get(ctx.run_id(), &name) {
                return Ok((p, false));
            }
            let dsn = step_dsn
                .ok_or_else(|| StepError::msg(format!("{action}: first bound step needs `dsn`")))?;
            ctx.ensure_network_url(&dsn)?;
            Ok((ensure_pg_pool(ctx.run_id(), &name, &dsn).await?, false))
        }
        None => {
            let dsn = step_dsn.ok_or_else(|| {
                StepError::msg(format!("{action}: `dsn` is required (or bind a `postgres` resource)"))
            })?;
            ctx.ensure_network_url(&dsn)?;
            let pool = sqlx::postgres::PgPoolOptions::new()
                .max_connections(1)
                .connect(&dsn)
                .await
                .map_err(|_| StepError::msg("postgres connect failed"))?;
            Ok((Arc::new(pool), true))
        }
    }
}

/// MySQL 版 `pg_pool_for`,语义完全一致。
async fn mysql_pool_for(
    ctx: &StepCtx,
    action: &str,
    step_dsn: Option<String>,
) -> Result<(Arc<sqlx::MySqlPool>, bool), StepError> {
    let (slot, step_dsn) = resolve_mysql(ctx, step_dsn);
    match slot {
        Some(name) => {
            if let Some(p) = MYSQL_POOLS.get(ctx.run_id(), &name) {
                return Ok((p, false));
            }
            let dsn = step_dsn
                .ok_or_else(|| StepError::msg(format!("{action}: first bound step needs `dsn`")))?;
            ctx.ensure_network_url(&dsn)?;
            Ok((ensure_mysql_pool(ctx.run_id(), &name, &dsn).await?, false))
        }
        None => {
            let dsn = step_dsn.ok_or_else(|| {
                StepError::msg(format!("{action}: `dsn` is required (or bind a `mysql` resource)"))
            })?;
            ctx.ensure_network_url(&dsn)?;
            let pool = sqlx::mysql::MySqlPoolOptions::new()
                .max_connections(1)
                .connect(&dsn)
                .await
                .map_err(|_| StepError::msg("mysql connect failed"))?;
            Ok((Arc::new(pool), true))
        }
    }
}

pub struct PgQueryAction;
#[async_trait]
impl Action for PgQueryAction {
    fn id(&self) -> &'static str {
        "db.postgres_query"
    }
    fn summary(&self) -> &'static str {
        "Run a SELECT against PostgreSQL; rows returned as a JSON array"
    }
    fn schema(&self) -> &'static Value {
        static S: Lazy<Value> = Lazy::new(crate::schema::derive::<PgIn>);
        &S
    }
    async fn execute(&self, ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let PgIn { dsn, sql, params, timeout_ms, limit } = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("db.postgres_query invalid: {e}")))?;
        let (pool, ephemeral) = pg_pool_for(ctx, "db.postgres_query", dsn).await?;
        // 流式取行:到 limit 即停,大结果集不全量进内存;整段套 timeout。
        let fetched = tokio::time::timeout(std::time::Duration::from_millis(timeout_ms), async {
            use futures::TryStreamExt;
            let mut q = sqlx::query(&sql);
            for p in &params {
                q = bind_pg(q, p);
            }
            let mut stream = q.fetch(pool.as_ref());
            let mut rows: Vec<Value> = Vec::new();
            let mut truncated = false;
            while let Some(row) = stream.try_next().await? {
                if rows.len() as u64 >= limit {
                    truncated = true;
                    break;
                }
                rows.push(pg_row_to_json(&row));
            }
            Ok::<_, sqlx::Error>((rows, truncated))
        })
        .await;
        if ephemeral {
            pool.close().await;
        }
        let (rows, truncated) = match fetched {
            Err(_) => {
                return Err(StepError::msg(format!(
                    "db.postgres_query: timed out after {timeout_ms} ms"
                )))
            }
            Ok(r) => r.map_err(|e| StepError::msg(format!("postgres query: {e}")))?,
        };
        Ok(ActionResult::from(serde_json::json!({
            "rows": rows,
            "count": rows.len(),
            "truncated": truncated,
        })))
    }
}

pub struct PgExecAction;
#[async_trait]
impl Action for PgExecAction {
    fn id(&self) -> &'static str {
        "db.postgres_exec"
    }
    fn summary(&self) -> &'static str {
        "Run an INSERT/UPDATE/DELETE/DDL against PostgreSQL"
    }
    fn schema(&self) -> &'static Value {
        static S: Lazy<Value> = Lazy::new(crate::schema::derive::<PgIn>);
        &S
    }
    async fn execute(&self, ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let PgIn { dsn, sql, params, timeout_ms, limit: _ } = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("db.postgres_exec invalid: {e}")))?;
        let (pool, ephemeral) = pg_pool_for(ctx, "db.postgres_exec", dsn).await?;
        let res = tokio::time::timeout(std::time::Duration::from_millis(timeout_ms), async {
            let mut q = sqlx::query(&sql);
            for p in &params {
                q = bind_pg(q, p);
            }
            q.execute(pool.as_ref()).await
        })
        .await;
        if ephemeral {
            pool.close().await;
        }
        let res = match res {
            Err(_) => {
                return Err(StepError::msg(format!(
                    "db.postgres_exec: timed out after {timeout_ms} ms"
                )))
            }
            Ok(r) => r.map_err(|e| StepError::msg(format!("postgres exec: {e}")))?,
        };
        Ok(ActionResult::from(serde_json::json!({
            "rows_affected": res.rows_affected(),
        })))
    }
}

// ─── db.mysql_query / db.mysql_exec ──────────────────────────────────────────

/// `mysql_*` 的输入:`dsn` + `sql` + 位置参数 `params`(`?` 占位符)。
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct MysqlIn {
    /// 连接串 `mysql://user:pass@host:3306/db`。绑定语义同 `postgres`。
    #[serde(default)]
    dsn: Option<String>,
    sql: String,
    /// 位置参数,按 `?` 顺序绑定(占位符原样透传,不改写)。
    #[serde(default)]
    params: Vec<Value>,
    /// 语句执行超时(毫秒,默认 60_000)。
    #[serde(default = "default_db_timeout_ms")]
    timeout_ms: u64,
    /// 仅 query 生效:返回行数上限(默认 1000),超限时输出 `truncated: true`。
    #[serde(default = "default_db_limit")]
    limit: u64,
}

pub struct MysqlQueryAction;
#[async_trait]
impl Action for MysqlQueryAction {
    fn id(&self) -> &'static str {
        "db.mysql_query"
    }
    fn summary(&self) -> &'static str {
        "Run a SELECT against MySQL; rows returned as a JSON array"
    }
    fn schema(&self) -> &'static Value {
        static S: Lazy<Value> = Lazy::new(crate::schema::derive::<MysqlIn>);
        &S
    }
    async fn execute(&self, ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let MysqlIn { dsn, sql, params, timeout_ms, limit } = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("db.mysql_query invalid: {e}")))?;
        let (pool, ephemeral) = mysql_pool_for(ctx, "db.mysql_query", dsn).await?;
        let fetched = tokio::time::timeout(std::time::Duration::from_millis(timeout_ms), async {
            use futures::TryStreamExt;
            let mut q = sqlx::query(&sql);
            for p in &params {
                q = bind_mysql(q, p);
            }
            let mut stream = q.fetch(pool.as_ref());
            let mut rows: Vec<Value> = Vec::new();
            let mut truncated = false;
            while let Some(row) = stream.try_next().await? {
                if rows.len() as u64 >= limit {
                    truncated = true;
                    break;
                }
                rows.push(mysql_row_to_json(&row));
            }
            Ok::<_, sqlx::Error>((rows, truncated))
        })
        .await;
        if ephemeral {
            pool.close().await;
        }
        let (rows, truncated) = match fetched {
            Err(_) => {
                return Err(StepError::msg(format!(
                    "db.mysql_query: timed out after {timeout_ms} ms"
                )))
            }
            Ok(r) => r.map_err(|e| StepError::msg(format!("mysql query: {e}")))?,
        };
        Ok(ActionResult::from(serde_json::json!({
            "rows": rows,
            "count": rows.len(),
            "truncated": truncated,
        })))
    }
}

pub struct MysqlExecAction;
#[async_trait]
impl Action for MysqlExecAction {
    fn id(&self) -> &'static str {
        "db.mysql_exec"
    }
    fn summary(&self) -> &'static str {
        "Run an INSERT/UPDATE/DELETE/DDL against MySQL"
    }
    fn schema(&self) -> &'static Value {
        static S: Lazy<Value> = Lazy::new(crate::schema::derive::<MysqlIn>);
        &S
    }
    async fn execute(&self, ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let MysqlIn { dsn, sql, params, timeout_ms, limit: _ } = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("db.mysql_exec invalid: {e}")))?;
        let (pool, ephemeral) = mysql_pool_for(ctx, "db.mysql_exec", dsn).await?;
        let res = tokio::time::timeout(std::time::Duration::from_millis(timeout_ms), async {
            let mut q = sqlx::query(&sql);
            for p in &params {
                q = bind_mysql(q, p);
            }
            q.execute(pool.as_ref()).await
        })
        .await;
        if ephemeral {
            pool.close().await;
        }
        let res = match res {
            Err(_) => {
                return Err(StepError::msg(format!(
                    "db.mysql_exec: timed out after {timeout_ms} ms"
                )))
            }
            Ok(r) => r.map_err(|e| StepError::msg(format!("mysql exec: {e}")))?,
        };
        Ok(ActionResult::from(serde_json::json!({
            "rows_affected": res.rows_affected(),
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

    // ─── PostgreSQL / MySQL T3 单测 ──────────────────────────────────────────

    /// 带 network 授权的 ctx,用于验证 DSN→host 提取 + 能力门禁。
    fn ctx_with_network(
        resources: &[(&str, &str)],
        current: Option<&str>,
        network: &[&str],
    ) -> StepCtx {
        let map: BTreeMap<String, ResourceDecl> = resources
            .iter()
            .map(|(name, yaml)| (name.to_string(), decl(yaml)))
            .collect();
        let caps = lumo_dsl::Capabilities {
            network: network.iter().map(|s| s.to_string()).collect(),
            ..Default::default()
        };
        let ctx = StepCtx::new(
            "run-db".into(),
            "flow-db".into(),
            ActionRegistry::new(),
            None,
            Value::Null,
            caps,
            Vec::new(),
        )
        .with_resources(map);
        ctx.set_current_resource(current);
        ctx
    }

    #[test]
    fn pg_and_mysql_factory_kinds_match() {
        assert_eq!(PgFactory.kind(), PG_KIND);
        assert_eq!(PG_KIND, "postgres");
        assert_eq!(MysqlFactory.kind(), MYSQL_KIND);
        assert_eq!(MYSQL_KIND, "mysql");
    }

    #[test]
    fn pg_slot_selects_only_postgres_kind_bindings() {
        let resources = &[
            ("pg", "kind: postgres\n"),
            ("my", "kind: mysql\n"),
            ("lite", "kind: sqlite\npath: /tmp/x.db\n"),
        ];
        // Bound to a postgres resource ⇒ its name is the slot (shared pool).
        assert_eq!(
            pg_slot(&ctx_with(resources, Some("pg"))).as_deref(),
            Some("pg")
        );
        // Unbound / wrong-kind / undeclared ⇒ None ⇒ per-call connect (back-compat).
        assert_eq!(pg_slot(&ctx_with(resources, None)), None);
        assert_eq!(pg_slot(&ctx_with(resources, Some("my"))), None);
        assert_eq!(pg_slot(&ctx_with(resources, Some("lite"))), None);
        assert_eq!(pg_slot(&ctx_with(resources, Some("ghost"))), None);
    }

    #[test]
    fn mysql_slot_selects_only_mysql_kind_bindings() {
        let resources = &[("pg", "kind: postgres\n"), ("my", "kind: mysql\n")];
        assert_eq!(
            mysql_slot(&ctx_with(resources, Some("my"))).as_deref(),
            Some("my")
        );
        assert_eq!(mysql_slot(&ctx_with(resources, None)), None);
        assert_eq!(mysql_slot(&ctx_with(resources, Some("pg"))), None);
        assert_eq!(mysql_slot(&ctx_with(resources, Some("ghost"))), None);
    }

    #[test]
    fn resolve_pg_and_mysql_carry_slot_and_dsn() {
        let resources = &[("pg", "kind: postgres\n"), ("my", "kind: mysql\n")];
        // Bound ⇒ Some(name); the step's own dsn is carried for first-bound pool build.
        let (slot, dsn) = resolve_pg(
            &ctx_with(resources, Some("pg")),
            Some("postgres://u:p@db.internal:5432/app".into()),
        );
        assert_eq!(slot.as_deref(), Some("pg"));
        assert_eq!(dsn.as_deref(), Some("postgres://u:p@db.internal:5432/app"));
        // Unbound ⇒ None slot, dsn from the step.
        let (slot, dsn) = resolve_mysql(
            &ctx_with(resources, None),
            Some("mysql://u:p@db.internal:3306/app".into()),
        );
        assert!(slot.is_none());
        assert_eq!(dsn.as_deref(), Some("mysql://u:p@db.internal:3306/app"));
    }

    #[test]
    fn dsn_host_is_gated_by_network_capability() {
        // The same `ensure_network_url(&dsn)` gate the action runs before connecting:
        // a DSN whose host is granted passes; an ungranted host is rejected — proving
        // host extraction from a `postgres://`/`mysql://` URL and capability denial.
        let resources = &[("pg", "kind: postgres\n")];
        let ctx = ctx_with_network(resources, Some("pg"), &["db.allowed.internal"]);
        // Granted host ⇒ Ok (port + creds + db path stripped by extract_host).
        ctx.ensure_network_url("postgres://user:secret@db.allowed.internal:5432/app")
            .expect("granted host must pass the network gate");
        ctx.ensure_network_url("mysql://user:secret@db.allowed.internal:3306/app")
            .expect("granted host must pass the network gate");
        // Ungranted host ⇒ capability denied (never reaches a connect attempt).
        let err = ctx
            .ensure_network_url("postgres://user:secret@evil.example.com:5432/app")
            .expect_err("an ungranted host must be rejected");
        assert!(
            matches!(err, StepError::CapabilityDenied { .. }),
            "got: {err}"
        );
    }

    #[test]
    fn pg_param_binding_maps_json_scalars() {
        // null/bool/i64/f64/string must each bind without panicking; the bound query
        // is consumed by the runtime API (no server needed to exercise the mapping).
        let params = vec![
            Value::Null,
            Value::Bool(true),
            Value::from(42i64),
            Value::from(1.5f64),
            Value::String("hi".into()),
            serde_json::json!(["arr"]), // falls through to JSON-text bind
        ];
        let mut q = sqlx::query("SELECT $1, $2, $3, $4, $5, $6");
        for p in &params {
            q = bind_pg(q, p);
        }
        // Reaching here (and dropping `q`) means every variant bound cleanly.
        drop(q);

        let mut mq = sqlx::query("SELECT ?, ?, ?, ?, ?, ?");
        for p in &params {
            mq = bind_mysql(mq, p);
        }
        drop(mq);
    }

    #[tokio::test]
    async fn pg_and_mysql_teardown_idempotent_for_a_run_that_opened_nothing() {
        // End-of-run teardown is idempotent: with no pools opened, `take_run` yields
        // nothing so the close loop runs zero times. This is the path a run cancelled
        // before any pg/mysql open — or a double-teardown — takes.
        assert!(!pg_pool_open("ghost-run"));
        assert!(!mysql_pool_open("ghost-run"));
        close_run_pg_pools("ghost-run").await; // clean no-op
        close_run_pg_pools("ghost-run").await; // idempotent
        close_run_mysql_pools("ghost-run").await;
        close_run_mysql_pools("ghost-run").await;
        assert!(!pg_pool_open("ghost-run"));
        assert!(!mysql_pool_open("ghost-run"));
    }
}
