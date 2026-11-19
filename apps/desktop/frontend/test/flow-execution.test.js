import test from "node:test";
import assert from "node:assert/strict";
const { EventTarget, Event } = globalThis;

// Exercise the real editor/run modules across the IPC boundary. The host below
// represents disk + Tauri; rendering only needs these small DOM surfaces.
class Element extends EventTarget {
  value = "";
  textContent = "";
  _innerHTML = "";
  get innerHTML() { return this._innerHTML; }
  set innerHTML(value) { this._innerHTML = value; this.timelineSource = null; this.children = new Map(); this.prepended = null; }
  style = {};
  dataset = {};
  classList = { toggle() {}, add() {}, remove() {} };
  querySelectorAll(selector) {
    if (selector !== ".timeline-step") return [];
    if (this.timelineSource !== this.innerHTML) {
      this.timelineSource = this.innerHTML;
      this.timelineSteps = [...this.innerHTML.matchAll(/data-idx="(\d+)"/g)].map((match) => {
        const element = new Element(); element.dataset.idx = match[1]; return element;
      });
    }
    return this.timelineSteps || [];
  }
  querySelector(selector) {
    this.children ||= new Map();
    if (!this.children.has(selector)) this.children.set(selector, new Element());
    return this.children.get(selector);
  }
  appendChild() {}
  prepend(child) { this.prepended = child; }
  remove() {}
}
const elements = new Map();
globalThis.document = Object.assign(new EventTarget(), {
  getElementById(id) {
    if (!elements.has(id)) elements.set(id, new Element());
    return elements.get(id);
  },
  querySelectorAll: () => [],
  createElement: () => new Element(),
});
globalThis.CustomEvent ||= class CustomEvent extends Event {
  constructor(type, options = {}) { super(type); this.detail = options.detail; }
};
const { document, CustomEvent } = globalThis;
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

test("scalar JSON values are visible in both timeline and historical results", async () => {
  const { showHistoricalRun } = await import("../src/js/runs.js");
  for (const value of [false, 0, "", null, {}, []]) {
    reset();
    const step = { seq: 1, path: "scalar", state: "ok", outputJson: value, hasOutput: true };
    invoke = async (cmd) => {
      if (cmd === "show_run") return { run: { id: "scalar", flowId: "scalar" }, steps: [step] };
      return [];
    };
    await applyRunResponse({ ...response("scalar"), steps: [step] });
    assert.match(globalThis.document.getElementById("timelineDetail").innerHTML, /data-result-preview/);
    await showHistoricalRun("scalar");
    assert.match(globalThis.document.getElementById("histStepDetail").innerHTML, /data-result-preview/);
  }
});

test("historical selection stays consistent when older cost responses finish late", async () => {
  const { showHistoricalRun } = await import("../src/js/runs.js");
  reset();
  const aCosts = deferred();
  const requested = deferred();
  invoke = async (cmd, { runId }) => {
    if (cmd === "show_run") return { run: { id: runId, flowId: `flow-${runId}` }, steps: [{ seq: 1, path: `step-${runId}`, state: "ok", outputJson: runId }] };
    if (cmd === "run_cost" && runId === "A") { requested.resolve(); return aCosts.promise; }
    return [];
  };
  const older = showHistoricalRun("A");
  await requested.promise;
  await showHistoricalRun("B");
  aCosts.resolve([]);
  await older;
  assert.equal(state.activeRun.id, "B");
  assert.match(globalThis.document.getElementById("runDetail").innerHTML, /flow-B/);
  assert.doesNotMatch(globalThis.document.getElementById("runDetail").innerHTML, /flow-A/);
  assert.match(globalThis.document.getElementById("histStepDetail").innerHTML, /step-B/);
});

test("node result selection keeps loop instances and source identity isolated", async () => {
  const { selectNodeRunResult } = await import("../src/js/runs.js");
  reset();
  state.ast = { spec: { steps: [{ id: "loop", action: "control.for_each", do: [{ id: "read" }] }] } };
  setRunInFlight(true);
  const runId = state.inFlightRunId;
  const reads = [];
  const steps = [0, 1].map((index) => ({ seq: index, path: `loop[${index}]/read`, state: "ok", detailsDeferred: true }));
  invoke = async (cmd, args) => {
    if (cmd === "read_run_step") { reads.push(args.seq); return { ...steps[args.seq], detailsDeferred: false, outputJson: args.seq, hasOutput: true }; }
    return [];
  };
  await applyRunResponse({ ...response(runId), steps });
  assert.equal(selectNodeRunResult([0, "do", 0]), true);
  await new Promise((resolve) => globalThis.setTimeout(resolve, 0));
  assert.equal(state.activeStepRun.path, "loop[1]/read");
  assert.match(globalThis.document.getElementById("timelineDetail").innerHTML, /data-result-preview/);
  assert.ok(reads.includes(1));
  state.source += "# changed";
  assert.equal(selectNodeRunResult([0, "do", 0]), false);
});

test("history and step pages request explicit cursors without full payload calls", async () => {
  const { refreshRuns, showHistoricalRun } = await import("../src/js/runs.js");
  reset();
  const calls = [];
  invoke = async (cmd, args) => {
    calls.push([cmd, args]);
    if (cmd === "list_runs") return [{ id: "page" }];
    if (cmd === "show_run") return { run: { id: "page" }, steps: [{ seq: 200, path: "step", state: "ok", detailsDeferred: true }] };
    if (cmd === "list_run_steps") return [{ seq: 201, path: "next", state: "ok", detailsDeferred: true }];
    if (cmd === "read_run_step") return { seq: args.seq, path: "loaded", outputJson: false, hasOutput: true };
    return [];
  };
  await refreshRuns({ offset: 60 });
  await showHistoricalRun("page");
  await showHistoricalRun("page", { afterSeq: 200, previous: [null] });
  assert.deepEqual(calls.find(([cmd]) => cmd === "list_runs")[1], { limit: 60, offset: 60 });
  assert.equal(calls.find(([cmd]) => cmd === "show_run")[1].summary, true);
  assert.deepEqual(calls.find(([cmd]) => cmd === "list_run_steps")[1], { runId: "page", afterSeq: 200, limit: 200 });
  assert.ok(calls.filter(([cmd]) => cmd === "read_run_step").length === 2);
  assert.ok(!calls.some(([cmd]) => cmd === "read_run_outputs"));
});

test("large table preview renders a bounded number of rows and columns", async () => {
  reset();
  invoke = async () => [];
  const rows = Array.from({ length: 10000 }, (_, index) => ({ id: index, message: "x".repeat(1000) }));
  await applyRunResponse({ ...response("table"), steps: [{ seq: 1, path: "table", state: "ok", outputJson: rows }] });
  const detail = globalThis.document.getElementById("timelineDetail").innerHTML;
  assert.match(detail, /data-result-table/);
  assert.equal((detail.match(/<tr>/g) || []).length, 21);
  assert.match(detail, /data-result-export/);
  assert.ok(detail.length < 30000);
});

test("nested single-step IPC includes exact selection path and maps its response", async () => {
  reset();
  state.savedSource = state.source;
  state.ast = { spec: { steps: [{ id: "gate", action: "control.if", do: [{ id: "child", action: "control.log" }], else: [{ id: "child", action: "control.log" }] }] } };
  state.selectedStepPath = [0, "else", 0];
  let request;
  invoke = async (cmd, args) => {
    if (cmd === "run_step") { request = args; return { ...response(args.runId), steps: [{ path: "child", state: "ok", outputJson: false }] }; }
    return [];
  };
  await runStep("child");
  assert.deepEqual(request.stepPath, [0, "else", 0]);
  assert.equal(state.editorRunContext.selectedRuntimePath, "gate/else/child");
  const { getGraphRunState } = await import("../src/js/editor/graph-run-state.js");
  assert.equal(getGraphRunState({ runtimePath: "gate/else/child" }, state).state, "ok");
  assert.equal(getGraphRunState({ runtimePath: "gate/do/child" }, state).state, "idle");
});

test("late per-step payload replies cannot replace the newer selected result", async () => {
  const { selectNodeRunResult } = await import("../src/js/runs.js");
  reset();
  state.ast = { spec: { steps: [{ id: "first" }, { id: "second" }] } };
  setRunInFlight(true);
  const runId = state.inFlightRunId;
  const older = deferred();
  const requested = deferred();
  invoke = async (cmd, args) => {
    if (cmd === "read_run_step" && args.seq === 1) { requested.resolve(); return older.promise; }
    if (cmd === "read_run_step") return { seq: 2, path: "second", outputJson: "newer", hasOutput: true };
    return [];
  };
  await applyRunResponse({ ...response(runId), steps: [{ seq: 1, path: "first", detailsDeferred: true }, { seq: 2, path: "second", detailsDeferred: true }] });
  await requested.promise;
  selectNodeRunResult([1]);
  await new Promise((resolve) => globalThis.setTimeout(resolve, 0));
  older.resolve({ seq: 1, path: "first", outputJson: "older", hasOutput: true });
  await new Promise((resolve) => globalThis.setTimeout(resolve, 0));
  assert.match(globalThis.document.getElementById("timelineDetail").innerHTML, /newer/);
  assert.doesNotMatch(globalThis.document.getElementById("timelineDetail").innerHTML, /older/);
  assert.match(globalThis.document.getElementById("graphResultDetail").innerHTML, /newer/);
});

test("result focus round trip preserves the selected loop instance", async () => {
  const { selectNodeRunResult } = await import("../src/js/runs.js");
  const { selectStep } = await import("../src/js/editor/inspector.js");
  reset();
  state.ast = { spec: { steps: [{ id: "loop", action: "control.for_each", do: [{ id: "read" }] }] } };
  state.selectedStepPath = [0, "do", 0];
  setRunInFlight(true);
  const runId = state.inFlightRunId;
  invoke = async () => [];
  const focused = [];
  const roundTrip = (event) => { focused.push(event.detail.path); selectStep([0, "do", 0]); };
  document.addEventListener("lumo:focus-node", roundTrip);
  try {
    await applyRunResponse({ ...response(runId), steps: [0, 1].map((index) => ({ seq: index, path: `loop[${index}]/read`, state: "ok", outputJson: `instance-${index}` })) });
    assert.equal(selectNodeRunResult([0, "do", 0]), true);
    assert.equal(state.activeStepRun.path, "loop[1]/read");
    document.getElementById("timelineTrack").querySelectorAll(".timeline-step")[0].dispatchEvent(new Event("click"));
    await Promise.resolve();
    assert.deepEqual(focused, ["loop/read"]);
    assert.equal(state.activeStepRun.path, "loop[0]/read");
    assert.match(document.getElementById("timelineDetail").innerHTML, /instance-0/);
    assert.doesNotMatch(document.getElementById("timelineDetail").innerHTML, /instance-1/);
  } finally { document.removeEventListener("lumo:focus-node", roundTrip); }
});

test("loop instance pages retain every result and navigate using exact global row indices", async () => {
  const { selectNodeRunResult } = await import("../src/js/runs.js");
  reset();
  state.ast = { spec: { steps: [{ id: "loop", action: "control.for_each", do: [{ id: "read" }, { id: "save" }] }] } };
  setRunInFlight(true);
  const runId = state.inFlightRunId;
  invoke = async () => [];
  const steps = Array.from({ length: 1001 }, (_, index) => ["read", "save"].map((id, offset) => ({ seq: index * 2 + offset + 1, path: `loop[${index}]/${id}`, state: "ok", outputJson: `${id}-${index}` }))).flat();
  await applyRunResponse({ ...response(runId), steps });
  assert.equal(selectNodeRunResult([0, "do", 0]), true);
  const box = document.getElementById("graphResultDetail");
  assert.equal(state.activeRunSteps, steps);
  assert.equal(state.activeRunSteps.length, 2002);
  assert.equal(state.activeStepRun.path, "loop[1000]/read");
  assert.match(box.prepended.innerHTML, /运行实例 1001–1001 \/ 1001/);
  assert.equal((box.prepended.innerHTML.match(/<option /g) || []).length, 1);
  box.prepended.querySelector("[data-instance-prev]").dispatchEvent(new Event("click"));
  assert.equal(state.activeStepRun.path, "loop[800]/read");
  assert.match(box.prepended.innerHTML, /运行实例 801–1000 \/ 1001/);
  assert.equal((box.prepended.innerHTML.match(/<option /g) || []).length, 200);
  const select = box.prepended.querySelector("select"); select.value = String(805 * 2);
  select.dispatchEvent(new Event("change"));
  assert.equal(state.activeStepRun.path, "loop[805]/read");
  assert.match(box.innerHTML, /read-805/);
  assert.match(box.prepended.innerHTML, /value="1610" selected/);
  box.prepended.querySelector("[data-instance-next]").dispatchEvent(new Event("click"));
  assert.equal(state.activeStepRun.path, "loop[1000]/read");
});

test("selecting an unrun node invalidates an older deferred result", async () => {
  reset();
  state.ast = { spec: { steps: [{ id: "old" }, { id: "unrun" }] } };
  setRunInFlight(true);
  const runId = state.inFlightRunId;
  const older = deferred();
  invoke = async (cmd) => cmd === "read_run_step" ? older.promise : [];
  await applyRunResponse({ ...response(runId), steps: [{ seq: 1, path: "old", detailsDeferred: true }] });
  document.dispatchEvent(new CustomEvent("lumo:step-selected", { detail: { flowPath: state.flowPath, path: [1] } }));
  older.resolve({ seq: 1, path: "old", outputJson: "stale-payload", hasOutput: true });
  await new Promise((resolve) => globalThis.setTimeout(resolve, 0));
  assert.equal(state.activeStepRun, null);
  assert.match(document.getElementById("graphResultDetail").innerHTML, /没有运行结果/);
  assert.doesNotMatch(document.getElementById("graphResultDetail").innerHTML, /stale-payload/);
});

test("closing the result drawer cancels its pending payload", async () => {
  const { selectNodeRunResult } = await import("../src/js/runs.js");
  reset();
  state.ast = { spec: { steps: [{ id: "old" }] } };
  setRunInFlight(true);
  const runId = state.inFlightRunId;
  const older = deferred();
  invoke = async (cmd) => cmd === "read_run_step" ? older.promise : [];
  await applyRunResponse({ ...response(runId), steps: [{ seq: 1, path: "old", detailsDeferred: true }] });
  selectNodeRunResult([0]);
  document.getElementById("graphResultDrawer").hidden = true;
  document.getElementById("graphResultDetail").innerHTML = "closed";
  document.dispatchEvent(new CustomEvent("lumo:graph-result-closed"));
  older.resolve({ seq: 1, path: "old", outputJson: "stale-payload", hasOutput: true });
  await new Promise((resolve) => globalThis.setTimeout(resolve, 0));
  assert.equal(document.getElementById("graphResultDrawer").hidden, true);
  assert.equal(document.getElementById("graphResultDetail").innerHTML, "closed");
});

test("an empty history page retains navigation to the previous page", async () => {
  const { refreshRuns } = await import("../src/js/runs.js");
  reset();
  const offsets = [];
  invoke = async (_cmd, { offset }) => { offsets.push(offset); return []; };
  await refreshRuns({ offset: 60 });
  const list = document.getElementById("runsList");
  assert.match(list.innerHTML, /data-runs-prev >上一页/);
  list.onclick({ target: { closest: (selector) => selector === "[data-runs-prev]" ? {} : null } });
  await Promise.resolve();
  assert.deepEqual(offsets, [60, 0]);
});

test("AI cost rows use bounded cursor pages and preserve previous navigation", async () => {
  const { showHistoricalRun } = await import("../src/js/runs.js");
  reset();
  const costs = Array.from({ length: 25000 }, (_, index) => ({ id: index + 1, step_id: `cost-${index}`, provider: "p", model: "m", cost_usd_micro: 1 }));
  const requests = [];
  invoke = async (cmd, args) => {
    if (cmd === "show_run") return { run: { id: "costs" }, steps: [] };
    if (cmd === "run_cost") { requests.push(args); return args.afterId ? [] : costs; }
    return [];
  };
  await showHistoricalRun("costs");
  const panel = document.getElementById("runCostDetail");
  assert.equal((panel.innerHTML.match(/<tr/g) || []).length, 201);
  assert.ok(panel.innerHTML.length < 120000);
  assert.deepEqual(requests[0], { runId: "costs", afterId: null, limit: 200 });
  panel.querySelector("[data-cost-next]").dispatchEvent(new Event("click"));
  await new Promise((resolve) => globalThis.setTimeout(resolve, 0));
  assert.deepEqual(requests[1], { runId: "costs", afterId: 200, limit: 200 });
  assert.match(panel.innerHTML, /data-cost-prev >上一页/);
  panel.querySelector("[data-cost-prev]").dispatchEvent(new Event("click"));
  await new Promise((resolve) => globalThis.setTimeout(resolve, 0));
  assert.deepEqual(requests[2], { runId: "costs", afterId: null, limit: 200 });
});

test("history defaults do not focus graph and async output retains its button", async () => {
  const { showHistoricalRun } = await import("../src/js/runs.js");
  reset();
  state.ast = { spec: { steps: [{ id: "read" }] } };
  setRunInFlight(true);
  const runId = state.inFlightRunId;
  const outputs = deferred();
  invoke = async (cmd) => {
    if (cmd === "show_run") return { run: { id: runId }, steps: [{ seq: 1, path: "read", outputJson: false }] };
    if (cmd === "read_run_outputs") return outputs.promise;
    return [];
  };
  let focuses = 0;
  const onFocus = () => { focuses++; };
  document.addEventListener("lumo:focus-node", onFocus);
  try {
    await showHistoricalRun(runId);
    assert.equal(focuses, 0);
    const button = document.getElementById("runDetail").querySelector("[data-load-run-output]");
    button.dispatchEvent(new Event("click"));
    outputs.resolve(false);
    await new Promise((resolve) => globalThis.setTimeout(resolve, 0));
    assert.equal(button.textContent, "已加载输出预览");
    assert.equal(document.getElementById("runDetail").querySelector("[data-run-output]").textContent, "false");
  } finally { document.removeEventListener("lumo:focus-node", onFocus); }
});
