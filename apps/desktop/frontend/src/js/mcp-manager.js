const escapeHtml = (value) => String(value ?? "").replaceAll("&", "&amp;").replaceAll("<", "&lt;").replaceAll(">", "&gt;").replaceAll('"', "&quot;").replaceAll("'", "&#39;");

export function renderMcpImportWorkspace(preview) {
  const servers = preview?.servers || [];
  const secrets = preview?.secrets || preview?.secretCandidates || [];
  return `<div class="mcp-import-preview" data-import-token="${escapeHtml(preview?.token)}"><div class="mcp-import-servers">${servers.map((server) => `<label><input type="checkbox" data-import-server="${escapeHtml(server.id)}" checked><span><strong>${escapeHtml(server.name || server.id)}</strong><em>${escapeHtml(String(server.transport?.type || server.transport || "auto"))}</em></span></label>`).join("")}</div>${secrets.length ? `<div class="mcp-secret-grid">${secrets.map((secret) => `<label><span>${escapeHtml(secret.serverId)} · ${escapeHtml(secret.fieldPath)}</span><input data-secret-vault-key value="${escapeHtml(secret.suggestedVaultKey)}" data-server-id="${escapeHtml(secret.serverId)}" data-field-path="${escapeHtml(secret.fieldPath)}"><input type="password" data-secret-value placeholder="输入密钥，留空则使用已有 Vault 值" data-server-id="${escapeHtml(secret.serverId)}" data-field-path="${escapeHtml(secret.fieldPath)}"></label>`).join("")}</div>` : ""}<button class="primary" data-apply-mcp-import>写入 Vault 并导入所选服务器</button></div>`;
}

export function buildMcpApplyRequest({ token, serverInputs, secretInputs }) {
  return { token, selectedIds: serverInputs.filter((input) => input.checked).map((input) => input.id), secretOverrides: secretInputs.map((input) => ({ serverId: input.serverId, fieldPath: input.fieldPath, vaultKey: input.vaultKey, value: input.value || null })) };
}

export function renderMcpToolCall(serverId, tool = {}) {
  const schema = tool.inputSchema || tool.input_schema || {};
  const required = new Set(schema.required || []);
  const fields = Object.entries(schema.properties || {}).map(([name, property]) => {
    const type = property.type || "string";
    const inputType = property.format === "password" ? "password" : ["integer", "number"].includes(type) ? "number" : type === "boolean" ? "checkbox" : "text";
    const control = ["object", "array"].includes(type)
      ? `<textarea data-mcp-argument="${escapeHtml(name)}" data-schema-type="${escapeHtml(type)}"${required.has(name) ? " required" : ""}></textarea>`
      : `<input type="${inputType}" data-mcp-argument="${escapeHtml(name)}" data-schema-type="${escapeHtml(type)}"${required.has(name) ? " required" : ""}>`;
    return `<label><span>${escapeHtml(name)}${required.has(name) ? " *" : ""}</span>${control}<small>${escapeHtml(property.description || type)}</small></label>`;
  }).join("");
  return `<form class="mcp-tool-call" data-mcp-call-server="${escapeHtml(serverId)}" data-mcp-call-tool="${escapeHtml(tool.name)}"><header><strong>${escapeHtml(tool.name || "MCP tool")}</strong><span>${escapeHtml(tool.description || "Schema-driven invocation")}</span></header>${fields || '<div class="hub-empty">此工具不需要参数</div>'}<button type="submit">调用工具</button><pre data-mcp-call-result></pre></form>`;
}

export function collectMcpToolArguments(fields = []) {
  return Object.fromEntries([...fields].map((field) => {
    const type = field.dataset.schemaType || "string";
    let value = type === "boolean" ? Boolean(field.checked) : field.value;
    if (["integer", "number"].includes(type) && value !== "") value = Number(value);
    if (["object", "array"].includes(type) && value !== "") value = JSON.parse(value);
    return [field.dataset.mcpArgument, value];
  }).filter(([, value]) => value !== ""));
}

export function renderMcpGovernance(server = {}) {
  const oauth = server.oauth || { state: "not configured", scopes: [] };
  const supervisor = server.supervisor || { state: "closed", failures: 0 };
  const changes = server.schemaChanges || [];
  return `<section class="mcp-governance"><div class="mcp-governance-status"><span>OAUTH ${escapeHtml(String(oauth.state).toUpperCase())}</span><span>CIRCUIT ${escapeHtml(String(supervisor.state).toUpperCase())}</span><span>${Number(supervisor.failures || 0)} FAILURES</span></div><div class="mcp-governance-actions"><button data-mcp-governance-action="oauth" data-server-id="${escapeHtml(server.id)}">${oauth.state === "active" ? "重新授权" : "连接 OAuth"}</button></div>${changes.map((change) => `<article><div><strong>${escapeHtml(change.tool)}</strong><span>${escapeHtml(change.oldHash)} → ${escapeHtml(change.newHash)}</span></div><button data-mcp-governance-action="approve-schema" data-server-id="${escapeHtml(server.id)}" data-tool="${escapeHtml(change.tool)}" data-schema-hash="${escapeHtml(change.newHash)}">批准 Schema</button></article>`).join("")}</section>`;
}

const governanceCommands = { oauth: "mcp_oauth_start", "approve-schema": "approve_mcp_schema_change" };

export function runMcpGovernanceAction(action, call) {
  const command = governanceCommands[action.action];
  if (!command) throw new Error(`unknown MCP governance action: ${action.action}`);
  return call(command, { id: action.serverId, tool: action.tool, schemaHash: action.schemaHash });
}
