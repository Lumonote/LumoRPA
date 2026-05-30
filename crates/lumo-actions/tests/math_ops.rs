//! Integration coverage for the `math.*` action family (P1-8).

mod common;
use common::{ok, run};
use serde_json::json;

fn f(v: &serde_json::Value) -> f64 {
    v.as_f64()
        .unwrap_or_else(|| panic!("expected a number, got {v}"))
}

#[tokio::test]
async fn round_to_digits() {
    assert!(
        (f(&ok("math.round", json!({"value": 1.23456, "digits": 2})).await) - 1.23).abs() < 1e-9
    );
    assert!((f(&ok("math.round", json!({"value": 2.567, "digits": 1})).await) - 2.6).abs() < 1e-9);
    // Default digits = 0, half rounds away from zero.
    assert!((f(&ok("math.round", json!({"value": 1.5})).await) - 2.0).abs() < 1e-9);
}

#[tokio::test]
async fn abs_value() {
    assert!((f(&ok("math.abs", json!({"value": -5.5})).await) - 5.5).abs() < 1e-9);
    assert!((f(&ok("math.abs", json!({"value": 7})).await) - 7.0).abs() < 1e-9);
}

#[tokio::test]
async fn min_max_sum_avg_ignore_non_numbers() {
    let items = json!({"items": [3, "x", 1, 2, null]});
    assert!((f(&ok("math.min", items.clone()).await) - 1.0).abs() < 1e-9);
    assert!((f(&ok("math.max", items.clone()).await) - 3.0).abs() < 1e-9);
    assert!((f(&ok("math.sum", items.clone()).await) - 6.0).abs() < 1e-9);
    assert!((f(&ok("math.avg", items).await) - 2.0).abs() < 1e-9);
}

#[tokio::test]
async fn aggregates_over_empty_are_null() {
    assert_eq!(ok("math.min", json!({"items": []})).await, json!(null));
    assert_eq!(ok("math.avg", json!({"items": ["x"]})).await, json!(null));
}

#[tokio::test]
async fn random_respects_range_and_integer_flag() {
    for _ in 0..50 {
        let v = ok("math.random", json!({"min": 5, "max": 10, "integer": true})).await;
        let i = v.as_i64().expect("integer random");
        assert!((5..10).contains(&i), "random {i} must land in [5,10)");
    }
    // Default is a float in [0,1).
    let d = f(&ok("math.random", json!({})).await);
    assert!(
        (0.0..1.0).contains(&d),
        "default random {d} must be in [0,1)"
    );
}

#[tokio::test]
async fn random_rejects_inverted_range() {
    let err = run("math.random", json!({"min": 10, "max": 1}))
        .await
        .unwrap_err();
    assert!(err.contains("max must be > min"), "got: {err}");
}
