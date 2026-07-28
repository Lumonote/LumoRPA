import assert from "node:assert/strict";
import test from "node:test";

import {
  applyLayoutOverrides,
  moveNodeByScreenDelta,
  screenToWorld,
  zoomGraphAt,
} from "../src/js/editor/graph-viewport.js";

test("screenToWorld maps viewport pixels into graph world coordinates", () => {
  assert.deepEqual(
    screenToWorld({ tx: 20, ty: 40, scale: 2 }, 120, 240),
    { x: 50, y: 100 },
  );
});

test("zoomGraphAt keeps the world point under the cursor fixed", () => {
  const start = { tx: 10, ty: 20, scale: 1 };
  const next = zoomGraphAt(start, 100, 120, 2, 0.25, 3.5);

  assert.deepEqual(next, { tx: -80, ty: -80, scale: 2 });
  assert.deepEqual(screenToWorld(start, 100, 120), screenToWorld(next, 100, 120));
});

test("zoomGraphAt clamps scale while preserving cursor focus", () => {
  const start = { tx: 0, ty: 0, scale: 1 };
  const next = zoomGraphAt(start, 50, 75, 10, 0.25, 3);

  assert.equal(next.scale, 3);
  assert.deepEqual(screenToWorld(start, 50, 75), screenToWorld(next, 50, 75));
});

test("moveNodeByScreenDelta converts drag pixels into world coordinates", () => {
  const next = moveNodeByScreenDelta({ x: 30, y: 80 }, { tx: 100, ty: 50, scale: 2 }, 40, -20);

  assert.deepEqual(next, { x: 50, y: 70 });
});

test("applyLayoutOverrides keeps auto layout unless a node has a saved position", () => {
  const positions = new Map([
    ["0", { x: 0, y: 0, w: 200 }],
    ["1", { x: 0, y: 88, w: 200 }],
  ]);

  applyLayoutOverrides(positions, { 1: { x: 320, y: -40 } });

  assert.deepEqual(positions.get("0"), { x: 0, y: 0, w: 200 });
  assert.deepEqual(positions.get("1"), { x: 320, y: -40, w: 200 });
});
