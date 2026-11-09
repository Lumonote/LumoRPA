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

test("pending approvals survive log truncation and clear only for the resolved node and plan", () => {
  let p = createRunProjection("run-approval");
  let seq = 0;
  for (const nodeId of ["a", "b"]) {
    p = applyAgentEvent(p, { seq: ++seq, kind: "permission.requested", node_id: nodeId, payload: { planHash: "hash", nodeIds: [nodeId], risk: "L2" } });
  }
  for (let i = 0; i < 310; i++) p = applyAgentEvent(p, { seq: ++seq, kind: "node.progress", nodeId: "c", payload: {} });
  assert.equal(p.pendingApprovals.length, 2);
  p = applyAgentEvent(p, { seq: ++seq, kind: "permission.resolved", payload: { planHash: "old", nodeIds: ["a"], approved: true } });
  assert.equal(p.pendingApprovals.length, 2);
  p = applyAgentEvent(p, { seq: ++seq, kind: "permission.resolved", payload: { planHash: "hash", nodeIds: ["a"], approved: true } });
  assert.deepEqual(p.pendingApprovals.map((event) => event.node_id), ["b"]);
  assert.equal(p.status, "waiting");
  p = applyAgentEvent(p, { seq: ++seq, kind: "run.cancelled", payload: {} });
  assert.deepEqual(p.pendingApprovals, []);
});

test("a paused run stays paused when an approval is resolved", () => {
  let p = createRunProjection("paused-approval");
  p = applyAgentEvent(p, { seq: 1, kind: "permission.requested", nodeId: "a", payload: { planHash: "hash" } });
  p = applyAgentEvent(p, { seq: 2, kind: "run.paused" });
  p = applyAgentEvent(p, { seq: 3, kind: "permission.resolved", payload: { planHash: "hash", nodeIds: ["a"], approved: true } });
  assert.equal(p.status, "paused");
});

test("foreign run approvals cannot replace the active run projection", () => {
  const p = createRunProjection("active");
  const foreign = applyAgentEvent(p, { run_id: "background", seq: 1, kind: "permission.requested", node_id: "a", payload: { planHash: "hash" } });
  assert.equal(foreign, p);
});

test("late pause and resume events cannot reopen a terminal run", () => {
  let p = createRunProjection("terminal");
  p = applyAgentEvent(p, { seq: 1, kind: "run.completed", payload: {} });
  p = applyAgentEvent(p, { seq: 2, kind: "run.paused", payload: {} });
  p = applyAgentEvent(p, { seq: 3, kind: "run.resumed", payload: {} });
  p = applyAgentEvent(p, { seq: 4, kind: "node.started", nodeId: "late", payload: {} });
  p = applyAgentEvent(p, { seq: 5, kind: "plan.revised", payload: { nodes: [] } });
  assert.equal(p.status, "completed");
  assert.equal(p.paused, false);
  assert.equal(p.activeNodeId, null);
  assert.equal(p.seq, 5);
  assert.equal(p.events.length, 5);
});
