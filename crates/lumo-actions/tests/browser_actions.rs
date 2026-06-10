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

// ─── 指令集 P1: drag_and_drop / print_pdf / wait_response ────────────────────────

#[tokio::test]
async fn drag_and_drop_requires_a_from_selector() {
    // 没有 from/from_selectors → 在会话查找之前拒绝(CI 可测,不拉 Chrome)。
    let err = run(
        "browser.drag_and_drop",
        json!({ "to": "#dest" }),
    )
    .await
    .unwrap_err();
    assert!(err.contains("requires `from`"), "got: {err}");
    assert!(
        !err.contains("not launched"),
        "the from check must run before the session lookup, got: {err}"
    );
}

#[tokio::test]
async fn drag_and_drop_requires_exactly_one_target_form() {
    // 终点缺失 → 报缺 target。
    let err = run("browser.drag_and_drop", json!({ "from": "#src" }))
        .await
        .unwrap_err();
    assert!(err.contains("requires a target"), "got: {err}");
    // 坐标只给了一半 → 同样报缺 target(x/y 必须成对)。
    let err = run(
        "browser.drag_and_drop",
        json!({ "from": "#src", "x": 100 }),
    )
    .await
    .unwrap_err();
    assert!(err.contains("requires a target"), "got: {err}");
    // 选择器与坐标同时给 → 拒绝二义性输入。
    let err = run(
        "browser.drag_and_drop",
        json!({ "from": "#src", "to": "#dest", "x": 1, "y": 2 }),
    )
    .await
    .unwrap_err();
    assert!(err.contains("not both"), "got: {err}");
    // 校验全部先于会话查找。
    assert!(!err.contains("not launched"), "got: {err}");
}

#[tokio::test]
async fn drag_and_drop_without_session_is_a_clean_error() {
    // 输入合法但没有浏览器会话 → 干净的 "not launched"。
    let err = run(
        "browser.drag_and_drop",
        json!({ "from": "#src", "to": "#dest" }),
    )
    .await
    .unwrap_err();
    assert!(err.contains("not launched"), "got: {err}");
}

#[tokio::test]
async fn print_pdf_gates_fs_write_before_session() {
    // fs.write 闸门在会话查找之前 —— 未授权目标路径报 capability 错误而不是
    // "browser not launched",且全程不拉 Chrome(与 browser.screenshot 同序)。
    let err = run("browser.print_pdf", json!({ "path": "/tmp/lumo-page.pdf" }))
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
async fn wait_response_rejects_empty_pattern_and_bad_regex() {
    // 空 pattern → 在会话之前拒绝。
    let err = run("browser.wait_response", json!({ "url_pattern": "" }))
        .await
        .unwrap_err();
    assert!(err.contains("non-empty"), "got: {err}");
    // 坏正则 → 同样先于会话失败。
    let err = run(
        "browser.wait_response",
        json!({ "url_pattern": "(unclosed", "regex": true }),
    )
    .await
    .unwrap_err();
    assert!(err.contains("invalid regex"), "got: {err}");
    assert!(
        !err.contains("not launched"),
        "pattern validation must run before the session lookup, got: {err}"
    );
}

#[tokio::test]
async fn wait_response_rejects_empty_trigger_click() {
    let err = run(
        "browser.wait_response",
        json!({ "url_pattern": "/api", "trigger": { "click": "" } }),
    )
    .await
    .unwrap_err();
    assert!(err.contains("trigger.click"), "got: {err}");
    assert!(!err.contains("not launched"), "got: {err}");
}

#[tokio::test]
async fn wait_response_without_session_is_a_clean_error() {
    let err = run("browser.wait_response", json!({ "url_pattern": "/api" }))
        .await
        .unwrap_err();
    assert!(err.contains("not launched"), "got: {err}");
}

#[tokio::test]
#[ignore = "launches a real headless Chrome; run with --ignored"]
async fn drag_and_drop_moves_a_mouse_driven_box() {
    // 真实浏览器路径:页面用 mousedown/mousemove/mouseup 监听器实现一个可拖拽
    // 方块;browser.drag_and_drop {from:"#box", x:200, y:150} 后,方块左上角应
    // 落在 (180,130)(事件坐标减去 20px 的拖拽锚点偏移),且 down/up 各派发一次。
    use lumo_core::{ActionRegistry, FlowVm, RunOptions};
    use lumo_storage::Repo;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/drag"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(
            r#"<html><body>
<div id="box" style="position:absolute;left:20px;top:20px;width:40px;height:40px;background:red"></div>
<script>
  let dragging = false;
  const box = document.getElementById('box');
  window.__log = { down: 0, move: 0, up: 0 };
  document.addEventListener('mousedown', () => { dragging = true; window.__log.down++; });
  document.addEventListener('mousemove', (e) => {
    if (!dragging) return;
    window.__log.move++;
    box.style.left = (e.clientX - 20) + 'px';
    box.style.top = (e.clientY - 20) + 'px';
  });
  document.addEventListener('mouseup', () => { dragging = false; window.__log.up++; });
</script>
</body></html>"#,
            "text/html",
        ))
        .mount(&server)
        .await;

    let repo = Repo::open_in_memory().unwrap();
    let mut reg = ActionRegistry::new();
    lumo_actions::register_all(&mut reg);
    let vm = FlowVm::new(reg, Some(repo.clone()));

    let flow = lumo_dsl::parse_str(&format!(
        // YAML 里 `"#box"` 含 `"#`,须用双井号原始串。
        r##"
apiVersion: lumorpa.io/v1
kind: Flow
metadata: {{ id: drag-e2e }}
spec:
  capabilities:
    network: ["*"]
  steps:
    - {{ id: launch, action: browser.launch, with: {{ headless: true }} }}
    - {{ id: open, action: browser.open, with: {{ url: "{url}/drag" }} }}
    - {{ id: drag, action: browser.drag_and_drop, with: {{ from: "#box", x: 200, y: 150 }} }}
    - id: read
      action: browser.eval
      with:
        expr: "[parseInt(document.getElementById('box').style.left), parseInt(document.getElementById('box').style.top), window.__log.down, window.__log.up]"
"##,
        url = server.uri(),
    ))
    .expect("parse flow");

    let report = vm.run(&flow, RunOptions::default()).await.expect("run ok");
    assert!(report.success, "flow should succeed");

    let steps = repo.list_steps(&report.run_id).expect("list steps");
    let read = steps.iter().find(|s| s.step_id == "read").expect("read row");
    let out = read.output_json.as_ref().expect("eval output");
    assert_eq!(
        out,
        &serde_json::json!([180, 130, 1, 1]),
        "box must land at (180,130) with exactly one mousedown/mouseup"
    );
}

#[tokio::test]
#[ignore = "launches a real headless Chrome; run with --ignored"]
async fn print_pdf_writes_a_pdf_and_attaches_an_artifact() {
    // 真实浏览器路径:launch → open(wiremock 页)→ print_pdf。验证写出的文件
    // 以 %PDF- 开头、输出携带 artifact_id 且 artifacts 表行 kind=pdf /
    // mime=application/pdf —— 与 screenshot 的 X-07 闭环同构。
    use lumo_core::{ActionRegistry, FlowVm, RunOptions};
    use lumo_storage::Repo;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/p"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(
            "<html><body><h1>print me</h1></body></html>",
            "text/html",
        ))
        .mount(&server)
        .await;

    let tmp = tempfile::tempdir().unwrap();
    let artifacts_dir = tmp.path().join("artifacts");
    let pdf_path = tmp.path().join("page.pdf");
    let repo = Repo::open_in_memory().unwrap();

    let mut reg = ActionRegistry::new();
    lumo_actions::register_all(&mut reg);
    let vm = FlowVm::new(reg, Some(repo.clone())).with_artifacts_dir(Some(artifacts_dir));

    let tmp_glob = format!("{}/**", tmp.path().display());
    let flow = lumo_dsl::parse_str(&format!(
        r#"
apiVersion: lumorpa.io/v1
kind: Flow
metadata: {{ id: print-pdf-e2e }}
spec:
  capabilities:
    network: ["*"]
    fs.write: ["{tmp_glob}"]
  steps:
    - {{ id: launch, action: browser.launch, with: {{ headless: true }} }}
    - {{ id: open, action: browser.open, with: {{ url: "{url}/p" }} }}
    - {{ id: pdf, action: browser.print_pdf, with: {{ path: "{pdf}", landscape: true }} }}
"#,
        url = server.uri(),
        pdf = pdf_path.display(),
    ))
    .expect("parse flow");

    let report = vm.run(&flow, RunOptions::default()).await.expect("run ok");
    assert!(report.success, "flow should succeed");

    let bytes = std::fs::read(&pdf_path).expect("pdf written");
    assert!(bytes.starts_with(b"%PDF-"), "output is a real PDF");

    let rows = repo.list_artifacts(&report.run_id).expect("list artifacts");
    let pdf_row = rows.iter().find(|r| r.kind == "pdf").expect("pdf artifact row");
    assert_eq!(pdf_row.mime, "application/pdf");
    assert_eq!(pdf_row.step_id.as_deref(), Some("pdf"));
}

#[tokio::test]
#[ignore = "launches a real headless Chrome; run with --ignored"]
async fn wait_response_catches_a_clicked_fetch() {
    // 真实浏览器路径:open 一个按钮页(点击后 fetch /api/data),
    // wait_response {url_pattern:"/api/data", trigger:{click:"#go"}, include_body:true}
    // 返回 {url, status:200, body} —— 先监听后点击,响应不会因竞态丢失。
    use lumo_core::{ActionRegistry, FlowVm, RunOptions};
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/page"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(
            r#"<html><body><button id="go"
                 onclick="fetch('/api/data').then(r => r.text())">go</button></body></html>"#,
            "text/html",
        ))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/data"))
        .respond_with(
            ResponseTemplate::new(200).set_body_raw(r#"{"ok":true}"#, "application/json"),
        )
        .mount(&server)
        .await;

    let mut reg = ActionRegistry::new();
    lumo_actions::register_all(&mut reg);
    let vm = FlowVm::new(reg, None);

    let flow = lumo_dsl::parse_str(&format!(
        // 注意:YAML 里的 `"#go"` 含 `"#`,会终结单井号原始串,故用 r##…##。
        r##"
apiVersion: lumorpa.io/v1
kind: Flow
metadata: {{ id: wait-response-e2e }}
spec:
  capabilities:
    network: ["*"]
  steps:
    - {{ id: launch, action: browser.launch, with: {{ headless: true }} }}
    - {{ id: open, action: browser.open, with: {{ url: "{url}/page" }} }}
    - id: wait
      action: browser.wait_response
      with:
        url_pattern: "/api/data"
        include_body: true
        trigger: {{ click: "#go" }}
"##,
        url = server.uri(),
    ))
    .expect("parse flow");

    let report = vm.run(&flow, RunOptions::default()).await.expect("run ok");
    assert!(report.success, "flow should succeed: {:?}", report);
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

// ─── X-07 artifacts 生产者(时光回溯)────────────────────────────────────────────

/// 真实浏览器路径的 artifacts 闭环:launch → open(本地 wiremock 页,data: URL
/// 没有 host 过不了 network 闸门)→ screenshot → extract_table。VM 接上
/// artifacts_dir + 内存 repo 后,两个生产者各归档一个 artifact —— 表行的
/// kind/mime/步骤归属正确,blob 逐字节可读(截图是真 PNG,表格 blob 解析回抽取
/// 结果),screenshot 输出新增的 `artifact_id` 与表行一致。
/// 不依赖浏览器的归档/no-op 契约见 `lumo-core/tests/artifacts.rs`。
#[tokio::test]
#[ignore = "launches a real headless Chrome; run with --ignored"]
async fn screenshot_and_extract_table_attach_artifacts() {
    use lumo_core::{ActionRegistry, FlowVm, RunOptions};
    use lumo_storage::Repo;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/t"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(
            "<html><body><table><tr><th>name</th><th>qty</th></tr>\
             <tr><td>widget</td><td>3</td></tr></table></body></html>",
            "text/html",
        ))
        .mount(&server)
        .await;

    let tmp = tempfile::tempdir().unwrap();
    let artifacts_dir = tmp.path().join("artifacts");
    let shot = tmp.path().join("shot.png");
    let repo = Repo::open_in_memory().unwrap();

    let mut reg = ActionRegistry::new();
    lumo_actions::register_all(&mut reg);
    let vm = FlowVm::new(reg, Some(repo.clone())).with_artifacts_dir(Some(artifacts_dir));

    let tmp_glob = format!("{}/**", tmp.path().display());
    let flow = lumo_dsl::parse_str(&format!(
        r#"
apiVersion: lumorpa.io/v1
kind: Flow
metadata: {{ id: artifacts-e2e }}
spec:
  capabilities:
    network: ["*"]
    fs.write: ["{tmp_glob}"]
  steps:
    - {{ id: launch, action: browser.launch, with: {{ headless: true }} }}
    - {{ id: open, action: browser.open, with: {{ url: "{url}/t" }} }}
    - {{ id: shot, action: browser.screenshot, with: {{ path: "{shot}" }} }}
    - {{ id: tbl, action: browser.extract_table, with: {{ selector: "table" }} }}
"#,
        url = server.uri(),
        shot = shot.display(),
    ))
    .expect("parse flow");

    let report = vm.run(&flow, RunOptions::default()).await.expect("run ok");
    assert!(report.success, "flow should succeed");

    let rows = repo.list_artifacts(&report.run_id).expect("list artifacts");
    let shot_row = rows
        .iter()
        .find(|r| r.kind == "screenshot")
        .expect("screenshot artifact row");
    assert_eq!(shot_row.mime, "image/png");
    assert_eq!(shot_row.step_id.as_deref(), Some("shot"));
    let tbl_row = rows
        .iter()
        .find(|r| r.kind == "table")
        .expect("table artifact row");
    assert_eq!(tbl_row.mime, "application/json");
    assert_eq!(tbl_row.step_id.as_deref(), Some("tbl"));

    let png = std::fs::read(&shot_row.blob_path).expect("png blob readable");
    assert!(png.starts_with(b"\x89PNG"), "screenshot blob is a real PNG");
    let table: serde_json::Value = serde_json::from_slice(
        &std::fs::read(&tbl_row.blob_path).expect("table blob readable"),
    )
    .expect("table blob is valid JSON");
    assert_eq!(table[0]["name"], "widget");
    assert_eq!(table[0]["qty"], "3");

    // screenshot 步骤输出回报的 artifact_id 与 artifacts 表行一致。
    let steps = repo.list_steps(&report.run_id).expect("list steps");
    let shot_step = steps.iter().find(|s| s.step_id == "shot").expect("shot row");
    assert_eq!(
        shot_step
            .output_json
            .as_ref()
            .and_then(|o| o.get("artifact_id"))
            .and_then(serde_json::Value::as_str),
        Some(shot_row.id.as_str()),
        "browser.screenshot output carries the artifact id"
    );
}
