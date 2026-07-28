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

test("renders shadow control comparison and blocks approval below sample gate", () => {
  const html = renderImprovementProposals([{ id: "p2", target: "alias", rationale: "shadow candidate", patch: {}, status: "shadowing", patchHash: "h2", shadow: { samples: 4, minimumSamples: 20, controlSuccess: 0.81, candidateSuccess: 0.9, latencyDeltaMs: -32, permissionDelta: 0 }, rollbackHistory: [{ reason: "latency regression", version: "v3" }] }]);
  assert.match(html, /SHADOW 4\/20/);
  assert.match(html, /CONTROL 81\.0%/);
  assert.match(html, /CANDIDATE 90\.0%/);
  assert.match(html, /latency regression/);
  assert.doesNotMatch(html, /批准并创建新版本/);
});

test("failed structural evaluation exposes reasons and blocks approval", () => {
  const html = renderImprovementProposals([{ id: "p3", target: "skill_patch", patchHash: "h3", evaluation: { passed: false, failures: ["permission expansion above L1"] } }]);
  assert.match(html, /permission expansion above L1/);
  assert.doesNotMatch(html, /批准并创建新版本/);
  assert.match(html, /评估未通过/);
});
