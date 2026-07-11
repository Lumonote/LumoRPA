import test from "node:test";
import assert from "node:assert/strict";

import { renderConfirmation, runAgentControl } from "../src/js/agent-confirmation.js";

test("renders strengthened risk copy and redacts arguments", () => {
  const html = renderConfirmation({ runId: "r1", nodeId: "n1", risk: "L3", planHash: "hash-1", capabilityId: "mcp:mail/send", reason: "external side effect", arguments: { recipient: "a@b.com", apiKey: "raw" } });
  assert.match(html, /高风险|L3/);
  assert.match(html, /hash-1/);
  assert.doesNotMatch(html, /raw/);
});

test("binds approvals to the visible plan hash and rejects stale responses", async () => {
  const calls = [];
  await runAgentControl({ action: "approve", runId: "r1", nodeId: "n1", planHash: "hash-1" }, async (cmd, args) => calls.push([cmd, args]), () => "hash-1");
  await assert.rejects(() => runAgentControl({ action: "approve", runId: "r1", nodeId: "n1", planHash: "old" }, async () => {}, () => "new"), /stale/i);
  assert.equal(calls[0][0], "agent_approve");
  assert.equal(calls[0][1].planHash, "hash-1");
});

