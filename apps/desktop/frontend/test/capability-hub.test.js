import test from "node:test";
import assert from "node:assert/strict";

import {
  redactedValue,
  renderAgentProfileRows,
  renderImportPreview,
  renderServerRows,
  renderSkillRows,
  renderToolSchema,
} from "../src/js/capability-hub.js";

test("server rows escape labels and expose transport and health", () => {
  const html = renderServerRows([{ name: "<unsafe>", transport: "stdio", health: "healthy", tools: 4 }]);
  assert.match(html, /&lt;unsafe&gt;/);
  assert.doesNotMatch(html, /<unsafe>/);
  assert.match(html, /STDIO/);
  assert.match(html, /Healthy/);
});

test("server and skill renderers provide useful empty states", () => {
  assert.match(renderServerRows([]), /No MCP servers/i);
  assert.match(renderSkillRows([]), /No skills/i);
});

test("import preview never leaks secret candidates or config values", () => {
  const html = renderImportPreview({
    servers: [{ name: "local", config: { token: "raw-config-secret" } }],
    secretCandidates: [{ key: "TOKEN", value: "candidate-secret" }],
    vaultTarget: "Lumo Vault / MCP",
  });
  assert.doesNotMatch(html, /raw-config-secret|candidate-secret/);
  assert.match(html, /1 secret/i);
  assert.match(html, /Lumo Vault \/ MCP/);
  assert.equal(redactedValue("candidate-secret"), "••••••••");
});

test("tool schema is escaped and pretty printed", () => {
  const html = renderToolSchema({ name: "lookup", inputSchema: { type: "object", properties: { query: { type: "string", description: "<term>" } } } });
  assert.match(html, /&quot;type&quot;: &quot;object&quot;/);
  assert.match(html, /&lt;term&gt;/);
  assert.doesNotMatch(html, /<term>/);
});

test("skill rows escape content", () => {
  const html = renderSkillRows([{ name: "Research", description: "Use <browser>", status: "ready" }]);
  assert.match(html, /Use &lt;browser&gt;/);
  assert.match(html, /Ready/);
});

test("agent profiles render tool and token budgets", () => {
  const html = renderAgentProfileRows([{ name: "Operator", model: "gpt-5", budgets: { tools: 12, tokens: 64000 } }]);
  assert.match(html, /12 tools/);
  assert.match(html, /64,000 tokens/);
});
