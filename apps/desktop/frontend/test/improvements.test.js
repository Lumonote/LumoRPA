import test from "node:test";
import assert from "node:assert/strict";

import { renderImprovementProposals, runImprovementAction } from "../src/js/improvements.js";

test("renders redacted structured diffs and evaluation deltas", () => {
  const html = renderImprovementProposals([{ id: "p1", target: "router_example", rationale: "faster route", patch: { alias: "日报", token: "raw-secret" }, status: "evaluated", metrics: { successDelta: 0.12, latencyDeltaMs: -84, riskDelta: 0 } }]);
  assert.match(html, /router example/i);
  assert.match(html, /\+12\.0%/);
  assert.doesNotMatch(html, /raw-secret/);
  assert.match(html, /批准并创建新版本/);
});

test("maps approve reject evaluate and rollback to durable commands", async () => {
  const calls = [];
  for (const action of ["evaluate", "approve", "reject", "rollback"]) await runImprovementAction({ action, proposalId: "p1", patchHash: "h1" }, async (cmd, args) => calls.push([cmd, args]));
  assert.deepEqual(calls.map(([cmd]) => cmd), ["evaluate_improvement", "approve_improvement", "reject_improvement", "rollback_improvement"]);
  assert.equal(calls[1][1].patchHash, "h1");
});

