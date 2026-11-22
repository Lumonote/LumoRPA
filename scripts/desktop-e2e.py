#!/usr/bin/env python3
"""Drive the shipped Tauri webview over W3C WebDriver, with no mocked IPC.

Linux: install tauri-driver + WebKitWebDriver, build the desktop binary, then
run under xvfb-run. Uses only the Python standard library. See desktop-e2e.md.
"""

import argparse
import base64
import json
import os
from pathlib import Path
import shutil
import socket
import subprocess
import sys
import tempfile
import time
import urllib.error
import urllib.request

from acceptance_errors import run_acceptance


class WebDriverError(RuntimeError):
    pass


class Driver:
    def __init__(self, base_url):
        self.base_url = base_url
        self.session = None
        self.http = urllib.request.build_opener(urllib.request.ProxyHandler({}))

    def request(self, method, path, body=None):
        data = None if body is None else json.dumps(body).encode()
        request = urllib.request.Request(
            self.base_url + path, data=data, method=method,
            headers={"Content-Type": "application/json"},
        )
        try:
            with self.http.open(request, timeout=40) as response:
                result = json.load(response)
        except urllib.error.HTTPError as error:
            raise WebDriverError(error.read().decode()) from error
        value = result.get("value")
        if isinstance(value, dict) and value.get("error"):
            raise WebDriverError(json.dumps(value))
        return value

    def command(self, method, path, body=None):
        return self.request(method, f"/session/{self.session}{path}", body)

    def start(self, application):
        value = self.request("POST", "/session", {"capabilities": {"alwaysMatch": {
            "tauri:options": {"application": str(application)},
            "unhandledPromptBehavior": "ignore",
        }}})
        self.session = value["sessionId"]
        self.command("POST", "/timeouts", {"script": 30000, "pageLoad": 60000, "implicit": 0})
        self.switch_to("#newFlowBtn")

    def switch_to(self, selector):
        def select_window():
            for handle in self.command("GET", "/window/handles"):
                self.command("POST", "/window", {"handle": handle})
                if self.evaluate("Boolean(document.querySelector(arguments[0]))", selector):
                    return handle
            return None
        return wait_for(f"webview containing {selector}", select_window, timeout=60)

    def evaluate(self, expression, *args):
        result = self.command("POST", "/execute/async", {
            "script": "const done = arguments[arguments.length - 1];"
                      f"Promise.resolve().then(async () => ({expression}))"
                      ".then(value => done({ok:true,value}),"
                      "error => done({ok:false,error:String(error)}));",
            "args": list(args),
        })
        if not result.get("ok"):
            raise WebDriverError(result.get("error", "JavaScript evaluation failed"))
        return result.get("value")

    def invoke(self, command, **args):
        return self.evaluate("window.__TAURI__.core.invoke(arguments[0], arguments[1])", command, args)

    def state(self, field):
        return self.evaluate("(await import(new URL('./js/state.js', document.baseURI).href)).state[arguments[0]]", field)

    def element(self, selector):
        value = self.command("POST", "/element", {"using": "css selector", "value": selector})
        return value["element-6066-11e4-a52e-4f735466cecf"]

    def click(self, selector):
        self.command("POST", f"/element/{self.element(selector)}/click", {})

    def fill(self, selector, text):
        element = self.element(selector)
        self.command("POST", f"/element/{element}/clear", {})
        self.command("POST", f"/element/{element}/value", {"text": text})

    def text(self, selector):
        return self.command("GET", f"/element/{self.element(selector)}/text")

    def drag(self, selector, dx, dy):
        rect = self.command("GET", f"/element/{self.element(selector)}/rect")
        x, y = round(rect['x'] + rect['width'] / 2), round(rect['y'] + rect['height'] / 2)
        try:
            self.command("POST", "/actions", {"actions": [{
                "type": "pointer", "id": "canvas-mouse", "parameters": {"pointerType": "mouse"},
                "actions": [
                    {"type": "pointerMove", "duration": 0, "origin": "viewport", "x": x, "y": y},
                    {"type": "pointerDown", "button": 0},
                    {"type": "pointerMove", "duration": 300, "origin": "viewport", "x": x + dx, "y": y + dy},
                    {"type": "pointerUp", "button": 0},
                ],
            }]})
        finally:
            self.command("DELETE", "/actions")

    def chord(self, key):
        try:
            self.command('POST', '/actions', {'actions': [{
                'type': 'key', 'id': 'keyboard', 'actions': [
                    {'type': 'keyDown', 'value': '\ue009'},
                    {'type': 'keyDown', 'value': key}, {'type': 'keyUp', 'value': key},
                    {'type': 'keyUp', 'value': '\ue009'},
                ],
            }]})
        finally:
            self.command('DELETE', '/actions')

    def capture(self, directory):
        if not self.session:
            return
        for name, action in [
            ("failure.png", lambda: base64.b64decode(self.command("GET", "/screenshot"))),
            ("failure.html", lambda: self.command("GET", "/source").encode()),
        ]:
            try:
                (directory / name).write_bytes(action())
            except (WebDriverError, OSError, urllib.error.URLError):
                pass

    def close(self):
        if self.session:
            try:
                self.command("DELETE", "")
            except (WebDriverError, OSError, urllib.error.URLError):
                pass


def wait_for(description, predicate, timeout=30):
    deadline = time.monotonic() + timeout
    last_error = None
    while time.monotonic() < deadline:
        try:
            result = predicate()
            if result:
                return result
        except (WebDriverError, urllib.error.URLError) as error:
            last_error = error
        time.sleep(0.1)
    raise AssertionError(f"timed out waiting for {description}; last error: {last_error}")


def flow_source(marker, fail=False):
    action = "control.fail" if fail else "control.log"
    return f"""apiVersion: lumorpa.io/v1
kind: Flow
metadata:
  id: desktop-e2e
  name: Desktop acceptance fixture
spec:
  inputs:
    - {{ name: who, type: string, default: fixture }}
  steps:
    - id: echo
      action: {action}
      with:
        message: "{marker} {{{{ inputs.who }}}}"
"""


def run_editor(driver, source, marker):
    previous_ids = {run["id"] for run in driver.invoke("list_runs", limit=60)}
    driver.fill("#codeEditor", source)
    driver.click('#rightTabs [data-section="inputs"]')
    driver.fill("#inputsJson", '{"who":"desktop"}')
    driver.click("#runBtn")
    wait_for("execution result in the UI", lambda: marker in driver.text("#outputBox")
             and driver.evaluate("!document.querySelector('#runBtn').disabled"))
    runs = driver.invoke("list_runs", limit=60)
    run = next((run for run in runs if run["id"] not in previous_ids and run["flowId"] == "desktop-e2e"), None)
    assert run, "the editor result was not persisted"
    detail = driver.invoke("show_run", runId=run["id"])
    assert len(detail["steps"]) == 1, detail
    return detail


def start_agent(driver, node, risk="L1"):
    return driver.invoke("agent_start", input={
        "utterance": f"Desktop acceptance {node}",
        "suppliedPlan": {"id": "e2e", "objective": "Run a local log fixture", "nodes": [{
            "id": node, "dependsOn": [], "capabilityId": "flow:desktop-e2e",
            "arguments": {"who": "agent"}, "risk": risk, "timeoutMs": 10000, "retryLimit": 0,
        }]},
    })["runId"]


def terminal_events(driver, run_id):
    events = driver.invoke("agent_events", runId=run_id, afterSeq=0)
    return events if any(event["kind"] == "run.completed" for event in events) else None


def schedule_acceptance(driver, path):
    previous_ids = {job["id"] for job in driver.invoke("job_list")}
    driver.click("#scheduleFlowBtn")
    driver.click('[data-flow-schedule] [name="kind"] option[value="cron"]')
    for field, value in {
        "expression": "0 0 9 1 1 * 2099", "timezone": "Asia/Shanghai",
        "inputs": '{"who":"scheduled"}', "maxAttempts": "4",
        "timeoutSeconds": "60", "priority": "3",
    }.items():
        driver.fill(f'[data-flow-schedule] [name="{field}"]', value)
    driver.click('[data-flow-schedule] button[type="submit"]')
    job = wait_for("future scheduled job", lambda: next((job for job in driver.invoke("job_list")
                   if job["id"] not in previous_ids), None))
    assert job["schedule_kind"] == "cron" and job["state"] == "queued", job
    assert job["schedule_spec"]["timezone"] == "Asia/Shanghai", job
    assert job["max_attempts"] == 4 and job["priority"] == 3, job
    node = job["payload"]["suppliedPlan"]["nodes"][0]
    assert node["arguments"] == {"who": "scheduled"} and node["timeoutMs"] == 60000, job
    assert job["payload"]["suppliedPlan"]["metadata"]["flowPath"] == path, job
    driver.click('.tabs [data-view="capability-hub"]')
    driver.click('[data-hub-section="jobs"]')
    edit = f'[data-job-action="edit"][data-job-id="{job["id"]}"]'
    wait_for("editable scheduled job", lambda: driver.element(edit))
    driver.click(edit)
    driver.fill('[data-flow-schedule] [name="maxAttempts"]', "5")
    driver.click('[data-flow-schedule] button[type="submit"]')
    wait_for("updated retry setting", lambda: any(item["id"] == job["id"] and item["max_attempts"] == 5
             for item in driver.invoke("job_list")))
    try:
        driver.invoke("job_update_schedule", jobId=job["id"], expectedUpdatedAt=job["updated_at"], request={
            "path": path, "inputs": {"who": "stale"}, "maxAttempts": 1,
            "schedule": {"kind": "cron", "expression": "0 0 9 1 1 * 2099", "timezone": "UTC", "misfire": "skip"},
        })
    except WebDriverError as error:
        assert "任务已变化" in str(error), error
    else:
        raise AssertionError("stale schedule edit was accepted")
    driver.click(f'[data-job-action="cancel"][data-job-id="{job["id"]}"]')
    wait_for("scheduled fixture cancellation", lambda: any(item["id"] == job["id"] and item["cancelled_at"]
             for item in driver.invoke("job_list")))


def canvas_acceptance(driver, path, restore_source):
    source = flow_source("desktop-e2e-canvas") + """    - id: capture
      action: control.set_var
      with:
        name: captured
        value: before
"""
    driver.fill("#codeEditor", source)
    driver.click('#viewSwitch [data-mode="graph"]')
    driver.click("#graphFit")
    wait_for("two canvas nodes", lambda: driver.evaluate("document.querySelectorAll('.graph-node').length") == 2)
    # Exercise palette insertion and undo through the shipped editor handlers.
    driver.click("#graphAddNode")
    driver.fill("#graphActionSearch", "control.log")
    driver.click('[data-graph-action="control.log"]')
    wait_for("palette node insertion", lambda: driver.evaluate("document.querySelectorAll('.graph-node').length") == 3)
    driver.click("#undoBtn")
    wait_for("undo palette insertion", lambda: driver.evaluate("document.querySelectorAll('.graph-node').length") == 2)
    driver.click("#graphFit")
    position = "(() => { const node = document.querySelector('.graph-node[data-step-path=\"0\"]').closest('foreignObject'); return [node.getAttribute('x'), node.getAttribute('y')]; })()"
    before = driver.evaluate(position)
    driver.drag('.graph-node[data-step-path="0"] .graph-node-title', 64, 48)
    wait_for("canvas node drag", lambda: driver.evaluate(position) != before)
    driver.click("#undoBtn")
    wait_for("canvas layout undo", lambda: driver.evaluate(position) == before)
    driver.click('button[data-node-key="0"][data-port-kind="data"][data-port-direction="output"][data-port-output="[]"]')
    driver.click('button[data-node-key="1"][data-port-kind="data"][data-port-input="value"]')
    wait_for("binding replacement confirmation", lambda: driver.evaluate("!document.querySelector('#graphConnectConfirm').hidden"))
    driver.click("#graphConnectApply")
    wait_for("YAML data binding", lambda: driver.state("source")
                      if "steps.echo.result" in driver.state("source") else None)
    driver.click('.graph-node[data-step-path="0"] .graph-node-title')
    driver.click('#rightTabs [data-section="inspector"]')
    wait_for('loaded node inspector', lambda: driver.evaluate("!document.querySelector('#applyStepBtn').disabled"))
    driver.fill('[data-prop="id"]', 'renamed_echo')
    driver.click('#applyStepBtn')
    linked = wait_for('renamed data binding', lambda: driver.state('source')
                      if 'steps.renamed_echo.result' in driver.state('source') else None)
    # Keyboard copy/paste must create a real node and be atomically undoable.
    driver.click('.graph-node[data-step-path="1"] .graph-node-title')
    driver.chord('c')
    driver.chord('v')
    wait_for('canvas paste', lambda: driver.evaluate("document.querySelectorAll('.graph-node').length") == 3)
    driver.click('#undoBtn')
    wait_for('canvas paste undo', lambda: driver.state('source') == linked)
    previous_ids = {run['id'] for run in driver.invoke('list_runs', limit=60)}
    driver.click("#runBtn")
    wait_for("canvas result", lambda: "desktop-e2e-canvas" in driver.text("#outputBox")
             and driver.evaluate("!document.querySelector('#runBtn').disabled"))
    assert driver.invoke('read_flow_source', path=path) == linked
    run = next(item for item in driver.invoke('list_runs', limit=60)
               if item['id'] not in previous_ids and item['flowId'] == 'desktop-e2e')
    detail = driver.invoke('show_run', runId=run['id'])
    assert detail['run']['state'] == 'ok' and len(detail['steps']) == 2, detail
    assert all(step['state'] == 'ok' for step in detail['steps']), detail
    assert 'desktop-e2e-canvas' in json.dumps(detail['steps'][1]), detail
    driver.click('.graph-node[data-step-path="1"] .graph-node-title')
    wait_for('canvas node result inspection', lambda: 'desktop-e2e-canvas' in driver.text('#timelineDetail [data-result-preview]'))
    driver.click('#viewSwitch [data-mode="code"]')
    driver.fill("#codeEditor", restore_source)
    driver.click("#saveFlowBtn")
    wait_for("fixture restored after canvas acceptance", lambda: driver.invoke('read_flow_source', path=path) == restore_source)


def companion_acceptance(driver, path, source):
    # The unique saved flow alias routes locally through the production router.
    # No model, microphone, fake IPC, or supplied plan is used for this request.
    driver.click("#voiceToggleBtn")
    driver.switch_to("[data-companion]")
    if driver.evaluate("document.querySelector('[data-capsule-panel]').hidden"):
        driver.click('[data-voice-action="panel"]')
    previous = driver.invoke('capsule_status').get('activeRunId')
    driver.fill('[data-capsule-input]', 'desktop-e2e')
    driver.click('[data-capsule-submit]')
    def completed():
        status = driver.invoke('capsule_status')
        return status if status.get('activeRunId') != previous and status.get('runState') == 'completed' else None
    status = wait_for('companion text task completion', completed, timeout=60)
    assert status['activeRunId'] and not status['listening'] and not status['error'], status
    wait_for('companion result display', lambda: bool(driver.text('[data-capsule-result]').strip()))
    assert status['result'].strip(), status
    assert any(event['kind'] == 'run.completed' for event in driver.invoke('agent_events', runId=status['activeRunId'], afterSeq=0))
    driver.click('[data-voice-action="expand"]')
    driver.switch_to('#newFlowBtn')
    wait_for('companion run selected in Mission Control', lambda:
             driver.evaluate("document.querySelector('[data-mc-run-select]')?.value") == status['activeRunId'])
    # A cancellable local sleep gives the native event transport time to expose
    # running/progress before stopping, then another submission must still work.
    driver.click('.tabs [data-view="design"]')
    slow = source.replace('  steps:\n', '  steps:\n    - id: pause\n      action: control.sleep\n      with: {ms: 30000}\n')
    driver.fill('#codeEditor', slow)
    driver.click('#saveFlowBtn')
    wait_for('saved cancellable companion fixture', lambda: driver.invoke('read_flow_source', path=path) == slow)
    driver.click('#voiceToggleBtn')
    driver.switch_to('[data-companion]')
    driver.fill('[data-capsule-input]', 'desktop-e2e')
    driver.click('[data-capsule-submit]')
    def running():
        snapshot = driver.invoke('capsule_status')
        return snapshot if snapshot.get('activeRunId') != status['activeRunId'] and snapshot.get('runState') == 'running' else None
    active = wait_for('companion running progress', running)
    assert driver.evaluate("document.querySelector('[data-capsule-progress]').getAttribute('aria-valuenow')") is not None
    wait_for('enabled companion stop', lambda: driver.evaluate("!document.querySelector('[data-voice-action=stop-task]').disabled"))
    driver.click('[data-voice-action="stop-task"]')
    wait_for('companion cancellation', lambda: driver.invoke('capsule_status').get('runState') == 'cancelled')
    driver.switch_to('#newFlowBtn')
    driver.fill('#codeEditor', source)
    driver.click('#saveFlowBtn')
    wait_for('restored companion fixture', lambda: driver.invoke('read_flow_source', path=path) == source)
    driver.click('#voiceToggleBtn')
    driver.switch_to('[data-companion]')
    driver.fill('[data-capsule-input]', 'desktop-e2e')
    driver.click('[data-capsule-submit]')
    def retried():
        snapshot = driver.invoke('capsule_status')
        return snapshot if snapshot.get('activeRunId') != active['activeRunId'] and snapshot.get('runState') == 'completed' else None
    wait_for('companion retry after cancellation', retried)
    driver.switch_to('#newFlowBtn')


def acquisition_acceptance(driver):
    driver.click('.tabs [data-view="acquisition"]')
    driver.click('[data-acq-action="new"]')
    form = '[data-acq-form]'
    for field, value in [
        ('name', '获客验收'), ('product', '门店预约'),
        ('targetAudience', '寻找预约系统的店主'), ('keywords', '门店预约'),
        ('maxLeads', '7'),
    ]:
        driver.fill(f'{form} [name="{field}"]', value)
    driver.click(f'{form} [type="submit"]')
    wait_for('saved acquisition campaign', lambda: driver.text('[data-acq-summary] h2') == '获客验收')
    current = driver.invoke('acquisition_snapshot', campaignId=None)['campaigns'][0]
    assert current['name'] == '获客验收' and current['maxLeads'] == 7, current
    assert set(current['platforms']) == {'xiaohongshu', 'douyin', 'weibo'}, current
    auto = '[data-acq-run="outreach"]'

    def disabled():
        return driver.evaluate("document.querySelector(arguments[0]).disabled", auto)

    assert disabled() and current['outreach']['approvedAt'] is None
    driver.click('[data-acq-action="review-approval"]')
    review = driver.text('[data-acq-dialog-content]')
    for text in ['小红书', '抖音', '微博', '最多 7 位', '寻找预约系统的店主',
                 current['outreach']['messageTemplate']]:
        assert text in review, review
    driver.click('[data-acq-action="confirm-approval"]')
    wait_for('outreach enabled after approval', lambda: not disabled())
    # Empty isolated campaign: exercise the real worker and audit trail without
    # starting a platform browser, calling a model, or contacting an account.
    driver.click(auto)
    def snapshot():
        return driver.invoke('acquisition_snapshot', campaignId=current['id'])
    wait_for('worker completion', lambda: snapshot()['campaigns'][0]['status'] == 'completed')
    wait_for('completed campaign displayed', lambda: '已完成' in driver.text('[data-acq-summary]'))
    driver.click('[data-acq-tab="activity"]')
    wait_for('outreach audit displayed', lambda: '本次发送 0 条' in driver.text('[data-acq-content]'))
    assert {'approval', 'start', 'outreach', 'completed'} <= {a['kind'] for a in snapshot()['activity']}

    driver.click('[data-acq-action="edit"]')
    driver.fill(f'{form} [name="targetAudience"]', '需要到店提醒的店主')
    driver.click(f'{form} [type="submit"]')
    wait_for('scope edit revokes approval', disabled)
    updated = snapshot()['campaigns'][0]
    assert updated['targetAudience'] == '需要到店提醒的店主' and updated['outreach']['approvedAt'] is None
    driver.click('[data-acq-tab="demands"]')
    assert '需求池为空' in driver.text('[data-acq-content]')
    driver.click('.tabs [data-view="design"]')
    print('PASS: three-platform campaign, scope approval, empty outreach, audit and revocation')


def acceptance(driver):
    wait_for("Studio startup", lambda: driver.text("#statusText") == "就绪", timeout=60)
    acquisition_acceptance(driver)
    previous_path = driver.state("flowPath")
    driver.click("#newFlowBtn")
    wait_for("a new user flow", lambda: driver.state("flowPath")
             and driver.state("flowPath") != previous_path)
    driver.click('#viewSwitch [data-mode="code"]')
    source = flow_source("desktop-e2e-saved")
    driver.fill("#codeEditor", source)
    driver.click("#saveFlowBtn")
    path = driver.state("flowPath")
    wait_for("source saved to disk", lambda: driver.invoke("read_flow_source", path=path) == source)
    result = run_editor(driver, source, "desktop-e2e-saved desktop")
    assert result["run"]["state"] == "ok", result
    assert result["steps"][0]["state"] == "ok", result

    # An edit followed directly by Run must execute the new buffer on disk.
    source = flow_source("desktop-e2e-autosaved")
    result = run_editor(driver, source, "desktop-e2e-autosaved desktop")
    assert result["run"]["state"] == "ok", result
    assert driver.invoke("read_flow_source", path=path) == source
    driver.click('.tabs [data-view="runs"]')
    wait_for("persisted history row", lambda: driver.element(f'[data-run-id="{result["run"]["id"]}"]'))
    driver.click(f'#runsList [data-run-id="{result["run"]["id"]}"]')
    wait_for("history result detail", lambda: "desktop-e2e-autosaved desktop" in driver.text("#runDetail"))

    # VM failure must leave a usable UI result and a failed persisted step.
    driver.click('.tabs [data-view="design"]')
    failed = run_editor(driver, flow_source("desktop-e2e-expected-error", fail=True), "desktop-e2e-expected-error")
    assert failed["run"]["state"] == "failed", failed
    assert failed["steps"][0]["state"] == "failed", failed
    driver.fill("#codeEditor", source)
    driver.click("#saveFlowBtn")
    wait_for("restored executable fixture", lambda: driver.invoke("read_flow_source", path=path) == source)
    canvas_acceptance(driver, path, source)
    schedule_acceptance(driver, path)

    # Supplied local plan avoids a remote model while retaining production
    # validation, executor, event transport, approval IPC and persistence.
    driver.click('.tabs [data-view="mission-control"]')
    approved = start_agent(driver, "approved-node", risk="L3")
    approve_selector = f'[data-agent-action="approve"][data-run-id="{approved}"]'
    wait_for("L3 confirmation card", lambda: driver.element(approve_selector))
    try:
        driver.click(approve_selector)
    except WebDriverError as error:
        if "unexpected alert open" not in str(error):
            raise
    wait_for("native strengthened confirmation", lambda: driver.command("GET", "/alert/text"))
    driver.command("POST", "/alert/accept", {})
    events = wait_for("approved local execution", lambda: terminal_events(driver, approved))
    assert any(event["kind"] == "permission.resolved" and event["payload"].get("approved")
               and event["payload"].get("strengthened") for event in events), events
    wait_for("completed approval UI", lambda: "COMPLETED" in driver.text("[data-mc-metrics]"))
    assert not driver.evaluate("Boolean(document.querySelector('[data-agent-action=approve]'))")

    latest = start_agent(driver, "latest-node")
    wait_for("next local execution", lambda: terminal_events(driver, latest))
    wait_for("new run displayed live", lambda: driver.element('[data-node-id="latest-node"]'))
    wait_for("refreshed run history", lambda: driver.element(f'[data-mc-run-select] option[value="{approved}"]'))
    driver.click(f'[data-mc-run-select] option[value="{approved}"]')
    wait_for("pinned historical topology", lambda: driver.element('[data-node-id="approved-node"]'))
    driver.click('[data-mc-run-select] option[value=""]')
    wait_for("return to newest topology", lambda: driver.element('[data-node-id="latest-node"]'))
    assert driver.evaluate("document.querySelector('[data-mc-run-select]').value") == ""
    companion_acceptance(driver, path, source)
    print("PASS: editor save/run/autosave, canvas palette/drag/undo/connect/rename/copy/run/results, failure results, history, scheduling/edit conflict, L3 approval, live/history switching, companion text/progress/result/details/cancel/retry")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--application", type=Path, default=Path("target/debug/lumorpa-desktop"))
    parser.add_argument("--driver", default="tauri-driver")
    parser.add_argument("--artifacts", type=Path, default=Path("/tmp/lumo-desktop-e2e-artifacts"))
    args = parser.parse_args()
    if sys.platform != "linux":
        parser.error("Tauri WebDriver acceptance requires Linux + WebKitWebDriver; macOS is unsupported")
    application = args.application.resolve()
    if not application.is_file():
        parser.error(f"desktop binary missing: {application}; build with cargo build -p lumorpa-desktop --locked")
    executable = shutil.which(args.driver)
    if not executable or not shutil.which("WebKitWebDriver"):
        parser.error("install tauri-driver and WebKitWebDriver (Ubuntu: webkit2gtk-driver)")
    artifacts = args.artifacts.resolve()
    artifacts.mkdir(parents=True, exist_ok=True)
    with socket.socket() as listener:
        listener.bind(("127.0.0.1", 0))
        port = listener.getsockname()[1]
    driver = Driver(f"http://127.0.0.1:{port}")
    with tempfile.TemporaryDirectory(prefix="lumo-desktop-e2e-") as directory:
        environment = os.environ.copy()
        for key, name in [("XDG_DATA_HOME", "data"), ("XDG_CONFIG_HOME", "config"), ("XDG_CACHE_HOME", "cache")]:
            isolated = Path(directory, name)
            isolated.mkdir()
            environment[key] = str(isolated)
        with (artifacts / "driver.log").open("w") as log:
            process = subprocess.Popen([executable, "--port", str(port)], env=environment,
                                       stdout=log, stderr=subprocess.STDOUT)
            try:
                wait_for("tauri-driver", lambda: driver.request("GET", "/status").get("ready"), timeout=20)
                driver.start(application)
                acceptance(driver)
            except Exception:
                driver.capture(artifacts)
                raise
            finally:
                driver.close()
                process.terminate()
                try:
                    process.wait(timeout=10)
                except subprocess.TimeoutExpired:
                    process.kill()
                    process.wait(timeout=5)


if __name__ == "__main__":
    run_acceptance(main, "Tauri native acceptance")
