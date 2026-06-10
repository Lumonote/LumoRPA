//! `excel.lookup` 集成测试:vlookup/xlookup 语义。
//! 覆盖:命中 / 未命中+default / 未命中报错 / all_matches / 表头名定位 /
//! 列字母+序号定位 / 数字键 vs 文本键(Excel 数字 1 与文本 "1" 不互通)。

mod common;
use common::{fs_caps, ok_with, run_with, Capabilities};
use serde_json::json;
use std::path::Path;

/// 用 `excel.write_range` 造一张标准查找表(表头在第 1 行)。
async fn make_book(dir: &Path) -> (std::path::PathBuf, Capabilities) {
    let file = dir.join("lookup.xlsx");
    let caps = fs_caps(dir);
    ok_with(
        "excel.write_range",
        json!({
            "file": file,
            "sheet": "S",
            "cell": "A1",
            "values": [
                ["id", "name", "score"],
                [1, "alice", 90],
                [2, "bob", 80],
                [2, "carol", 70]
            ]
        }),
        caps.clone(),
    )
    .await;
    (file, caps)
}

#[tokio::test]
async fn finds_first_match_by_header_name() {
    let dir = tempfile::tempdir().unwrap();
    let (file, caps) = make_book(dir.path()).await;

    let out = ok_with(
        "excel.lookup",
        json!({
            "file": file,
            "sheet": "S",
            "key_column": "id",
            "key_value": 1,
            "value_column": "name"
        }),
        caps,
    )
    .await;
    assert_eq!(
        out,
        json!({"found": true, "value": "alice", "row_number": 2})
    );
}

#[tokio::test]
async fn miss_with_default_returns_the_default_without_error() {
    let dir = tempfile::tempdir().unwrap();
    let (file, caps) = make_book(dir.path()).await;

    let out = ok_with(
        "excel.lookup",
        json!({
            "file": file,
            "key_column": "id",
            "key_value": 99,
            "value_column": "name",
            "default": "N/A"
        }),
        caps,
    )
    .await;
    assert_eq!(out, json!({"found": false, "value": "N/A"}));
}

#[tokio::test]
async fn miss_without_default_errors() {
    let dir = tempfile::tempdir().unwrap();
    let (file, caps) = make_book(dir.path()).await;

    let err = run_with(
        "excel.lookup",
        json!({
            "file": file,
            "key_column": "id",
            "key_value": 99,
            "value_column": "name"
        }),
        caps,
    )
    .await
    .unwrap_err();
    assert!(err.contains("not found"), "got: {err}");
}

#[tokio::test]
async fn all_matches_returns_every_hit_and_empty_on_miss() {
    let dir = tempfile::tempdir().unwrap();
    let (file, caps) = make_book(dir.path()).await;

    let out = ok_with(
        "excel.lookup",
        json!({
            "file": file,
            "key_column": "id",
            "key_value": 2,
            "value_column": "name",
            "all_matches": true
        }),
        caps.clone(),
    )
    .await;
    assert_eq!(
        out,
        json!({"found": true, "values": ["bob", "carol"], "row_numbers": [3, 4]})
    );

    // all_matches 未命中:空数组即合法答案,不报错。
    let empty = ok_with(
        "excel.lookup",
        json!({
            "file": file,
            "key_column": "id",
            "key_value": 99,
            "value_column": "name",
            "all_matches": true
        }),
        caps,
    )
    .await;
    assert_eq!(
        empty,
        json!({"found": false, "values": [], "row_numbers": []})
    );
}

#[tokio::test]
async fn resolves_columns_by_letter_and_one_based_index() {
    let dir = tempfile::tempdir().unwrap();
    let (file, caps) = make_book(dir.path()).await;

    // key_column 用列字母 "A",value_column 用 1-based 序号 3(score 列)。
    let out = ok_with(
        "excel.lookup",
        json!({
            "file": file,
            "key_column": "A",
            "key_value": 2,
            "value_column": 3
        }),
        caps,
    )
    .await;
    assert_eq!(out, json!({"found": true, "value": 80.0, "row_number": 3}));
}

#[tokio::test]
async fn numeric_key_and_text_key_do_not_cross_match() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("typed.xlsx");
    let caps = fs_caps(dir.path());
    // 同一列里既有文本 "1" 又有数字 1 —— 同类型精确匹配,互不串。
    ok_with(
        "excel.write_range",
        json!({
            "file": file,
            "sheet": "S",
            "cell": "A1",
            "values": [["k", "v"], ["1", "text-one"], [1, "num-one"]]
        }),
        caps.clone(),
    )
    .await;

    let by_number = ok_with(
        "excel.lookup",
        json!({"file": file, "key_column": "k", "key_value": 1, "value_column": "v"}),
        caps.clone(),
    )
    .await;
    assert_eq!(by_number["value"], json!("num-one"));

    let by_text = ok_with(
        "excel.lookup",
        json!({"file": file, "key_column": "k", "key_value": "1", "value_column": "v"}),
        caps,
    )
    .await;
    assert_eq!(by_text["value"], json!("text-one"));
}

#[tokio::test]
async fn unknown_header_name_errors_clearly() {
    let dir = tempfile::tempdir().unwrap();
    let (file, caps) = make_book(dir.path()).await;

    let err = run_with(
        "excel.lookup",
        json!({
            "file": file,
            "key_column": "no_such_header",
            "key_value": 1,
            "value_column": "name"
        }),
        caps,
    )
    .await
    .unwrap_err();
    assert!(
        err.contains("no_such_header") && err.contains("header row"),
        "got: {err}"
    );
}

#[tokio::test]
async fn lookup_denied_without_fs_grant() {
    let err = run_with(
        "excel.lookup",
        json!({
            "file": "/etc/whatever.xlsx",
            "key_column": "A",
            "key_value": 1,
            "value_column": "B"
        }),
        Capabilities::default(),
    )
    .await
    .unwrap_err();
    assert!(err.contains("capability denied"), "got: {err}");
}
