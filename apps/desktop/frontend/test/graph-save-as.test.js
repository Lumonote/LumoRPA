import test from "node:test";
import assert from "node:assert/strict";

// Keep the real flow/history/layout modules; only replace their host boundary.
const storage = new Map();
globalThis.localStorage = {
  getItem: (key) => storage.get(key) ?? null,
  setItem: (key, value) => storage.set(key, String(value)),
  removeItem: (key) => storage.delete(key),
};
let invoke;
globalThis.window = { __TAURI__: { core: { invoke: (...args) => invoke(...args) } } };
const { state } = await import("../src/js/state.js");
const { saveCurrentFlowAs, ensureCurrentFlowSaved } = await import("../src/js/flows.js");
const { snapshotSource, resetHistory, undoEdit, redoEdit } = await import("../src/js/editor/history.js");
const { loadGraphLayout, persistGraphLayout } = await import("../src/js/editor/graph-layout-store.js");
const { parseYaml } = await import("../src/js/yaml.js");

const source = (message) => `spec:\n  steps:\n    - { id: log, action: control.log, with: { message: ${message} } }`;

async function mount(context, kind) {
  context.mock.method(globalThis, "setTimeout", () => 0);
  const elements = new Map();
  const element = () => ({
    value: "", textContent: "", innerHTML: "", style: {}, disabled: false,
    classList: { toggle() {} }, appendChild() {}, remove() {}, querySelectorAll: () => [],
  });
  globalThis.document = {
    getElementById: (id) => { if (!elements.has(id)) elements.set(id, element()); return elements.get(id); },
    createElement: element,
  };
  const from = `/flows/${kind}-original.yaml`, to = `/flows/${kind}-copy.yaml`;
  const layouts = new Map([[from, { "id:log": { x: -20, y: 30 } }]]);
  const writes = [], movedDrafts = [];
  invoke = async (command, args) => {
    if (command === "graph_layout_read") return { writable: true, layout: { version: 1, positions: globalThis.structuredClone(layouts.get(args.path) || {}) } };
    if (command === "graph_layout_write") {
      writes.push(globalThis.structuredClone(args));
      layouts.set(args.path, globalThis.structuredClone(args.layout.positions));
      return;
    }
    if (command === "save_flow_as") { assert.equal(args.source, source("edited")); return to; }
    if (command === "duplicate_flow") { assert.equal(args.path, from); return to; }
    if (command === "save_flow_source") { assert.equal(args.path, to); assert.equal(args.source, source("edited")); return; }
    if (command === "list_flow_library") return [];
    throw new Error(`unexpected command ${command}`);
  };
  Object.assign(state, {
    flowPath: from, flow: { id: kind, path: from, fileName: `${kind}-original.yaml`, source: kind === "example" ? "example" : "user", inputs: [] },
    source: source("original"), savedSource: source("original"), ast: parseYaml(source("original")),
    examples: [], viewMode: "code", graphLayouts: {}, breakpoints: new Set(),
    selectedStepPath: null, selectedStepId: null, selectedStepPaths: new Set(), orphanDraftPath: null,
    draftController: { move: async (path) => movedDrafts.push(path), capture() {} },
  });
  state.graphLayouts[from] = await loadGraphLayout(from);
  resetHistory();
  snapshotSource("original-flow-edit");
  state.source = source("edited");
  state.ast = parseYaml(state.source);
  state.graphLayouts[from]["id:log"] = { x: 400, y: -60 };
  await persistGraphLayout(from, state.graphLayouts[from]);
  writes.length = 0;
  assert.equal(elements.get("undoBtn").disabled, false, "the original document must have an undo entry");
  return { from, to, layouts, writes, movedDrafts, elements };
}

async function verifyCopyHistory(ui) {
  const { from, to, layouts, writes, movedDrafts, elements } = ui;
  const copied = { "id:log": { x: 400, y: -60 } };
  assert.equal(state.flowPath, to);
  assert.equal(state.flow.source, "user");
  assert.deepEqual(state.graphLayouts[from], copied);
  assert.deepEqual(state.graphLayouts[to], copied);
  assert.notEqual(state.graphLayouts[to], state.graphLayouts[from]);
  assert.deepEqual(movedDrafts, [from]);
  assert.equal(elements.get("undoBtn").disabled, true);
  assert.equal(undoEdit(), false, "undo from the new document cannot restore the original document's snapshot");
  assert.equal(redoEdit(), false);
  assert.equal(state.source, source("edited"));
  assert.deepEqual(layouts.get(from), copied);
  assert.ok(writes.length > 0, "the new document's layout must be persisted");
  assert.ok(writes.every(({ path }) => path === to), "copying and undo cannot write to the original sidecar");

  snapshotSource("new-document-layout");
  state.graphLayouts[to]["id:log"] = { x: 900, y: 800 };
  assert.equal(undoEdit(), true, "new edits must remain undoable after the copy");
  await loadGraphLayout(to); // Drain the history restore's sidecar write.
  assert.deepEqual(state.graphLayouts[to], copied);
  assert.deepEqual(state.graphLayouts[from], copied);
  assert.deepEqual(layouts.get(from), copied);
  assert.deepEqual(layouts.get(to), copied);
  assert.ok(writes.every(({ path }) => path === to));
}

test("save as copies the layout and starts history at the new document", async (context) => {
  const ui = await mount(context, "save-as");
  globalThis.prompt = () => "copy";
  context.after(() => { delete globalThis.prompt; });
  await saveCurrentFlowAs();
  await verifyCopyHistory(ui);
});

test("saving a bundled example starts the user copy's history without rewriting the example layout", async (context) => {
  const ui = await mount(context, "example");
  assert.equal(await ensureCurrentFlowSaved(), ui.to);
  await verifyCopyHistory(ui);
});
