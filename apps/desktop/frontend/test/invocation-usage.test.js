import test from "node:test";
import assert from "node:assert/strict";
import { createUsageResolution, renderInvocationUsage } from "../src/js/invocation-usage.js";

const row = { runId: "run", nodeId: "node", attempt: 2, revision: 7, state: "failed", usageState: "unknown", tokensUsed: 20, costUsdMicro: 10 };
test("usage display distinguishes unknown totals, lower bounds and measured zero", () => {
  const result = renderInvocationUsage([row, { ...row, tokensUsed: null, costUsdMicro: null }, { ...row, usageState: "recorded", tokensUsed: 0, costUsdMicro: 0 }]);
  assert.match(result, /至少 20/);
  assert.match(result, /至少 0\.000010/);
  assert.match(result, /未知/);
  assert.match(result, /已记录/);
  assert.match(result, /核对账单/);
  assert.doesNotMatch(renderInvocationUsage([{ ...row, state: "running" }]), /data-resolve-usage/);
});
test("billing resolution preserves revision and requires final totals above the known lower bound", () => {
  const values = { tokensUsed: "25", costUsdMicro: "30", resolvedBy: " 操作员 ", reason: "服务账单编号 42" };
  assert.deepEqual(createUsageResolution(row, values), { runId: "run", nodeId: "node", attempt: 2, expectedRevision: 7, tokensUsed: 25, costUsdMicro: 30, resolvedBy: "操作员", reason: "服务账单编号 42" });
  for (const patch of [{ tokensUsed: "19" }, { costUsdMicro: "9" }, { tokensUsed: "" }, { tokensUsed: "1e3" }, { tokensUsed: "9007199254740992" }, { resolvedBy: "" }, { reason: " " }]) {
    assert.throws(() => createUsageResolution(row, { ...values, ...patch }));
  }
});
test("usage values and reviewer names cannot inject markup", () => {
  const rendered = renderInvocationUsage([{ ...row, nodeId: '<img src=x onerror="boom">', resolvedBy: "<script>bad</script>", usageState: "recorded" }]);
  assert.doesNotMatch(rendered, /<img|<script/);
});
