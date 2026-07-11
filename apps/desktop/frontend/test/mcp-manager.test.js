import test from "node:test";
import assert from "node:assert/strict";

import { renderMcpImportWorkspace, buildMcpApplyRequest } from "../src/js/mcp-manager.js";

test("renders secret inputs without exposing imported values", () => {
  const html = renderMcpImportWorkspace({ token: "t1", servers: [{ id: "s1", name: "Local" }], secrets: [{ serverId: "s1", fieldPath: "env.API_TOKEN", suggestedVaultKey: "mcp/s1/api_token" }] });
  assert.match(html, /mcp\/s1\/api_token/);
  assert.match(html, /type="password"/);
  assert.doesNotMatch(html, /raw-secret/);
});

test("builds selected server and vault override request", () => {
  const request = buildMcpApplyRequest({ token: "t1", serverInputs: [{ id: "s1", checked: true }], secretInputs: [{ serverId: "s1", fieldPath: "env.API_TOKEN", vaultKey: "mcp/s1/api", value: "secret" }] });
  assert.deepEqual(request.selectedIds, ["s1"]);
  assert.equal(request.secretOverrides[0].vaultKey, "mcp/s1/api");
});

