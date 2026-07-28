//! HIGH-1 回归:`pg_row_to_json` / `mysql_row_to_json` 的列类型覆盖。
//!
//! 这些断言需要真实数据库,默认跳过;在有库的环境里设置
//! `LUMO_PG_TEST_DSN` / `LUMO_MYSQL_TEST_DSN` 即可启用,例如:
//! `LUMO_PG_TEST_DSN=postgres://user:pass@127.0.0.1:5432/postgres cargo test -p lumo-actions --test db_remote`
//!
//! 回归点:TIMESTAMP/TIMESTAMPTZ/DATE/TIME/NUMERIC(DECIMAL)/UUID 列曾因
//! 走 `try_get::<String>` 类型不兼容被 `unwrap_or(Null)` 吞掉,静默变 null。

mod common;
use common::{run_with, Capabilities};
use serde_json::{json, Value};

fn net_all() -> Capabilities {
    Capabilities {
        network: vec!["*".to_string()],
        ..Default::default()
    }
}

/// 断言 `rows[0][col]` 是非空字符串且包含 `needle`。
fn assert_str_col(row: &Value, col: &str, needle: &str) {
    let v = row
        .get(col)
        .unwrap_or_else(|| panic!("column `{col}` missing from row: {row}"));
    let s = v.as_str().unwrap_or_else(|| {
        panic!("column `{col}` should be a string, got: {v} (silent-null regression?)")
    });
    assert!(
        s.contains(needle),
        "column `{col}` = {s:?}, want substring {needle:?}"
    );
}

#[tokio::test]
async fn pg_typed_columns_do_not_silently_null() {
    let Ok(dsn) = std::env::var("LUMO_PG_TEST_DSN") else {
        eprintln!("LUMO_PG_TEST_DSN not set; skipping live postgres type-coverage test");
        return;
    };
    let out = run_with(
        "db.postgres_query",
        json!({
            "dsn": dsn,
            "sql": "SELECT \
                TIMESTAMP '2024-01-02 03:04:05' AS ts, \
                TIMESTAMPTZ '2024-01-02 03:04:05+00' AS tstz, \
                DATE '2024-01-02' AS d, \
                TIME '03:04:05' AS t, \
                NUMERIC '12345.6789' AS amount, \
                '550e8400-e29b-41d4-a716-446655440000'::uuid AS id, \
                NULL::timestamp AS ts_null"
        }),
        net_all(),
    )
    .await
    .expect("postgres query should succeed");
    let row = &out["rows"][0];
    assert_str_col(row, "ts", "2024-01-02");
    assert_str_col(row, "tstz", "2024-01-02");
    assert_str_col(row, "d", "2024-01-02");
    assert_str_col(row, "t", "03:04:05");
    // NUMERIC 走字符串保精度(金额不过 f64)。
    assert_str_col(row, "amount", "12345.6789");
    assert_str_col(row, "id", "550e8400");
    assert_eq!(row["ts_null"], Value::Null, "real NULL stays null");
    assert_eq!(out["truncated"], json!(false));
}

#[tokio::test]
async fn mysql_typed_columns_do_not_silently_null() {
    let Ok(dsn) = std::env::var("LUMO_MYSQL_TEST_DSN") else {
        eprintln!("LUMO_MYSQL_TEST_DSN not set; skipping live mysql type-coverage test");
        return;
    };
    let out = run_with(
        "db.mysql_query",
        json!({
            "dsn": dsn,
            "sql": "SELECT \
                TIMESTAMP('2024-01-02 03:04:05') AS dt, \
                DATE('2024-01-02') AS d, \
                CAST('12345.6789' AS DECIMAL(20,4)) AS amount, \
                CAST(NULL AS DATETIME) AS dt_null"
        }),
        net_all(),
    )
    .await
    .expect("mysql query should succeed");
    let row = &out["rows"][0];
    assert_str_col(row, "dt", "2024-01-02");
    assert_str_col(row, "d", "2024-01-02");
    assert_str_col(row, "amount", "12345.6789");
    assert_eq!(row["dt_null"], Value::Null, "real NULL stays null");
}

// ─── db.postgres_batch / db.mysql_batch:显式事务的提交 / 回滚 / 超时不落地 ───
// 指令集 P0:全部成功才 commit;坏语句整批回滚;超时把事务 future drop 掉后,
// sqlx 连接回池/断开由服务端 ROLLBACK —— 「引擎说没写,库里也确实没写」。

#[tokio::test]
async fn pg_batch_commits_atomically_and_timeout_rolls_back() {
    let Ok(dsn) = std::env::var("LUMO_PG_TEST_DSN") else {
        eprintln!("LUMO_PG_TEST_DSN not set; skipping live postgres batch tx test");
        return;
    };
    let t = "lumo_batch_tx_test_pg";
    for sql in [
        format!("DROP TABLE IF EXISTS {t}"),
        format!("CREATE TABLE {t} (id INT)"),
    ] {
        run_with(
            "db.postgres_exec",
            json!({"dsn": &dsn, "sql": sql}),
            net_all(),
        )
        .await
        .expect("setup table");
    }

    // 全部成功 ⇒ commit:逐条 rows_affected + 总计。
    let out = run_with(
        "db.postgres_batch",
        json!({"dsn": &dsn, "statements": [
            {"sql": format!("INSERT INTO {t} VALUES ($1)"), "params": [1]},
            {"sql": format!("INSERT INTO {t} VALUES ($1), ($2)"), "params": [2, 3]},
        ]}),
        net_all(),
    )
    .await
    .expect("all-good batch should commit");
    assert_eq!(out["rows_affected"], json!([1, 2]));
    assert_eq!(out["total_affected"], json!(3));
    assert_eq!(out["statements"], json!(2));

    // 中途失败 ⇒ 整批回滚:坏 SQL 之前的 INSERT 也不落地。
    let err = run_with(
        "db.postgres_batch",
        json!({"dsn": &dsn, "statements": [
            {"sql": format!("INSERT INTO {t} VALUES ($1)"), "params": [4]},
            {"sql": "THIS IS NOT SQL"},
        ]}),
        net_all(),
    )
    .await
    .expect_err("a bad statement must fail the whole batch");
    assert!(err.contains("statement 1"), "got: {err}");

    // 超时中断 ⇒ 事务被 drop ⇒ 数据未落地(P0 的核心回归点)。
    let err = run_with(
        "db.postgres_batch",
        json!({"dsn": &dsn, "timeout_ms": 300, "statements": [
            {"sql": format!("INSERT INTO {t} VALUES ($1)"), "params": [5]},
            {"sql": "SELECT pg_sleep(10)"},
        ]}),
        net_all(),
    )
    .await
    .expect_err("pg_sleep must trip the batch timeout");
    assert!(err.contains("timed out"), "got: {err}");

    // 只有第一批的 3 行在库里 —— 失败批与超时批的 INSERT 全部回滚。
    let out = run_with(
        "db.postgres_query",
        json!({"dsn": &dsn, "sql": format!("SELECT COUNT(*) AS n FROM {t}")}),
        net_all(),
    )
    .await
    .expect("count rows");
    assert_eq!(
        out["rows"][0]["n"],
        json!(3),
        "uncommitted batch rows must not land"
    );

    run_with(
        "db.postgres_exec",
        json!({"dsn": &dsn, "sql": format!("DROP TABLE {t}")}),
        net_all(),
    )
    .await
    .expect("cleanup table");
}

#[tokio::test]
async fn mysql_batch_commits_atomically_and_timeout_rolls_back() {
    let Ok(dsn) = std::env::var("LUMO_MYSQL_TEST_DSN") else {
        eprintln!("LUMO_MYSQL_TEST_DSN not set; skipping live mysql batch tx test");
        return;
    };
    let t = "lumo_batch_tx_test_mysql";
    for sql in [
        format!("DROP TABLE IF EXISTS {t}"),
        // InnoDB 显式声明:回滚语义依赖事务引擎(MyISAM 会静默不回滚)。
        format!("CREATE TABLE {t} (id INT) ENGINE=InnoDB"),
    ] {
        run_with("db.mysql_exec", json!({"dsn": &dsn, "sql": sql}), net_all())
            .await
            .expect("setup table");
    }

    // 全部成功 ⇒ commit。
    let out = run_with(
        "db.mysql_batch",
        json!({"dsn": &dsn, "statements": [
            {"sql": format!("INSERT INTO {t} VALUES (?)"), "params": [1]},
            {"sql": format!("INSERT INTO {t} VALUES (?), (?)"), "params": [2, 3]},
        ]}),
        net_all(),
    )
    .await
    .expect("all-good batch should commit");
    assert_eq!(out["rows_affected"], json!([1, 2]));
    assert_eq!(out["total_affected"], json!(3));

    // 中途失败 ⇒ 整批回滚。
    let err = run_with(
        "db.mysql_batch",
        json!({"dsn": &dsn, "statements": [
            {"sql": format!("INSERT INTO {t} VALUES (?)"), "params": [4]},
            {"sql": "THIS IS NOT SQL"},
        ]}),
        net_all(),
    )
    .await
    .expect_err("a bad statement must fail the whole batch");
    assert!(err.contains("statement 1"), "got: {err}");

    // 超时中断 ⇒ 事务被 drop ⇒ 数据未落地。
    let err = run_with(
        "db.mysql_batch",
        json!({"dsn": &dsn, "timeout_ms": 300, "statements": [
            {"sql": format!("INSERT INTO {t} VALUES (?)"), "params": [5]},
            {"sql": "SELECT SLEEP(10)"},
        ]}),
        net_all(),
    )
    .await
    .expect_err("SLEEP must trip the batch timeout");
    assert!(err.contains("timed out"), "got: {err}");

    let out = run_with(
        "db.mysql_query",
        json!({"dsn": &dsn, "sql": format!("SELECT COUNT(*) AS n FROM {t}")}),
        net_all(),
    )
    .await
    .expect("count rows");
    assert_eq!(
        out["rows"][0]["n"],
        json!(3),
        "uncommitted batch rows must not land"
    );

    run_with(
        "db.mysql_exec",
        json!({"dsn": &dsn, "sql": format!("DROP TABLE {t}")}),
        net_all(),
    )
    .await
    .expect("cleanup table");
}
