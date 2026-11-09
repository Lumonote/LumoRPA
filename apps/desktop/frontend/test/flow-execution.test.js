import test from "node:test";
import assert from "node:assert/strict";

// Exercise the real editor/run modules across the IPC boundary. The host below
// represents disk + Tauri; rendering only needs these small DOM surfaces.
class Element {
  value = "";
  textContent = "";
  innerHTML = "";
  style = {};
  dataset = {};
  classList = { toggle() {}, add() {}, remove() {} };
  querySelectorAll() { return []; }
  addEventListener() {}
  appendChild() {}
  remove() {}
}
const elements = new Map();
globalThis.document = {
  getElementById(id) {
    if (!elements.has(id)) elements.set(id, new Element());
    return elements.get(id);
  },
  querySelectorAll: () => [],
  createElement: () => new Element(),
};
globalThis.localStorage = { getItem: () => null };
let invoke;
globalThis.window = { __TAURI__: { core: { invoke: (...args) => invoke(...args) } } };
const { state } = await import("../src/js/state.js");
const { ensureCurrentFlowSaved } = await import("../src/js/flows.js");
const { runSelectedFlow, runStep, applyRunResponse, cancelActiveRun, updateRunProgress, setRunInFlight } = await import("../src/js/runs.js");
const { startDebug } = await import("../src/js/debug.js");

const source = (message) => `apiVersion: lumorpa.io/v1
kind: Flow
metadata: { id: edit-run }
spec:
  steps:
    - { id: log, action: control.log, with: { message: ${message} } }
`;
const response = (runId, outputs = { log: { result: "edited" } }) => ({
  run: { id: runId, state: "ok" }, steps: [],
  report: { runId, success: true, outputs, durationMs: 12, stepsOk: 1, stepsTotal: 1, stepsFailed: 0 },
});
const deferred = () => {
  let resolve;
  const promise = new Promise((done) => { resolve = done; });
  return { promise, resolve };
};
function reset() {
  elements.clear();
  Object.assign(state, {
    flowPath: "/flows/edit.yaml", source: source("edited"), savedSource: source("old"),
    flow: { id: "edit-run" }, viewMode: "code", ast: null, runInFlight: false,
    inFlightRunId: null, liveRunProgress: null, activeRun: null, activeRunSteps: [],
    activeArtifacts: [], selectedStepPath: null, breakpoints: new Set(), debugRunId: null,
  });
  globalThis.document.getElementById("inputsJson").value = '{"message":"captured"}';
}

test("save acknowledgement preserves edits made while disk write is pending", async () => {
  reset();
  const write = deferred();
  const saved = state.source;
  invoke = async (cmd) => { assert.equal(cmd, "save_flow_source"); await write.promise; };
  const saving = ensureCurrentFlowSaved();
  state.source = source("newer");
  write.resolve();
  await saving;
  assert.equal(state.savedSource, saved);
  assert.notEqual(state.source, state.savedSource, "new edits must remain unsaved");
});

test("editing a bundled example saves a user copy and executes that copy", async () => {
  reset();
  state.flow.source = "example";
  const calls = [];
  invoke = async (cmd, args) => {
    calls.push([cmd, args]);
    if (cmd === "duplicate_flow") return "/flows/example-copy.yaml";
    if (cmd === "save_flow_source") {
      assert.equal(args.path, "/flows/example-copy.yaml");
      assert.equal(args.source, source("edited"));
      return;
    }
    if (cmd === "list_flow_library") return [];
    if (cmd === "run_flow") {
      assert.equal(args.path, "/flows/example-copy.yaml");
      return response(args.runId);
    }
    if (cmd === "list_artifacts") return [];
    throw new Error(`unexpected command ${cmd}`);
  };
  await runSelectedFlow();
  assert.equal(state.flowPath, "/flows/example-copy.yaml");
  assert.equal(state.flow.source, "user");
  assert.deepEqual(calls.slice(0, 2).map(([cmd]) => cmd), ["duplicate_flow", "save_flow_source"]);
  assert.match(globalThis.document.getElementById("outputBox").textContent, /edited/);
});

test("cancelling while the editor saves prevents execution", async () => {
  reset();
  const write = deferred();
  let runs = 0;
  invoke = async (cmd, args) => {
    if (cmd === "save_flow_source") return write.promise;
    if (cmd === "cancel_run") return { ok: false };
    if (cmd === "run_flow") { runs++; return response(args.runId); }
    if (cmd === "list_artifacts") return [];
    throw new Error(`unexpected command ${cmd}`);
  };
  const running = runSelectedFlow();
  await cancelActiveRun();
  write.resolve();
  await running;
  assert.equal(runs, 0);
  assert.equal(state.runInFlight, false);
});

for (const [command, run] of [["run_flow", runSelectedFlow], ["run_step", () => runStep("log")], ["debug_flow", startDebug]]) {
  test(`${command} saves the edited snapshot before execution and retains its target`, async () => {
    reset();
    const write = deferred();
    const calls = [];
    const edited = state.source;
    let disk;
    invoke = async (cmd, args) => {
      calls.push([cmd, args]);
      if (cmd === "save_flow_source") { await write.promise; disk = args.source; return; }
      if (cmd === command) {
        assert.equal(disk, edited);
        assert.equal(args.path, "/flows/edit.yaml");
        assert.equal(args.inputsJson, '{"message":"captured"}');
        assert.match(args.runId, /^[0-7][0-9A-HJKMNP-TV-Z]{25}$/);
        return response(args.runId);
      }
      if (cmd === "list_artifacts") return [];
      throw new Error(`unexpected command ${cmd}`);
    };
    const running = run();
    state.flowPath = "/flows/other.yaml";
    state.source = state.savedSource = source("other");
    globalThis.document.getElementById("inputsJson").value = "{}";
    write.resolve();
    await running;
    assert.deepEqual(calls.slice(0, 2).map(([cmd]) => cmd), ["save_flow_source", command]);
    assert.equal(state.savedSource, source("other"), "saving the old document cannot mark the new one saved");
    assert.match(globalThis.document.getElementById("outputBox").textContent, /edited/);
    assert.equal(state.runInFlight, false);
  });
}

test("a failed execution shows its persisted step error and unlocks controls", async () => {
  reset();
  state.savedSource = state.source;
  let runId;
  invoke = async (cmd, args) => {
    if (cmd === "run_flow") { runId = args.runId; throw new Error("step log: deliberate failure"); }
    if (cmd === "show_run") {
      assert.equal(args.runId, runId);
      return {
        run: { id: runId, state: "failed", durationMs: 4, outputs: null },
        steps: [{ path: "log", stepId: "log", state: "failed", error: "deliberate failure" }],
      };
    }
    if (cmd === "list_artifacts") return [];
    throw new Error(`unexpected command ${cmd}`);
  };
  await runSelectedFlow();
  assert.match(globalThis.document.getElementById("outputBox").textContent, /deliberate failure/);
  assert.match(globalThis.document.getElementById("timelineDetail").innerHTML, /deliberate failure/);
  assert.equal(state.activeRun.id, runId);
  assert.equal(state.runInFlight, false);
  assert.equal(globalThis.document.getElementById("runBtn").disabled, false);
});

test("a second execution request cannot overlap the active editor run", async () => {
  reset();
  state.savedSource = state.source;
  const completion = deferred();
  let runs = 0;
  invoke = async (cmd, args) => {
    if (cmd === "run_flow") { runs++; await completion.promise; return response(args.runId); }
    if (cmd === "list_artifacts") return [];
    throw new Error(`unexpected command ${cmd}`);
  };
  const first = runSelectedFlow();
  await Promise.resolve();
  const second = runSelectedFlow();
  completion.resolve();
  await Promise.all([first, second]);
  assert.equal(runs, 1);
});

test("late artifact replies cannot replace the newer run's artifacts", async () => {
  reset();
  const oldArtifacts = deferred();
  invoke = async (cmd, { runId }) => {
    assert.equal(cmd, "list_artifacts");
    return runId === "old" ? oldArtifacts.promise : [{ id: "new-artifact" }];
  };
  const old = applyRunResponse(response("old"));
  await applyRunResponse(response("new"));
  oldArtifacts.resolve([{ id: "old-artifact" }]);
  await old;
  assert.equal(state.activeRun.id, "new");
  assert.deepEqual(state.activeArtifacts, [{ id: "new-artifact" }]);
});

test("progress arriving while final artifacts load cannot replace final output", async () => {
  reset();
  state.savedSource = state.source;
  const artifacts = deferred();
  let id;
  let loading;
  const started = new Promise((resolve) => { loading = resolve; });
  invoke = async (cmd, args) => {
    if (cmd === "run_flow") { id = args.runId; return response(id); }
    if (cmd === "list_artifacts") { loading(); return artifacts.promise; }
    throw new Error(`unexpected command ${cmd}`);
  };
  const running = runSelectedFlow();
  await started;
  updateRunProgress({ runId: id, type: "log", line: "late log" });
  artifacts.resolve([]);
  await running;
  assert.match(globalThis.document.getElementById("outputBox").textContent, /edited/);
});

test("cancelling a known editor run never falls back to a background run", async () => {
  reset();
  state.savedSource = state.source;
  const completion = deferred();
  const started = deferred();
  let id;
  const cancelled = [];
  invoke = async (cmd, args) => {
    if (cmd === "run_flow") { id = args.runId; started.resolve(); await completion.promise; return response(id); }
    if (cmd === "list_runs") return [{ id: "background-run", state: "running" }];
    if (cmd === "cancel_run") { cancelled.push(args.runId); return { ok: false }; }
    if (cmd === "list_artifacts") return [];
    throw new Error(`unexpected command ${cmd}`);
  };
  const running = runSelectedFlow();
  await started.promise;
  await cancelActiveRun();
  completion.resolve();
  await running;
  assert.deepEqual(cancelled, [id]);
});

test("background and delayed progress cannot overwrite the editor result", () => {
  reset();
  setRunInFlight(true);
  state.inFlightRunId = "editor";
  updateRunProgress({ runId: "background", type: "log", line: "unrelated" });
  assert.equal(state.liveRunProgress, null);
  updateRunProgress({ runId: "editor", type: "log", line: "current" });
  assert.equal(globalThis.document.getElementById("outputBox").textContent, "current");
  state.runInFlight = false;
  globalThis.document.getElementById("outputBox").textContent = "final outputs";
  updateRunProgress({ runId: "editor", type: "log", line: "late event" });
  assert.equal(globalThis.document.getElementById("outputBox").textContent, "final outputs");
});
