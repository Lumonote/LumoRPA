import test from "node:test";
import assert from "node:assert/strict";

import { renderTopology, summarizeTopology, virtualizeTopology } from "../src/js/mission-control.js";

function largeTopology() {
  const lanes = Array.from({ length: 100 }, (_, lane) => Array.from({ length: 10 }, (_, node) => ({
    id: `n-${lane}-${node}`,
    capabilityId: `skill:batch-${lane}-${node}`,
    status: lane === 77 && node === 4 ? "running" : "queued",
    attempt: 1,
  })));
  return { lanes, edges: [], activeNodeId: "n-77-4" };
}

test("virtualizes one thousand nodes while preserving the active lane", () => {
  const view = virtualizeTopology(largeTopology(), { startLane: 0, laneCount: 8, maxNodes: 120 });
  assert.ok(view.lanes.length <= 9);
  assert.ok(view.lanes.flat().some((node) => node.id === "n-77-4"));
  assert.equal(view.totalNodes, 1000);
  assert.ok(view.hiddenNodes > 0);
});

test("renders minimap zoom controls and keyboard navigable nodes", () => {
  const html = renderTopology(virtualizeTopology(largeTopology(), { startLane: 70, laneCount: 12 }));
  assert.match(html, /data-mc-zoom="in"/);
  assert.match(html, /data-mc-minimap/);
  assert.match(html, /tabindex="0"/);
  assert.match(html, /aria-setsize="1000"/);
});

test("summarizes serial and parallel execution accessibly", () => {
  assert.equal(summarizeTopology({ lanes: [[{ id: "a" }], [{ id: "b" }, { id: "c" }]], edges: [] }), "2 stages, 3 nodes, 1 parallel branch");
});
