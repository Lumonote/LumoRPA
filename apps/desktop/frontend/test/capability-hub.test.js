import test, { beforeEach } from "node:test";
import assert from "node:assert/strict";

import {
  redactedValue,
  renderAgentProfileRows,
  renderImportPreview,
  renderServerRows,
  renderSkillRows,
  renderToolSchema,
} from "../src/js/capability-hub.js";
import { setLocale } from "../src/js/i18n.js";

// 渲染结果随 locale 变化，逐例复位到默认语言，避免用例间互相污染。
beforeEach(() => setLocale("zh-CN"));

test("server rows escape labels and expose transport and health", () => {
  const html = renderServerRows([{ name: "<unsafe>", transport: "stdio", health: "healthy", tools: 4 }]);
  assert.match(html, /&lt;unsafe&gt;/);
  assert.doesNotMatch(html, /<unsafe>/);
  assert.match(html, /STDIO/);
  assert.match(html, /健康/);
});

test("server and skill renderers provide useful empty states", () => {
  assert.match(renderServerRows([]), /尚未配置 MCP 服务器/);
  assert.match(renderSkillRows([]), /尚未安装技能/);
});

test("import preview never leaks secret candidates or config values", () => {
  const html = renderImportPreview({
    servers: [{ name: "local", config: { token: "raw-config-secret" } }],
    secretCandidates: [{ key: "TOKEN", value: "candidate-secret" }],
    vaultTarget: "Lumo Vault / MCP",
  });
  assert.doesNotMatch(html, /raw-config-secret|candidate-secret/);
  assert.match(html, /1<\/strong><span>个密钥/);
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
  // 词表未收录的状态值回落为人类可读原值，而不是裸键名
  assert.match(html, /Ready/);
  assert.doesNotMatch(html, /hub\.status\./);
});

test("agent profiles render tool and token budgets", () => {
  const html = renderAgentProfileRows([{ name: "Operator", model: "gpt-5", budgets: { tools: 12, tokens: 64000 } }]);
  assert.match(html, /12 工具/);
  assert.match(html, /64,000 tokens/);
});

test("switching to en renders the English surface", () => {
  setLocale("en");
  assert.match(renderServerRows([]), /No MCP servers configured/);
  assert.match(renderSkillRows([]), /No skills installed/);
  const profiles = renderAgentProfileRows([{ name: "Operator", budgets: { tools: 12, tokens: 64000 } }]);
  assert.match(profiles, /12 tools/);
  assert.match(profiles, /Default model/);
  assert.match(renderServerRows([{ name: "srv", transport: "stdio", health: "healthy", tools: 4 }]), /Healthy/);
});

test("zh-CN keeps the default surface after switching back", () => {
  setLocale("en");
  setLocale("zh-CN");
  assert.match(renderServerRows([]), /尚未配置 MCP 服务器/);
});
