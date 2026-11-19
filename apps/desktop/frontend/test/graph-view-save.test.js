import test from "node:test";
import assert from "node:assert/strict";

// Exercise debounced viewport persistence against the real layout store. Disk
// responses are isolated by document, as they are in the desktop IPC boundary.
const storage = new Map(), disk = new Map(), calls = [];
globalThis.localStorage = {
  getItem: (key) => storage.get(key) ?? null,
  setItem: (key, value) => storage.set(key, String(value)),
  removeItem: (key) => storage.delete(key),
};
globalThis.window = { __TAURI__: { core: { invoke: async (command, args) => {
  calls.push({ command, path: args.path });
  if (command === "graph_layout_read") return { writable: true, layout: globalThis.structuredClone(disk.get(args.path) || { version: 1, positions: {} }) };
  if (command === "graph_layout_write") { disk.set(args.path, globalThis.structuredClone(args.layout)); return; }
  throw new Error(command);
} } } };
globalThis.document = { getElementById: () => ({ appendChild() {} }), createElement: () => ({ style: {}, remove() {} }) };

const { state } = await import("../src/js/state.js");
const { loadGraphLayout, persistGraphLayout, scheduleGraphLayoutSave, flushScheduledGraphLayouts } = await import("../src/js/editor/graph-layout-store.js");

function controlTimers(context) {
  const pending = new Map();
  let next = 0;
  context.mock.method(globalThis, "setTimeout", (callback) => { const id = ++next; pending.set(id, callback); return id; });
  context.mock.method(globalThis, "clearTimeout", (id) => pending.delete(id));
  return {
    async runAll() {
      for (const [id, callback] of [...pending]) {
        if (!pending.delete(id)) continue;
        await callback();
      }
    },
  };
}

async function openDocument(path, x = 10) {
  disk.set(path, { version: 1, positions: { "id:first": { x, y: 20 } } });
  state.graphLayouts[path] = await loadGraphLayout(path);
  return state.graphLayouts[path];
}

test("an explicit undo save supersedes the older viewport timer", async (context) => {
  const timers = controlTimers(context), path = "/flow/view-save-undo.yaml";
  const original = await openDocument(path);
  const before = globalThis.structuredClone(original);
  scheduleGraphLayoutSave(path);
  original["id:first"].x = 900;

  // History replaces the layout object when undoing in code/steps mode. No
  // graph render follows to replace a timer that captured the pre-undo object.
  state.graphLayouts[path] = before;
  await persistGraphLayout(path, before);
  await timers.runAll();
  await flushScheduledGraphLayouts(path);

  assert.equal(state.graphLayouts[path]["id:first"].x, 10);
  assert.equal(disk.get(path).positions["id:first"].x, 10);
  assert.equal(JSON.parse(storage.get(`lumo.graphLayout.v1:${path}`))["id:first"].x, 10);
});

test("a viewport timer captures the latest document layout at flush time", async (context) => {
  const timers = controlTimers(context), path = "/flow/view-save-replaced.yaml";
  const original = await openDocument(path);
  scheduleGraphLayoutSave(path);
  state.graphLayouts[path] = { "id:first": { x: 45, y: -30 } };
  original["id:first"].x = 999;
  await timers.runAll();
  await flushScheduledGraphLayouts(path);
  assert.deepEqual(disk.get(path).positions["id:first"], { x: 45, y: -30 });
});

test("reopening a flow flushes its scheduled layout before reading the sidecar", async (context) => {
  const timers = controlTimers(context), path = "/flow/view-save-reopen.yaml";
  const layout = await openDocument(path);
  layout["id:first"].x = -400;
  scheduleGraphLayoutSave(path);
  const start = calls.length;

  const reloaded = await loadGraphLayout(path);

  assert.equal(reloaded["id:first"].x, -400);
  assert.deepEqual(calls.slice(start).map(({ command }) => command), ["graph_layout_write", "graph_layout_read"]);
  await timers.runAll();
  assert.equal(calls.slice(start).filter(({ command }) => command === "graph_layout_write").length, 1);
});

test("flushing viewport saves preserves document ownership across a flow switch", async (context) => {
  const timers = controlTimers(context), first = "/flow/view-save-first.yaml", second = "/flow/view-save-second.yaml";
  await openDocument(first);
  await openDocument(second, 70);
  state.flowPath = first;
  state.graphLayouts[first]["id:first"].x = 30;
  scheduleGraphLayoutSave(first);
  state.flowPath = second;
  state.graphLayouts[second]["id:first"].x = 80;
  scheduleGraphLayoutSave(second);

  await flushScheduledGraphLayouts(first);
  assert.equal(disk.get(first).positions["id:first"].x, 30);
  assert.equal(disk.get(second).positions["id:first"].x, 70);
  await flushScheduledGraphLayouts();
  assert.equal(disk.get(first).positions["id:first"].x, 30);
  assert.equal(disk.get(second).positions["id:first"].x, 80);
  const writesBeforeTimers = calls.filter(({ command }) => command === "graph_layout_write").length;
  await timers.runAll();
  assert.equal(calls.filter(({ command }) => command === "graph_layout_write").length, writesBeforeTimers);
});
