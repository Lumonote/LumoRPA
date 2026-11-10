import assert from "node:assert/strict";
import test from "node:test";

globalThis.window = {};
globalThis.localStorage = { getItem: () => null };
const { state } = await import("../src/js/state.js");
const { snapshotSource, resetHistory, undoEdit, redoEdit } = await import("../src/js/editor/history.js");
const { insertGraphStepAfter, resolveSelectionAfterEdit } = await import("../src/js/editor/graph-editing.js");
const { buildGraphModel } = await import("../src/js/editor/graph-model.js");
const { extractSteps, findStepByPath, parseYaml } = await import("../src/js/yaml.js");

const source = `spec:
  steps:
    - {id: first, action: control.log}
    - id: gate
      action: control.if
      do:
        - {id: shared, action: control.log}
      else:
        - {id: shared, action: control.log}
`;
const flowPath = "/flows/history.yaml";
const initialLayouts = {
  [flowPath]: { "id:first": { x: -240, y: 80 }, "id:gate/else/id:shared": { x: 120, y: 600 } },
  "/flows/other.yaml": { "id:untouched": { x: 77, y: 88 } },
};

// Exercise the actual history/render/inspector modules in code view. The fake
// exposes only their DOM boundary; source, selection and model logic stay real.
function mount(context, selectedPath = null) {
  const previous = { document: globalThis.document, localStorage: globalThis.localStorage };
  const storage = new Map();
  context.after(() => {
    globalThis.document = previous.document;
    globalThis.localStorage = previous.localStorage;
  });
  globalThis.localStorage = { getItem: (key) => storage.get(key) ?? null, setItem: (key, value) => storage.set(key, String(value)) };
  const element = () => ({
    value: "", textContent: "", innerHTML: "", style: {}, hidden: false, disabled: false,
    classList: { toggle() {} }, addEventListener() {},
    querySelectorAll: () => [],
  });
  const ids = new Map(["codeEditor", "codeGutter", "saveFlowBtn", "statusText", "undoBtn", "redoBtn", "inspectorBody", "propForm", "schemaFields", "applyStepBtn", "runThisStepBtn", "toggleBpBtn"].map((id) => [id, element()]));
  const fields = new Map();
  ids.get("propForm").querySelector = (selector) => {
    if (!fields.has(selector)) fields.set(selector, { ...element(), checked: false, value: selector === '[data-prop="action"]' ? "control.log" : "" });
    return fields.get(selector);
  };
  globalThis.document = { getElementById: (id) => ids.get(id) || null };
  state.source = source;
  state.savedSource = source;
  state.ast = parseYaml(source);
  state.flowPath = flowPath;
  state.viewMode = "code";
  state.graphLayouts = globalThis.structuredClone(initialLayouts);
  state.selectedStepPath = selectedPath;
  state.selectedStepId = selectedPath ? findStepByPath(extractSteps(state.ast), selectedPath)?.id : null;
  state.selectedStepPaths = new Set();
  state.schemaCache = new Map([["control.log", {}]]);
  let captures = 0;
  state.draftController = { capture() { captures++; } };
  resetHistory();
  return { ids, storage, captures: () => captures };
}

const positions = () => buildGraphModel(extractSteps(state.ast), { layouts: state.graphLayouts[flowPath] }).nodes;

test("layout-only movement and arrange undo/redo retain identical executable source and persisted coordinates", async (context) => {
  const ui = mount(context);
  const moved = { x: 310, y: -90 };
  snapshotSource("graph-layout");
  state.graphLayouts[flowPath]["id:first"] = moved;
  assert.deepEqual({ x: positions().get("0").x, y: positions().get("0").y }, moved);
  snapshotSource("graph-arrange");
  state.graphLayouts[flowPath] = {};
  assert.equal(positions().get("0").x, 0);

  assert.equal(undoEdit(), true);
  assert.equal(state.source, source);
  assert.deepEqual(state.graphLayouts[flowPath]["id:first"], moved);
  assert.equal(ui.ids.get("codeEditor").value, source);
  assert.equal(undoEdit(), true);
  assert.deepEqual(state.graphLayouts, initialLayouts);
  assert.equal(state.source, source);
  assert.deepEqual(JSON.parse(ui.storage.get("lumo.graphLayouts")), initialLayouts);
  assert.equal(ui.ids.get("undoBtn").disabled, true);

  assert.equal(redoEdit(), true);
  assert.deepEqual(state.graphLayouts[flowPath]["id:first"], moved);
  assert.equal(redoEdit(), true);
  assert.deepEqual(state.graphLayouts[flowPath], {});
  assert.deepEqual(state.graphLayouts["/flows/other.yaml"], initialLayouts["/flows/other.yaml"]);
  assert.equal(state.source, source);
  assert.equal(ui.ids.get("redoBtn").disabled, true);
  assert.equal(ui.captures(), 4);
  await Promise.resolve();
});

test("source insertion and layout coordinates undo together while branch selection follows its stable identity", async (context) => {
  const oldPath = [1, "else", 0];
  const ui = mount(context, oldPath);
  const previousAst = state.ast;
  const inserted = insertGraphStepAfter(state.source, [0], "control.log");
  snapshotSource("graph-insert");
  state.source = inserted.source;
  state.ast = parseYaml(inserted.source);
  state.selectedStepPath = resolveSelectionAfterEdit(previousAst, state.ast, oldPath);
  state.graphLayouts[flowPath][`id:${inserted.id}`] = { x: 510, y: 620 };
  state.graphLayouts[flowPath]["id:gate/else/id:shared"] = { x: 130, y: 610 };
  state.selectedStepPaths.add("1/:else/0");
  const editedSource = state.source;
  const editedLayouts = globalThis.structuredClone(state.graphLayouts);
  assert.deepEqual(state.selectedStepPath, [2, "else", 0]);

  assert.equal(undoEdit(), true);
  assert.equal(state.source, source);
  assert.deepEqual(state.graphLayouts, initialLayouts);
  assert.deepEqual(state.selectedStepPath, oldPath);
  assert.equal(state.selectedStepId, "shared");
  assert.equal(positions().get("1/:else/0").layoutKey, "id:gate/else/id:shared");
  assert.equal(positions().get("1/:do/0").layoutKey, "id:gate/do/id:shared");
  assert.equal(state.selectedStepPaths.size, 0);

  assert.equal(redoEdit(), true);
  assert.equal(state.source, editedSource);
  assert.deepEqual(state.graphLayouts, editedLayouts);
  assert.deepEqual(state.selectedStepPath, [2, "else", 0]);
  assert.equal(state.selectedStepId, "shared");
  assert.equal(positions().get("1").step.id, inserted.id);
  assert.equal(positions().get("2/:else/0").x, 130);
  assert.deepEqual(JSON.parse(ui.storage.get("lumo.graphLayouts")), editedLayouts);
  await Promise.resolve();
});

test("history snapshots own deep layout copies across repeated restore and subsequent mutations", async (context) => {
  mount(context);
  snapshotSource("graph-layout");
  state.graphLayouts[flowPath]["id:first"].x = 500;
  assert.equal(undoEdit(), true);
  assert.equal(state.graphLayouts[flowPath]["id:first"].x, -240);
  state.graphLayouts[flowPath]["id:first"].x = -999;
  assert.equal(redoEdit(), true);
  assert.equal(state.graphLayouts[flowPath]["id:first"].x, 500);
  assert.equal(undoEdit(), true);
  assert.equal(state.graphLayouts[flowPath]["id:first"].x, -999);
  assert.equal(initialLayouts[flowPath]["id:first"].x, -240);
  await Promise.resolve();
});
