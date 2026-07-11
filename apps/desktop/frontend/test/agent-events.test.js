import test from "node:test";
import assert from "node:assert/strict";
import { createRunProjection, applyAgentEvent, readyTopology } from "../src/js/agent-events.js";

test("projects serial and parallel plan events deterministically", () => {
  let p = createRunProjection("run-1");
  p = applyAgentEvent(p, { seq: 1, kind: "plan.created", payload: { nodes: [
    { id: "intent", capabilityId: "system:intent", dependsOn: [] },
    { id: "orders", capabilityId: "mcp:erp/orders", dependsOn: ["intent"] },
    { id: "summary", capabilityId: "ai:summary", dependsOn: ["intent"] },
    { id: "export", capabilityId: "flow:export", dependsOn: ["orders", "summary"] },
  ] } });
  p = applyAgentEvent(p, { seq: 2, kind: "node.started", nodeId: "orders", payload: { progress: 0.2 } });
  const topology = readyTopology(p);
  assert.deepEqual(topology.lanes.map((lane) => lane.map((node) => node.id)), [["intent"], ["orders", "summary"], ["export"]]);
  assert.equal(topology.activeNodeId, "orders");
  assert.equal(topology.edges.length, 4);
});

test("ignores duplicate and out-of-order events", () => {
  let p = createRunProjection("run-2");
  p = applyAgentEvent(p, { seq: 3, kind: "run.paused", payload: {} });
  const same = applyAgentEvent(p, { seq: 2, kind: "run.completed", payload: {} });
  assert.equal(same.seq, 3);
  assert.equal(same.status, "paused");
});

test("tracks retries replans and terminal node state", () => {
  let p = createRunProjection("run-3");
  p = applyAgentEvent(p, { seq: 1, kind: "plan.created", payload: { nodes: [{ id: "a", capabilityId: "mcp:x/a", dependsOn: [] }] } });
  p = applyAgentEvent(p, { seq: 2, kind: "node.failed", nodeId: "a", payload: { error: "timeout" } });
  p = applyAgentEvent(p, { seq: 3, kind: "plan.revised", payload: { reason: "retry", nodes: [{ id: "a", capabilityId: "mcp:x/a", dependsOn: [], attempt: 2 }] } });
  p = applyAgentEvent(p, { seq: 4, kind: "node.completed", nodeId: "a", payload: { result: "ok" } });
  assert.equal(p.nodes.a.status, "completed");
  assert.equal(p.nodes.a.attempt, 2);
  assert.equal(p.replans.length, 1);
});
