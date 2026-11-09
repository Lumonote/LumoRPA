import test from "node:test";
import assert from "node:assert/strict";

import { renderConfirmation, renderRunControls, runAgentControl } from "../src/js/agent-confirmation.js";

test("renders strengthened risk copy and redacts arguments", () => {
  const html = renderConfirmation({ runId: "r1", nodeId: "n1", risk: "L3", planHash: "hash-1", capabilityId: "mcp:mail/send", reason: "external side effect", arguments: { recipient: "a@b.com", apiKey: "raw" } });
  assert.match(html, /高风险|L3/);
  assert.match(html, /hash-1/);
  assert.match(html, /data-strength="strengthened"/);
  assert.doesNotMatch(html, /raw/);
});

test("binds approvals to the visible plan hash and rejects stale responses", async () => {
  const calls = [];
  await runAgentControl({ action: "approve", runId: "r1", nodeId: "n1", planHash: "hash-1" }, async (cmd, args) => calls.push([cmd, args]), () => "hash-1");
  await assert.rejects(() => runAgentControl({ action: "approve", runId: "r1", nodeId: "n1", planHash: "old" }, async () => {}, () => "new"), /stale/i);
  assert.equal(calls[0][0], "agent_approve");
  assert.deepEqual(calls[0][1], { runId: "r1", nodeIds: ["n1"], planHash: "hash-1", approved: true, strengthened: false });
});

test("rejects stale rejection requests as well as stale approvals", async () => {
  let called = false;
  await assert.rejects(() => runAgentControl({ action: "reject", runId: "r1", nodeId: "n1", planHash: "old" }, async () => { called = true; }, () => "new"), /stale/i);
  assert.equal(called, false);
});

test("strengthened approval requires a separate confirmation before IPC", async () => {
  const calls = [];
  const control = { action: "approve", runId: "r1", nodeId: "n1", planHash: "hash", risk: "L3" };
  const cancelled = await runAgentControl(control, async (...args) => calls.push(args), () => "hash", () => false);
  assert.equal(cancelled, null);
  assert.equal(calls.length, 0);
  await runAgentControl(control, async (...args) => calls.push(args), () => "hash", () => true);
  assert.equal(calls.length, 1);
  assert.equal(calls[0][1].strengthened, true);
});

test("a plan changed during strengthened confirmation cannot be approved", async () => {
  let hash = "old";
  let called = false;
  await assert.rejects(() => runAgentControl(
    { action: "approve", runId: "r1", nodeId: "n1", planHash: "old", risk: "L3" },
    async () => { called = true; }, () => hash,
    () => { hash = "new"; return true; },
  ), /stale/);
  assert.equal(called, false);
});

test("run controls are unavailable for idle and finished runs", () => {
  for (const status of ["idle", "completed", "failed", "cancelled"]) {
    assert.equal(renderRunControls({ runId: "r1", status }), "");
  }
  assert.match(renderRunControls({ runId: "r1", status: "paused" }), /data-agent-action="resume"/);
  assert.match(renderRunControls({ runId: "r1", status: "waiting" }), /data-agent-action="cancel"/);
});

