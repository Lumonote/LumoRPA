import test from "node:test";
import assert from "node:assert/strict";
import {
  applyGraphRunResponse, createGraphRunContext, finishGraphRunContext,
  getGraphRunState, recordGraphRunProgress,
} from "../src/js/editor/graph-run-state.js";

const source = "spec:\n  steps: []\n";
const context = () => createGraphRunContext({ flowPath: "/flows/current.yaml", source, runId: "current" });
const editor = (editorRunContext, extra = {}) => ({ flowPath: "/flows/current.yaml", source, editorRunContext, ...extra });
const progress = (path, type = "step_started", state = "ok") => ({ runId: "current", path, type, state });
const loopAst = { spec: { steps: [
  { id: "outer", action: "control.for", do: [{ id: "inner", action: "control.while", do: [{ id: "read" }] }] },
  { id: "legacy", action: "control.for_each", do: [{ id: "read" }] },
  { id: "loop", action: "control.for_each", do: [{ id: "read" }] },
] } };

test("canvas execution state is isolated by exact flow path and source snapshot", () => {
  const current = recordGraphRunProgress(context(), progress("log"));
  assert.equal(getGraphRunState({ idChain: "log" }, editor(current)).state, "running");
  assert.equal(getGraphRunState({ idChain: "log" }, editor(current, { flowPath: "/flows/other.yaml" })).state, "idle");
  assert.equal(getGraphRunState({ idChain: "log" }, editor(current, { source: source + "# edited" })).state, "idle");
  assert.equal(getGraphRunState({ idChain: "log" }, editor(null)).state, "idle");
});

test("background events and late progress after a final response cannot recolor nodes", () => {
  const initial = context();
  assert.equal(recordGraphRunProgress(initial, { ...progress("log"), runId: "background" }), initial);
  assert.equal(recordGraphRunProgress(initial, { runId: "current", type: "log", line: "text" }), initial);
  const completed = applyGraphRunResponse(initial, { report: { runId: "current", success: true }, steps: [{ path: "log", state: "ok" }] });
  assert.equal(recordGraphRunProgress(completed, progress("log")), completed);
  assert.equal(getGraphRunState({ idChain: "log" }, editor(completed)).state, "ok");
});

test("runtime branch scopes keep identical child IDs from sharing execution colors", () => {
  let current = recordGraphRunProgress(context(), progress("condition/do/read", "step_finished", "ok"));
  current = recordGraphRunProgress(current, progress("parallel/branch[0]/read", "step_finished", "failed"));
  assert.equal(getGraphRunState({ idChain: "condition/read", runtimePath: "condition/do/read" }, editor(current)).state, "ok");
  assert.equal(getGraphRunState({ idChain: "condition/read", runtimePath: "condition/else/read" }, editor(current)).state, "idle");
  assert.equal(getGraphRunState({ runtimePath: "parallel/branch[0]/read" }, editor(current)).state, "failed");
  assert.equal(getGraphRunState({ runtimePath: "parallel/branch[1]/read" }, editor(current)).state, "idle");
  assert.equal(getGraphRunState({ runtimePath: "another/read" }, editor(current)).state, "idle");
});

test("nested loop iterations aggregate without stripping semantic branch scopes", () => {
  let current = recordGraphRunProgress(context(), progress("outer[2]/inner[4]/read", "step_finished", "ok"));
  current = recordGraphRunProgress(current, progress("outer[3]/inner[0]/read", "step_finished", "failed"));
  current = recordGraphRunProgress(current, progress("legacy/item.2/read", "step_finished", "ok"));
  assert.equal(getGraphRunState({ runtimePath: "outer/inner/read" }, editor(current, { ast: loopAst })).state, "failed");
  assert.equal(getGraphRunState({ runtimePath: "outer/other/read" }, editor(current, { ast: loopAst })).state, "idle");
  assert.equal(getGraphRunState({ runtimePath: "legacy/read" }, editor(current, { ast: loopAst })).state, "ok");
  assert.equal(getGraphRunState({ runtimePath: "read" }, editor(current)).state, "idle");
});

test("literal indexed leaf IDs remain exact and are not treated as loop scopes", () => {
  const current = recordGraphRunProgress(context(), progress("task[0]", "step_finished", "ok"));
  assert.equal(getGraphRunState({ idChain: "task[0]" }, editor(current)).state, "ok");
  assert.equal(getGraphRunState({ idChain: "task" }, editor(current)).state, "idle");
});

test("literal indexed parent IDs never normalize as an undeclared loop", () => {
  const ast = { spec: { steps: [
    { id: "job", action: "control.if", do: [{ id: "read" }] },
    { id: "job[0]", action: "control.if", do: [{ id: "read" }] },
  ] } };
  let current = recordGraphRunProgress(context(), progress("job[0]/do/read", "step_finished", "ok"));
  current = recordGraphRunProgress(current, progress("job/item.2/do/read", "step_finished", "failed"));
  assert.equal(getGraphRunState({ runtimePath: "job/do/read" }, editor(current, { ast })).state, "idle");
  assert.equal(getGraphRunState({ runtimePath: "job[0]/do/read" }, editor(current, { ast })).state, "ok");
});

test("a final response requires the captured run identity and preserves loop durations", () => {
  const initial = context();
  assert.equal(applyGraphRunResponse(initial, { report: { runId: "other", success: true }, steps: [{ path: "read", state: "ok" }] }), initial);
  assert.equal(applyGraphRunResponse(initial, { report: { runId: "current", success: true }, run: { id: "other" } }), initial);
  const completed = applyGraphRunResponse(initial, {
    report: { runId: "current", success: true }, run: { id: "current", state: "ok" },
    steps: [{ path: "loop[0]/read", state: "ai_healed", durationMs: 12 }, { path: "loop[1]/read", state: "ok", durationMs: 18 }],
  });
  assert.deepEqual(getGraphRunState({ runtimePath: "loop/read" }, editor(completed, { ast: loopAst })), { state: "ok", label: "已完成", durationMs: 30 });
});

test("failed and cancelled exits terminate running ancestors without erasing completed steps", () => {
  let current = recordGraphRunProgress(context(), progress("loop"));
  current = recordGraphRunProgress(current, progress("loop[0]/read", "step_finished", "ok"));
  const failed = finishGraphRunContext(current, "failed");
  assert.equal(getGraphRunState({ runtimePath: "loop" }, editor(failed)).state, "failed");
  assert.equal(getGraphRunState({ runtimePath: "loop/read" }, editor(failed, { ast: loopAst })).state, "ok");
  const cancelled = applyGraphRunResponse(current, { run: { id: "current", state: "cancelled" }, report: { runId: "current", success: false }, steps: [] });
  assert.equal(getGraphRunState({ runtimePath: "loop" }, editor(cancelled)).state, "cancelled");
});

test("paused debug responses clear running colors and invalid durations stay absent", () => {
  const current = recordGraphRunProgress(context(), progress("loop"));
  const paused = applyGraphRunResponse(current, { report: { runId: "current", success: false, pausedAt: "loop/read" }, steps: [{ path: "log", state: "skipped", durationMs: -1 }] });
  assert.deepEqual(getGraphRunState({ runtimePath: "loop" }, editor(paused)), { state: "idle", label: "待执行" });
  assert.deepEqual(getGraphRunState({ runtimePath: "log" }, editor(paused)), { state: "skipped", label: "已跳过" });
});

// Verify production run wiring with a small DOM boundary, rather than testing
// only a reducer that the application might never call.
class Element {
  value = "{}"; textContent = ""; innerHTML = ""; style = {}; dataset = {};
  classList = { toggle() {}, add() {}, remove() {} };
  querySelectorAll() { return []; }
  addEventListener() {}
  appendChild() {}
  remove() {}
}
const elements = new Map();
globalThis.document = {
  getElementById(id) { if (!elements.has(id)) elements.set(id, new Element()); return elements.get(id); },
  querySelectorAll: () => [], createElement: () => new Element(),
};
globalThis.localStorage = { getItem: () => null };
let invoke;
globalThis.window = { __TAURI__: { core: { invoke: (...args) => invoke(...args) } } };
const { state } = await import("../src/js/state.js");
const { setRunInFlight, updateRunProgress, applyRunResponse, showRunFailure } = await import("../src/js/runs.js");
const reset = () => {
  Object.assign(state, { flowPath: "/flows/current.yaml", source, editorRunContext: null, runInFlight: false, liveRunProgress: null });
  elements.clear();
};

test("production editor run captures a new context and projects matching live/final steps", async () => {
  reset();
  setRunInFlight(true);
  const runId = state.inFlightRunId;
  assert.equal(state.editorRunContext.runId, runId);
  updateRunProgress({ ...progress("log"), runId });
  assert.equal(getGraphRunState({ idChain: "log" }, state).state, "running");
  updateRunProgress({ ...progress("log", "step_finished", "failed"), runId: "background" });
  assert.equal(getGraphRunState({ idChain: "log" }, state).state, "running");
  invoke = async () => [];
  await applyRunResponse({ run: { id: runId, state: "ok" }, report: { runId, success: true, outputs: {}, durationMs: 3, stepsTotal: 1, stepsOk: 1, stepsFailed: 0 }, steps: [{ path: "log", state: "ok", durationMs: 3 }] });
  setRunInFlight(false);
  assert.deepEqual(getGraphRunState({ idChain: "log" }, state), { state: "ok", label: "已完成", durationMs: 3 });
  state.source += "# edit";
  assert.equal(getGraphRunState({ idChain: "log" }, state).state, "idle");
});

test("production IPC failure clears live running nodes when no persisted trace exists", async () => {
  reset();
  setRunInFlight(true);
  updateRunProgress({ ...progress("log"), runId: state.inFlightRunId });
  invoke = async () => { throw new Error("trace unavailable"); };
  await showRunFailure(new Error("execution failed"));
  setRunInFlight(false);
  assert.equal(getGraphRunState({ idChain: "log" }, state).state, "failed");
});
