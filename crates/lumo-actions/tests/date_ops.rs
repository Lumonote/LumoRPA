//! Integration coverage for the `date.*` action family (P1-8).
//! `date.now` is wall-clock, so it's exercised via a parse round-trip rather
//! than an exact value; every other op is deterministic.

mod common;
use common::{ok, run};
use serde_json::json;

#[tokio::test]
async fn now_is_a_parseable_timestamp() {
    let now = ok("date.now", json!({})).await;
    let s = now.as_str().expect("date.now is a string");
    assert!(s.contains('T'), "RFC3339 has a date/time separator: {s}");
    // Feed it straight back through date.parse to prove it's well-formed.
    let reparsed = ok("date.parse", json!({"value": s})).await;
    assert!(reparsed.as_str().unwrap().contains('T'));
}

#[tokio::test]
async fn now_honors_a_custom_strftime_format() {
    let year = ok("date.now", json!({"format": "%Y"})).await;
    let n: i64 = year.as_str().unwrap().parse().expect("year is numeric");
    assert!(n >= 2026, "current year is at least 2026: {n}");
}

#[tokio::test]
async fn parse_normalizes_several_shapes_to_rfc3339() {
    assert_eq!(
        ok("date.parse", json!({"value": "2026-05-29"})).await,
        json!("2026-05-29T00:00:00+00:00")
    );
    assert_eq!(
        ok("date.parse", json!({"value": "2026-05-29 12:30:45"})).await,
        json!("2026-05-29T12:30:45+00:00")
    );
}

#[tokio::test]
async fn parse_rejects_unrecognized_input() {
    let err = run("date.parse", json!({"value": "not a date"}))
        .await
        .unwrap_err();
    assert!(err.contains("cannot parse"), "got: {err}");
}

#[tokio::test]
async fn format_applies_strftime() {
    assert_eq!(
        ok(
            "date.format",
            json!({"value": "2026-05-29T12:30:00Z", "format": "%Y/%m/%d %H:%M"})
        )
        .await,
        json!("2026/05/29 12:30")
    );
}

#[tokio::test]
async fn add_offsets_in_both_directions() {
    assert_eq!(
        ok(
            "date.add",
            json!({"value": "2026-05-29T00:00:00Z", "days": 1, "hours": 2})
        )
        .await,
        json!("2026-05-30T02:00:00+00:00")
    );
    assert_eq!(
        ok(
            "date.add",
            json!({"value": "2026-05-29T00:00:00Z", "days": -1})
        )
        .await,
        json!("2026-05-28T00:00:00+00:00"),
        "negative offsets go backwards"
    );
}

#[tokio::test]
async fn diff_respects_the_unit() {
    let args = json!({"a": "2026-05-29T01:00:00Z", "b": "2026-05-29T00:00:00Z"});
    assert_eq!(
        ok("date.diff", args.clone()).await,
        json!(3600.0),
        "default unit is seconds"
    );
    let mut minutes = args.clone();
    minutes["unit"] = json!("minutes");
    assert_eq!(ok("date.diff", minutes).await, json!(60.0));
}

#[tokio::test]
async fn weekday_is_one_indexed_from_monday() {
    // 2024-01-01 was a Monday; 2024-01-06 Saturday; 2024-01-07 Sunday.
    assert_eq!(
        ok("date.weekday", json!({"value": "2024-01-01"})).await,
        json!(1)
    );
    assert_eq!(
        ok("date.weekday", json!({"value": "2024-01-06"})).await,
        json!(6)
    );
    assert_eq!(
        ok("date.weekday", json!({"value": "2024-01-07"})).await,
        json!(7)
    );
}
