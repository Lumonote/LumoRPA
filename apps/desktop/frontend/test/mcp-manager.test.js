import test from "node:test";
import assert from "node:assert/strict";

import { collectMcpToolArguments, renderMcpImportWorkspace, buildMcpApplyRequest, renderMcpGovernance, renderMcpToolCall, runMcpGovernanceAction } from "../src/js/mcp-manager.js";

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

test("renders OAuth health circuit and schema drift governance", () => {
  const html = renderMcpGovernance({ id: "erp", oauth: { state: "expired", scopes: ["tools:call"] }, supervisor: { state: "open", failures: 5, retryAt: "12:30" }, schemaChanges: [{ tool: "send_invoice", oldHash: "a", newHash: "b" }] });
  assert.match(html, /OAUTH EXPIRED/);
  assert.match(html, /CIRCUIT OPEN/);
  assert.match(html, /send_invoice/);
  assert.match(html, /批准 Schema/);
});

test("maps MCP OAuth reconnect and schema approval actions", async () => {
  const calls = [];
  await runMcpGovernanceAction({ action: "oauth", serverId: "erp" }, async (cmd, args) => calls.push([cmd, args]));
  await runMcpGovernanceAction({ action: "approve-schema", serverId: "erp", tool: "send_invoice", schemaHash: "b" }, async (cmd, args) => calls.push([cmd, args]));
  assert.deepEqual(calls.map(([cmd]) => cmd), ["mcp_oauth_start", "approve_mcp_schema_change"]);
});

test("renders MCP tool inputs from JSON Schema without leaking defaults", () => {
  const html = renderMcpToolCall("erp", { name: "lookup", inputSchema: { type: "object", required: ["query"], properties: { query: { type: "string" }, limit: { type: "integer", default: 10 }, secret: { type: "string", format: "password" } } } });
  assert.match(html, /data-mcp-argument="query"/);
  assert.match(html, /type="number"/);
  assert.match(html, /type="password"/);
  assert.match(html, /required/);
});

test("collects typed MCP tool arguments", () => {
  const fields = [
    { dataset: { mcpArgument: "query", schemaType: "string" }, value: "lumo" },
    { dataset: { mcpArgument: "limit", schemaType: "integer" }, value: "5" },
    { dataset: { mcpArgument: "enabled", schemaType: "boolean" }, checked: true },
  ];
  assert.deepEqual(collectMcpToolArguments(fields), { query: "lumo", limit: 5, enabled: true });
});
