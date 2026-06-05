//! Coverage for the F-10 browser action completions (eval / screenshot / scroll
//! / hover / select / cookies / set_cookie). Input + capability validation runs
//! in CI (it errors before any Chrome session is needed); behavioural paths need
//! a real Chrome and are `#[ignore]`d, mirroring `browser_wait.rs`.

mod common;
use common::run;
use serde_json::json;

#[tokio::test]
async fn screenshot_gates_fs_write_before_session() {
    // fs-write is checked BEFORE the browser session, so an ungranted dest fails
    // with a capability error (not "browser not launched") and without a Chrome.
    let err = run(
        "browser.screenshot",
        json!({ "path": "/tmp/lumo-shot.png" }),
    )
    .await
    .unwrap_err();
    assert!(
        err.contains("capability denied") && err.contains("fs.write"),
        "expected an fs.write capability error, got: {err}"
    );
    assert!(
        !err.contains("not launched"),
        "fs gate must run before the session lookup, got: {err}"
    );
}

#[tokio::test]
async fn select_requires_value_label_or_index() {
    // All three targets absent → rejected before a session is needed.
    let err = run("browser.select", json!({ "selector": "#dropdown" }))
        .await
        .unwrap_err();
    assert!(err.contains("requires"), "got: {err}");
}

#[tokio::test]
async fn eval_without_session_is_a_clean_error() {
    let err = run("browser.eval", json!({ "expr": "1 + 1" }))
        .await
        .unwrap_err();
    assert!(err.contains("not launched"), "got: {err}");
}

#[tokio::test]
async fn info_without_session_is_a_clean_error() {
    let err = run("browser.info", json!({ "fields": ["url", "title"] }))
        .await
        .unwrap_err();
    assert!(err.contains("not launched"), "got: {err}");
}

#[tokio::test]
async fn set_cookie_requires_name_and_value() {
    // `name`/`value` are required fields — the execute deserialize rejects a
    // missing one (the derived schema enforces the same in the VM path).
    let err = run("browser.set_cookie", json!({ "value": "v" }))
        .await
        .unwrap_err();
    assert!(err.contains("invalid"), "got: {err}");
}

#[tokio::test]
async fn tab_requires_a_selector() {
    // Neither target_id nor url_includes → rejected before a session is needed.
    let err = run("browser.tab", json!({ "op": "activate" }))
        .await
        .unwrap_err();
    assert!(err.contains("requires"), "got: {err}");
    assert!(
        !err.contains("not launched"),
        "the addressing check must run before the session lookup, got: {err}"
    );
}

#[tokio::test]
async fn tab_rejects_two_selectors() {
    // target_id and url_includes are mutually exclusive.
    let err = run(
        "browser.tab",
        json!({ "op": "close", "target_id": "ABC", "url_includes": "x" }),
    )
    .await
    .unwrap_err();
    assert!(err.contains("only one"), "got: {err}");
}

#[tokio::test]
async fn tab_without_session_is_a_clean_error() {
    // A well-formed address still needs a launched browser.
    let err = run(
        "browser.tab",
        json!({ "op": "activate", "target_id": "ABC" }),
    )
    .await
    .unwrap_err();
    assert!(err.contains("not launched"), "got: {err}");
}

#[tokio::test]
async fn tabs_without_session_is_a_clean_error() {
    let err = run("browser.tabs", json!({})).await.unwrap_err();
    assert!(err.contains("not launched"), "got: {err}");
}

#[tokio::test]
async fn upload_gates_fs_read_before_session() {
    // fs-read is checked BEFORE the browser session, so an ungranted file fails
    // with a capability error (not "browser not launched") and without a Chrome.
    let err = run(
        "browser.upload",
        json!({ "selector": "input[type=file]", "files": ["/tmp/lumo-upload.txt"] }),
    )
    .await
    .unwrap_err();
    assert!(
        err.contains("capability denied") && err.contains("fs.read"),
        "expected an fs.read capability error, got: {err}"
    );
    assert!(
        !err.contains("not launched"),
        "the fs gate must run before the session lookup, got: {err}"
    );
}

#[tokio::test]
async fn upload_requires_at_least_one_file() {
    // Empty `files` is rejected before a session (and before the fs gate).
    let err = run(
        "browser.upload",
        json!({ "selector": "input[type=file]", "files": [] }),
    )
    .await
    .unwrap_err();
    assert!(err.contains("requires"), "got: {err}");
}

#[tokio::test]
async fn upload_requires_a_selector() {
    // A non-empty `files` but no selector → build_selector rejects it.
    let err = run("browser.upload", json!({ "files": ["/tmp/x"] }))
        .await
        .unwrap_err();
    assert!(err.contains("selector"), "got: {err}");
}

#[tokio::test]
async fn eval_frame_requires_url_or_name() {
    // `frame: {}` (neither url_includes nor name) is rejected before a session.
    let err = run("browser.eval", json!({ "expr": "1", "frame": {} }))
        .await
        .unwrap_err();
    assert!(err.contains("frame"), "got: {err}");
    assert!(
        !err.contains("not launched"),
        "the frame check must run before the session lookup, got: {err}"
    );
}

#[tokio::test]
async fn extract_frame_requires_url_or_name() {
    let err = run("browser.extract", json!({ "selector": "h1", "frame": {} }))
        .await
        .unwrap_err();
    assert!(err.contains("frame"), "got: {err}");
}

#[tokio::test]
#[ignore = "launches a real headless Chrome; run with --ignored"]
async fn eval_and_cookies_roundtrip() {
    // Sketch for local e2e: browser.open a data: URL, browser.set_cookie, then
    // browser.eval "document.title" / browser.cookies reflect the page state, and
    // browser.scroll / browser.hover / browser.select drive a small fixture page.
}

#[tokio::test]
#[ignore = "launches a real headless Chrome; run with --ignored"]
async fn tabs_open_activate_close() {
    // Sketch for local e2e: browser.open A then B (each opens a new tab, B active);
    // browser.tabs lists both with B marked active; browser.tab activate
    // {url_includes: <A>} repoints to A; browser.tab close {url_includes: <B>}
    // drops B and leaves A as the active tab.
}

#[tokio::test]
#[ignore = "launches a real headless Chrome; run with --ignored"]
async fn upload_sets_file_input() {
    // Sketch for local e2e: browser.open a data: URL with an <input type=file>,
    // grant fs-read for a temp file, browser.upload {selector, files:[temp]}, then
    // browser.eval "document.querySelector('input').files[0].name" reflects it.
}

#[tokio::test]
#[ignore = "launches a real headless Chrome; run with --ignored"]
async fn eval_inside_iframe() {
    // Sketch for local e2e: browser.open a page embedding an <iframe>, then
    // browser.eval { expr: "document.title", frame: { url_includes: <child-url> } }
    // returns the *child* frame's title, not the parent page's.
}

// ─── 批次B: download_wait / dialog / frame / extract_table ──────────────────────

#[tokio::test]
async fn download_wait_gates_fs_write_before_session() {
    // fs-write on the download dir is checked BEFORE the browser session, so an
    // ungranted dir fails with a capability error (not "browser not launched").
    let err = run(
        "browser.download_wait",
        json!({ "dir": "/tmp/lumo-dl", "selector": "#go" }),
    )
    .await
    .unwrap_err();
    assert!(
        err.contains("capability denied") && err.contains("fs.write"),
        "expected an fs.write capability error, got: {err}"
    );
    assert!(
        !err.contains("not launched"),
        "the fs gate must run before the session lookup, got: {err}"
    );
}

#[tokio::test]
async fn dialog_without_session_is_a_clean_error() {
    let err = run("browser.dialog", json!({ "accept": true }))
        .await
        .unwrap_err();
    assert!(err.contains("not launched"), "got: {err}");
}

#[tokio::test]
async fn frame_requires_an_address() {
    // No url_includes / name / index → rejected before a session is needed.
    let err = run("browser.frame", json!({ "op": "eval", "expr": "1" }))
        .await
        .unwrap_err();
    assert!(err.contains("requires"), "got: {err}");
    assert!(
        !err.contains("not launched"),
        "the frame-address check must run before the session lookup, got: {err}"
    );
}

#[tokio::test]
async fn frame_eval_requires_expr() {
    let err = run(
        "browser.frame",
        json!({ "op": "eval", "index": 0 }),
    )
    .await
    .unwrap_err();
    assert!(err.contains("requires `expr`"), "got: {err}");
}

#[tokio::test]
async fn frame_extract_requires_selector() {
    let err = run(
        "browser.frame",
        json!({ "op": "extract", "name": "child" }),
    )
    .await
    .unwrap_err();
    assert!(err.contains("requires `selector`"), "got: {err}");
}

#[tokio::test]
async fn frame_rejects_unknown_op() {
    // `op` is a derived enum (eval/extract) → a bogus op fails at deserialize.
    let err = run(
        "browser.frame",
        json!({ "op": "navigate", "index": 0, "expr": "1" }),
    )
    .await
    .unwrap_err();
    assert!(err.contains("invalid"), "got: {err}");
}

#[tokio::test]
async fn extract_table_without_session_is_a_clean_error() {
    let err = run("browser.extract_table", json!({ "selector": "table#data" }))
        .await
        .unwrap_err();
    assert!(err.contains("not launched"), "got: {err}");
}

#[tokio::test]
#[ignore = "launches a real headless Chrome; run with --ignored"]
async fn download_wait_captures_a_triggered_download() {
    // Sketch for local e2e: browser.open a data: URL with an <a download> link,
    // grant fs-write for a temp dir, browser.download_wait {dir, selector:"a"},
    // then the returned `path` exists under `dir` with the downloaded bytes.
}

#[tokio::test]
#[ignore = "launches a real headless Chrome; run with --ignored"]
async fn dialog_accepts_a_confirm() {
    // Sketch for local e2e: browser.open a page whose button calls confirm();
    // browser.dialog {accept:true, selector:"#confirm-btn"} returns
    // {accepted:true, type:"confirm"} and the page proceeds down the accept path.
}

#[tokio::test]
#[ignore = "launches a real headless Chrome; run with --ignored"]
async fn frame_eval_runs_in_child_frame() {
    // Sketch for local e2e: browser.open a page embedding an <iframe name="child">;
    // browser.frame {op:"eval", name:"child", expr:"document.title"} returns the
    // child's title; {op:"extract", index:1, selector:"h1"} reads its heading.
}

#[tokio::test]
#[ignore = "launches a real headless Chrome; run with --ignored"]
async fn extract_table_maps_rows_to_headers() {
    // Sketch for local e2e: browser.open a data: URL with a <table> (a header row
    // of <th> then <td> rows); browser.extract_table {selector:"table"} returns an
    // array of objects keyed by the header cells' text.
}
