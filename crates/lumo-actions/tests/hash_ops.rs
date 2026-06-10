//! Integration coverage for the `hash.*` / `util.*` action family (P1-8).
//! Digests are checked against the canonical "abc" test vectors.

mod common;
use common::{ok, run};
use serde_json::json;

#[tokio::test]
async fn sha_and_md5_vectors_for_abc() {
    assert_eq!(
        ok("hash.sha256", json!({"text": "abc"})).await,
        json!("ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad")
    );
    assert_eq!(
        ok("hash.sha512", json!({"text": "abc"})).await,
        json!("ddaf35a193617abacc417349ae20413112e6fa4e89a97ea20a9eeee64b55d39a2192992a274fc1a836ba3c23a3feebbd454d4423643ce80e2a9ac94fa54ca49f")
    );
    assert_eq!(
        ok("hash.sha1", json!({"text": "abc"})).await,
        json!("a9993e364706816aba3e25717850c26c9cd0d89d")
    );
    assert_eq!(
        ok("hash.md5", json!({"text": "abc"})).await,
        json!("900150983cd24fb0d6963f7d28e17f72")
    );
}

#[tokio::test]
async fn base64_round_trips() {
    assert_eq!(
        ok("util.base64_encode", json!({"text": "hello"})).await,
        json!("aGVsbG8=")
    );
    assert_eq!(
        ok("util.base64_decode", json!({"text": "aGVsbG8="})).await,
        json!("hello")
    );
}

#[tokio::test]
async fn base64_decode_rejects_garbage() {
    let err = run("util.base64_decode", json!({"text": "!!!not base64!!!"}))
        .await
        .unwrap_err();
    assert!(err.contains("base64"), "got: {err}");
}

#[tokio::test]
async fn url_encode_component_and_uri_semantics() {
    // 默认 component=true:encodeURIComponent 语义 —— /?&= 等结构字符全部
    // 转义,空格是 %20(不是表单语义的 +),JS 不转义的 -_.!~*'() 保留。
    assert_eq!(
        ok("util.url_encode", json!({"text": "a b/c?x=1&y=中"})).await,
        json!("a%20b%2Fc%3Fx%3D1%26y%3D%E4%B8%AD")
    );
    assert_eq!(
        ok("util.url_encode", json!({"text": "-_.!~*'()"})).await,
        json!("-_.!~*'()")
    );
    // component=false:encodeURI 语义 —— URL 结构保留字 /?:&=# 原样保留,
    // 可整条 URL 编码而不破坏结构。
    assert_eq!(
        ok(
            "util.url_encode",
            json!({"text": "https://e.com/a b?q=中#f", "component": false})
        )
        .await,
        json!("https://e.com/a%20b?q=%E4%B8%AD#f")
    );
}

#[tokio::test]
async fn url_decode_round_trips_and_rejects_bad_utf8() {
    assert_eq!(
        ok("util.url_decode", json!({"text": "a%20b%2Fc%3F%E4%B8%AD"})).await,
        json!("a b/c?中")
    );
    // `+` 不当空格 —— 那是 x-www-form-urlencoded 的表单语义,纯 percent
    // 解码原样保留。
    assert_eq!(
        ok("util.url_decode", json!({"text": "a+b"})).await,
        json!("a+b")
    );
    // 解码出非法 UTF-8 字节序列要显式报错,不能静默吞。
    let err = run("util.url_decode", json!({"text": "%FF%FE"}))
        .await
        .unwrap_err();
    assert!(err.contains("UTF-8"), "got: {err}");
}

#[tokio::test]
async fn uuid_is_a_v4_string() {
    let v = ok("util.uuid", json!({})).await;
    let s = v.as_str().expect("uuid is a string");
    assert_eq!(s.len(), 36, "canonical UUID length");
    assert_eq!(s.split('-').count(), 5, "five dash-separated groups");
    // Version nibble (first char of the 3rd group) is 4 for v4.
    assert_eq!(s.split('-').nth(2).unwrap().chars().next(), Some('4'));
}
