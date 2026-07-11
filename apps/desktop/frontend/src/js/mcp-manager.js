const escapeHtml = (value) => String(value ?? "").replaceAll("&", "&amp;").replaceAll("<", "&lt;").replaceAll(">", "&gt;").replaceAll('"', "&quot;").replaceAll("'", "&#39;");

export function renderMcpImportWorkspace(preview) {
  const servers = preview?.servers || [];
  const secrets = preview?.secrets || preview?.secretCandidates || [];
  return `<div class="mcp-import-preview" data-import-token="${escapeHtml(preview?.token)}"><div class="mcp-import-servers">${servers.map((server) => `<label><input type="checkbox" data-import-server="${escapeHtml(server.id)}" checked><span><strong>${escapeHtml(server.name || server.id)}</strong><em>${escapeHtml(String(server.transport?.type || server.transport || "auto"))}</em></span></label>`).join("")}</div>${secrets.length ? `<div class="mcp-secret-grid">${secrets.map((secret) => `<label><span>${escapeHtml(secret.serverId)} · ${escapeHtml(secret.fieldPath)}</span><input data-secret-vault-key value="${escapeHtml(secret.suggestedVaultKey)}" data-server-id="${escapeHtml(secret.serverId)}" data-field-path="${escapeHtml(secret.fieldPath)}"><input type="password" data-secret-value placeholder="输入密钥，留空则使用已有 Vault 值" data-server-id="${escapeHtml(secret.serverId)}" data-field-path="${escapeHtml(secret.fieldPath)}"></label>`).join("")}</div>` : ""}<button class="primary" data-apply-mcp-import>写入 Vault 并导入所选服务器</button></div>`;
}

export function buildMcpApplyRequest({ token, serverInputs, secretInputs }) {
  return { token, selectedIds: serverInputs.filter((input) => input.checked).map((input) => input.id), secretOverrides: secretInputs.map((input) => ({ serverId: input.serverId, fieldPath: input.fieldPath, vaultKey: input.vaultKey, value: input.value || null })) };
}

