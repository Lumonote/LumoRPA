import test from "node:test";
import assert from "node:assert/strict";

const storage = new Map(), writes = [];
let disk = { version: 1, positions: { "id:disk": { x: 1, y: 2 } } };
let failWrite = false;
globalThis.localStorage = { getItem: (key) => storage.get(key) ?? null, setItem: (key, value) => storage.set(key, value), removeItem: (key) => storage.delete(key) };
globalThis.window = { __TAURI__: { core: { invoke: async (command, args) => {
  if (command === "graph_layout_read") return { writable: true, layout: disk };
  if (command === "graph_layout_write") { if (failWrite) throw new Error("disk full"); writes.push(args); disk = args.layout; return; }
  throw new Error(command);
} } } };
const notices = [];
globalThis.document = { getElementById: () => ({ appendChild: (node) => notices.push(node) }), createElement: () => ({ style: {}, remove() {} }) };
const { loadGraphLayout, persistGraphLayout, copyGraphLayout } = await import("../src/js/editor/graph-layout-store.js");
const { state } = await import("../src/js/state.js");

test("sidecar layouts load across machines, save captured snapshots in order, and copy with flow", async () => {
  const path = "/flow/a.yaml";
  assert.deepEqual(await loadGraphLayout(path), disk.positions);
  const first = { "id:a": { x: 3, y: 4 } };
  const saving = persistGraphLayout(path, first);
  first["id:a"].x = 900;
  const last = persistGraphLayout(path, { "id:a": { x: 8, y: 9 } });
  await Promise.all([saving, last]);
  assert.equal(writes.at(-2).layout.positions["id:a"].x, 3);
  assert.equal(writes.at(-1).layout.positions["id:a"].x, 8);
  state.graphLayouts[path] = first;
  await copyGraphLayout(path, "/flow/b.yaml");
  assert.equal(writes.at(-1).path, "/flow/b.yaml");
  assert.equal(writes.at(-1).layout.positions["id:a"].x, 900);
});

test("failed disk writes preserve local edits on reload and clear pending only after retry", async (context) => {
  // Toast timers are irrelevant to persistence; avoid delaying the Node test process.
  context.mock.method(globalThis, "setTimeout", () => 0);
  const path = "/flow/failure.yaml";
  await loadGraphLayout(path);
  failWrite = true;
  await assert.rejects(persistGraphLayout(path, { "id:new": { x: -20, y: 3 } }), /disk full/);
  const recovered = await loadGraphLayout(path);
  assert.deepEqual(recovered, { "id:new": { x: -20, y: 3 } });
  assert.ok(notices.length);
  failWrite = false;
  await persistGraphLayout(path, recovered);
  assert.equal(storage.get(`lumo.graphLayout.v1:${path}:pending`), undefined);
});

test("layout loading sanitizes edited sidecars and unsaved local cache before use", async () => {
  const previousDisk = disk;
  try {
    disk = { version: 1, positions: { "id:a": { x: "invalid", y: 3, collapsed: true }, __studio: { version: 1, groups: [null], routes: { bad: { x: 3, y: null } }, view: { tx: 0, ty: 0, scale: 0 } } } };
    const loaded = await loadGraphLayout("/flow/malformed-disk.yaml");
    assert.deepEqual(loaded, { "id:a": { collapsed: true }, __studio: { version: 1, groups: [], routes: {}, snap: true } });
    const path = "/flow/malformed-local.yaml";
    storage.set(`lumo.graphLayout.v1:${path}`, JSON.stringify({ "id:local": { x: -10, y: 20 }, __studio: { version: 1, groups: {}, routes: [], view: { tx: 0, ty: 0, scale: -1 } } }));
    storage.set(`lumo.graphLayout.v1:${path}:pending`, "true");
    assert.deepEqual(await loadGraphLayout(path), { "id:local": { x: -10, y: 20 }, __studio: { version: 1, groups: [], routes: {}, snap: true } });
    disk = { version: 1, positions: [] };
    assert.deepEqual(await loadGraphLayout("/flow/malformed-root.yaml"), {});
  } finally { disk = previousDisk; }
});
