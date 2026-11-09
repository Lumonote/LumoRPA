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
const { ensureCurrentFlowSaved, loadFlow } = await import("../src/js/flows.js");
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
    debugPausedAt: null, examples: [],
  });
  globalThis.document.getElementById("inputsJson").value = '{"message":"captured"}';
}

const summary = (path, extra = {}) => ({ path, id: path, fileName: path.split("/").pop(), valid: true, inputs: [], stepCount: 1, ...extra });

test("rapid flow switching keeps the latest document when older reads finish late", async () => {
  reset();
  state.savedSource = state.source;
  const older = deferred();
  invoke = async (cmd, { path }) => {
    if (cmd === "inspect_flow") return summary(path);
    if (cmd === "read_flow_source") return path === "/flows/older.yaml" ? older.promise : source("latest");
    throw new Error(`unexpected command ${cmd}`);
  };
  const loadingOld = loadFlow("/flows/older.yaml");
  await loadFlow("/flows/latest.yaml");
  older.resolve(source("older"));
  await loadingOld;
  assert.equal(state.flowPath, "/flows/latest.yaml");
  assert.equal(state.flow.id, "/flows/latest.yaml");
  assert.equal(state.source, source("latest"));
});

test("failed source reads preserve the current editor instead of loading an error comment", async () => {
  reset();
  state.savedSource = state.source;
  const previousSource = state.source;
  invoke = async (cmd, { path }) => {
    if (cmd === "inspect_flow") return summary(path);
    if (cmd === "read_flow_source") throw new Error("file unavailable");
    throw new Error(`unexpected command ${cmd}`);
  };
  await loadFlow("/flows/unavailable.yaml");
  assert.equal(state.flowPath, "/flows/edit.yaml");
  assert.equal(state.source, previousSource);
  assert.equal(state.flow.id, "edit-run");
});

test("edits made while another document is loading are preserved", async () => {
  reset();
  state.savedSource = state.source;
  const read = deferred();
  invoke = async (cmd, { path }) => cmd === "inspect_flow" ? summary(path) : read.promise;
  const loading = loadFlow("/flows/other.yaml");
  state.source = source("new-edit");
  read.resolve(source("other"));
  await loading;
  assert.equal(state.flowPath, "/flows/edit.yaml");
  assert.equal(state.source, source("new-edit"));
  assert.notEqual(state.source, state.savedSource);
});

test("save-and-switch preserves edits made while the save is pending", async () => {
  reset();
  const write = deferred();
  const calls = [];
  globalThis.confirm = () => true;
  invoke = async (cmd, { path }) => {
    calls.push(cmd);
    if (cmd === "save_flow_source") return write.promise;
    if (cmd === "inspect_flow") return summary(path);
    if (cmd === "read_flow_source") return source("other");
    throw new Error(`unexpected command ${cmd}`);
  };
  try {
    const loading = loadFlow("/flows/other.yaml");
    state.source = source("new-edit");
    write.resolve();
    await loading;
    assert.equal(state.flowPath, "/flows/edit.yaml");
    assert.equal(state.source, source("new-edit"));
    assert.equal(state.savedSource, source("edited"));
    assert.deepEqual(calls, ["save_flow_source"]);
  } finally {
    delete globalThis.confirm;
  }
});

test("switching documents clears the previous flow's debug resume and breakpoints", async () => {
  reset();
  state.savedSource = state.source;
  state.debugRunId = "previous-run";
  state.debugPausedAt = "log";
  state.breakpoints.add("log");
  invoke = async (cmd, { path }) => cmd === "inspect_flow" ? summary(path) : source("other");
  await loadFlow("/flows/other.yaml");
  assert.equal(state.debugRunId, null);
  assert.equal(state.debugPausedAt, null);
  assert.equal(state.breakpoints.size, 0);
  assert.equal(globalThis.document.getElementById("debugBar").style.display, "none");
});

test("a malformed document remains editable without retaining the previous AST or inputs", async () => {
  reset();
  state.savedSource = state.source;
  state.ast = { previous: true };
  const invalid = "spec: [unterminated";
  invoke = async (cmd, { path }) => cmd === "inspect_flow"
    ? summary(path, { valid: false, error: "invalid YAML" }) : invalid;
  await loadFlow("/flows/invalid.yaml");
  assert.equal(state.source, invalid);
  assert.equal(state.ast, null);
  assert.equal(globalThis.document.getElementById("codeEditor").value, invalid);
  assert.equal(globalThis.document.getElementById("inputsJson").value, "{}");
});

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

test("overlapping saves write in request order before switching documents", async () => {
  reset();
  const writes = [deferred(), deferred()];
  let disk;
  let requests = 0;
  globalThis.confirm = () => true;
  invoke = async (cmd, args) => {
    if (cmd === "save_flow_source") {
      const pending = writes[requests++];
      await pending.promise;
      disk = args.source;
      return;
    }
    if (cmd === "inspect_flow") return summary(args.path);
    if (cmd === "read_flow_source") return source("other");
    throw new Error(`unexpected command ${cmd}`);
  };
  try {
    const first = ensureCurrentFlowSaved();
    state.source = source("new-edit");
    const switching = loadFlow("/flows/other.yaml");
    const inFlight = requests;
    writes[1].resolve();
    writes[0].resolve();
    await Promise.all([first, switching]);
    assert.equal(inFlight, 1, "a newer save must wait for the previous disk write");
    assert.equal(disk, source("new-edit"));
    assert.equal(state.flowPath, "/flows/other.yaml");
  } finally {
    delete globalThis.confirm;
  }
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

test("overlapping example saves reuse one user copy and retain the latest edit", async () => {
  reset();
  state.flow.source = "example";
  const duplicate = deferred();
  let copies = 0;
  const writes = [];
  invoke = async (cmd, args) => {
    if (cmd === "duplicate_flow") { copies++; return duplicate.promise; }
    if (cmd === "save_flow_source") { writes.push(args); return; }
    if (cmd === "list_flow_library") return [];
    throw new Error(`unexpected command ${cmd}`);
  };
  const first = ensureCurrentFlowSaved();
  state.source = source("new-edit");
  const second = ensureCurrentFlowSaved();
  duplicate.resolve("/flows/example-copy.yaml");
  await Promise.all([first, second]);
  assert.equal(copies, 1);
  assert.deepEqual(writes, [
    { path: "/flows/example-copy.yaml", source: source("edited") },
    { path: "/flows/example-copy.yaml", source: source("new-edit") },
  ]);
  assert.equal(state.flowPath, "/flows/example-copy.yaml");
  assert.equal(state.source, state.savedSource);
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
