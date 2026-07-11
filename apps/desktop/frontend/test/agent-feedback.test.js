import test from "node:test";
import assert from "node:assert/strict";

import { selectAgentFeedback, renderAgentLog } from "../src/js/agent-feedback.js";

test("selects concise safe feedback and honors quiet mode", () => {
  assert.equal(selectAgentFeedback({ status: "completed", result: "ok" }, { quiet: true }), null);
  assert.equal(selectAgentFeedback({ status: "completed", result: "x".repeat(500) }), "任务已完成，详细结果已写入 Mission Control。");
  assert.doesNotMatch(selectAgentFeedback({ status: "failed", error: "token=raw-secret" }), /raw-secret/);
});

test("caps logs and reports dropped events with redaction", () => {
  const events = Array.from({ length: 6 }, (_, index) => ({ seq: index + 1, kind: "node.progress", payload: { token: `secret-${index}` } }));
  const html = renderAgentLog(events, 3);
  assert.match(html, /3 earlier events dropped/);
  assert.doesNotMatch(html, /secret-/);
});

