//! Real browser acceptance coverage. Explicitly opt in locally and in the
//! desktop-e2e workflow; a missing browser is a failure, never a silent skip.

use lumo_core::{ActionRegistry, FlowVm, RunOptions};
use lumo_storage::Repo;
use serde_json::json;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test]
#[ignore = "requires Chrome/Chromium; run --test browser_workflow_e2e -- --ignored"]
async fn real_browser_form_upload_cookies_and_iframe_persist_outputs() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/form"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(
            r#"<!doctype html><title>Lumo browser acceptance</title>
<input id="name" value="seed"><select id="edition">
<option value="old">Old</option><option value="new">New</option></select>
<input id="attachment" type="file"><button id="commit">Commit</button>
<output id="result"></output>
<iframe name="child" src="/child" style="display:block;margin:40px;border:5px solid;width:400px;height:250px"></iframe>
<script>
document.querySelector('#commit').onclick = event => {
  window.commitTrusted = event.isTrusted;
  document.querySelector('#result').textContent = document.querySelector('#name').value
    + ':' + document.querySelector('#edition').value
    + ':' + document.querySelector('#attachment').files[0].name;
};
</script>"#,
            "text/html",
        ))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/child"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(
            r#"<!doctype html><input id="child-name" value="seed">
<button id="child-commit">Commit child</button><script>
document.querySelector('#child-commit').onclick = event => {
  window.result = [document.querySelector('#child-name').value, event.isTrusted];
};
</script>"#,
            "text/html",
        ))
        .mount(&server)
        .await;

    let temp = tempfile::tempdir().expect("tempdir");
    let attachment = temp.path().canonicalize().unwrap().join("attachment.txt");
    std::fs::write(&attachment, "Lumo acceptance fixture").unwrap();
    let flow = lumo_dsl::parse_str(
        &json!({
            "apiVersion": "lumorpa.io/v1", "kind": "Flow",
            "metadata": { "id": "browser-workflow-e2e" },
            "spec": {
                "capabilities": { "network": ["*"], "fs.read": [attachment] },
                "steps": [
                    { "id": "launch", "action": "browser.launch", "with": { "headless": true } },
                    { "id": "open", "action": "browser.open", "with": { "url": format!("{}/form", server.uri()) } },
                    { "id": "name", "action": "browser.type", "with": { "selector": "#name", "text": "Lumo", "clear": true } },
                    { "id": "edition", "action": "browser.select", "with": { "selector": "#edition", "value": "new" } },
                    { "id": "cookie", "action": "browser.set_cookie", "with": { "name": "lumo_acceptance", "value": "confirmed" } },
                    { "id": "upload", "action": "browser.upload", "with": { "selector": "#attachment", "files": [attachment] } },
                    { "id": "commit", "action": "browser.click", "with": { "selector": "#commit" } },
                    { "id": "child-name", "action": "browser.type", "with": { "selector": "#child-name", "text": "iframe typed", "clear": true, "frame": { "name": "child" } } },
                    { "id": "child-commit", "action": "browser.click", "with": { "selector": "#child-commit", "frame": { "name": "child" } } },
                    { "id": "child-output", "action": "browser.frame", "with": { "op": "eval", "name": "child", "expr": "window.result" } },
                    { "id": "output", "action": "browser.eval", "with": { "expr": "[document.querySelector('#result').textContent, window.commitTrusted, document.cookie.includes('lumo_acceptance=confirmed')]" } }
                ]
            }
        })
        .to_string(),
    )
    .expect("valid flow");
    let repo = Repo::open_in_memory().unwrap();
    let mut registry = ActionRegistry::new();
    lumo_actions::register_all(&mut registry);
    let vm = FlowVm::new(registry, Some(repo.clone()));
    let report = vm.run(&flow, RunOptions::default()).await.expect("run");
    let steps = repo.list_steps(&report.run_id).unwrap();
    assert!(report.success, "browser flow failed: {steps:?}");
    let output = |id: &str| {
        steps
            .iter()
            .find(|step| step.step_id == id)
            .and_then(|step| step.output_json.as_ref())
            .unwrap_or_else(|| panic!("missing persisted output for {id}"))
    };
    assert_eq!(
        output("output"),
        &json!(["Lumo:new:attachment.txt", true, true])
    );
    assert_eq!(output("child-output"), &json!(["iframe typed", true]));
    assert!(steps.iter().all(|step| step.state == "ok"));
}
