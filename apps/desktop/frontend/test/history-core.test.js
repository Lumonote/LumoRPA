import test from "node:test";
import assert from "node:assert/strict";

import { createHistoryCore } from "../src/js/editor/history-core.js";

test("push/undo/redo round-trips snapshots", () => {
  const h = createHistoryCore();
  assert.equal(h.canUndo(), false);
  h.push("v1");
  assert.equal(h.canUndo(), true);
  assert.equal(h.undo("v2"), "v1");
  assert.equal(h.canUndo(), false);
  assert.equal(h.canRedo(), true);
  assert.equal(h.redo("v1"), "v2");
  assert.equal(h.canRedo(), false);
  assert.equal(h.canUndo(), true);
});

test("undo/redo on empty stacks return null", () => {
  const h = createHistoryCore();
  assert.equal(h.undo("cur"), null);
  assert.equal(h.redo("cur"), null);
});

test("a new push clears the redo stack", () => {
  const h = createHistoryCore();
  h.push("v1");
  assert.equal(h.undo("v2"), "v1");
  assert.equal(h.canRedo(), true);
  h.push("v1-edited");
  assert.equal(h.canRedo(), false);
  assert.equal(h.redo("x"), null);
});

test("cap drops the oldest snapshot", () => {
  const h = createHistoryCore({ cap: 3 });
  h.push("a");
  h.push("b");
  h.push("c");
  h.push("d");
  assert.equal(h.undo("e"), "d");
  assert.equal(h.undo("d"), "c");
  assert.equal(h.undo("c"), "b");
  assert.equal(h.undo("b"), null);
});

test("coalesce keeps the earliest snapshot within the window", () => {
  let t = 0;
  const h = createHistoryCore({ now: () => t });
  h.push("s0", { label: "code", coalesceMs: 1000 });
  t = 500;
  h.push("s1", { label: "code", coalesceMs: 1000 });
  assert.equal(h.size(), 1);
  assert.equal(h.undo("s2"), "s0");
});

test("coalesce window expires and starts a new entry", () => {
  let t = 0;
  const h = createHistoryCore({ now: () => t });
  h.push("s0", { label: "code", coalesceMs: 1000 });
  t = 500;
  h.push("s1", { label: "code", coalesceMs: 1000 });
  t = 2000;
  h.push("s2", { label: "code", coalesceMs: 1000 });
  assert.equal(h.size(), 2);
  assert.equal(h.undo("s3"), "s2");
  assert.equal(h.undo("s2"), "s0");
});

test("coalesce only merges consecutive pushes with the same label", () => {
  let t = 0;
  const h = createHistoryCore({ now: () => t });
  h.push("a", { label: "code", coalesceMs: 1000 });
  t = 100;
  h.push("b", { label: "move" });
  assert.equal(h.size(), 2);
  t = 200;
  h.push("c", { label: "code", coalesceMs: 1000 });
  assert.equal(h.size(), 3);
});

test("reset clears both stacks", () => {
  const h = createHistoryCore();
  h.push("v1");
  h.undo("v2");
  h.reset();
  assert.equal(h.canUndo(), false);
  assert.equal(h.canRedo(), false);
  assert.equal(h.size(), 0);
});
