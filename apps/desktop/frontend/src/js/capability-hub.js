import { renderImprovementProposals, runImprovementAction } from "./improvements.js";
import { buildMcpApplyRequest, collectMcpToolArguments, renderMcpGovernance, renderMcpImportWorkspace, renderMcpToolCall, runMcpGovernanceAction } from "./mcp-manager.js";
import { renderSecurityCenter, runSecurityAction } from "./security-center.js";
import { renderJobs, runJobAction } from "./job-manager.js";
import { applyStaticTranslations, getLocale, subscribeLocale, t, tEnum } from "./i18n.js";

const escapeHtml = (value) => String(value ?? "")
  .replaceAll("&", "&amp;").replaceAll("<", "&lt;").replaceAll(">", "&gt;")
  .replaceAll('"', "&quot;").replaceAll("'", "&#39;");

const label = (value) => String(value || "unknown").replaceAll(/[_-]+/g, " ")
  .replace(/\b\w/g, (char) => char.toUpperCase());

const numberLocale = () => (getLocale() === "zh-CN" ? "zh-CN" : "en-US");

export function redactedValue() { return "••••••••"; }

function emptyRow(message) {
  return `<div class="hub-empty"><strong>${escapeHtml(message)}</strong><span>${escapeHtml(t("hub.empty.hint"))}</span></div>`;
}

export function renderServerRows(servers = []) {
  if (!servers.length) return emptyRow(t("hub.empty.servers"));
  return servers.map((server) => {
    const health = server.health || server.status || "unknown";
    const tools = Array.isArray(server.tools) ? server.tools : [];
    return `<article class="hub-row"><div><strong>${escapeHtml(server.name || server.id || t("hub.unnamed_server"))}</strong><span>${escapeHtml(server.description || t("hub.tools_available", { n: tools.length || server.tools || 0 }))}</span><div class="hub-tool-chips">${tools.slice(0, 6).map((tool) => `<button data-mcp-tool="${escapeHtml(tool.name)}" data-server-id="${escapeHtml(server.id)}">${escapeHtml(tool.name)}</button>`).join("")}</div>${renderMcpGovernance(server)}</div><div class="hub-tags"><span class="hub-tag">${escapeHtml(String(server.transport || "unknown").toUpperCase())}</span><span class="hub-tag health-${escapeHtml(health)}">${escapeHtml(tEnum("hub.status", health, label(health)))}</span><button data-mcp-action="test" data-server-id="${escapeHtml(server.id)}">${escapeHtml(t("hub.action.test"))}</button><button data-mcp-action="discover" data-server-id="${escapeHtml(server.id)}">${escapeHtml(t("hub.action.discover"))}</button><button data-mcp-action="toggle" data-enabled="${Boolean(server.enabled)}" data-server-id="${escapeHtml(server.id)}">${escapeHtml(server.enabled === false ? t("hub.action.enable") : t("hub.action.disable"))}</button><button data-mcp-action="delete" data-server-id="${escapeHtml(server.id)}">${escapeHtml(t("hub.action.delete"))}</button></div></article>`;
  }).join("");
}

export function renderSkillRows(skills = []) {
  if (!skills.length) return emptyRow(t("hub.empty.skills"));
  return skills.map((skill) => `<article class="hub-row"><div><strong>${escapeHtml(skill.name || t("hub.unnamed_skill"))}</strong><span>${escapeHtml(skill.description || t("hub.no_description"))}</span><small>${escapeHtml(skill.hash || skill.version || t("hub.legacy"))}</small></div><div class="hub-tags"><span class="hub-tag">${escapeHtml(tEnum("hub.status", skill.status || (skill.enabled === false ? "disabled" : "active"), label(skill.status)))}</span>${skill.hash ? `<button data-skill-action="validate" data-skill-name="${escapeHtml(skill.name)}" data-skill-hash="${escapeHtml(skill.hash)}">${escapeHtml(t("hub.action.validate"))}</button><button data-skill-action="rollback" data-skill-name="${escapeHtml(skill.name)}">${escapeHtml(t("hub.action.rollback"))}</button><button data-skill-action="toggle" data-skill-name="${escapeHtml(skill.name)}" data-enabled="${Boolean(skill.enabled)}">${escapeHtml(skill.enabled === false ? t("hub.action.enable") : t("hub.action.disable"))}</button>` : ""}</div></article>`).join("");
}

export function renderAgentProfileRows(profiles = []) {
  if (!profiles.length) return emptyRow(t("hub.empty.profiles"));
  return profiles.map((profile) => {
    const budgets = profile.budgets || profile.budget || {};
    const tools = Number(budgets.tools ?? profile.toolBudget ?? 0).toLocaleString(numberLocale());
    const tokens = Number(budgets.tokens ?? profile.tokenBudget ?? 0).toLocaleString(numberLocale());
    return `<article class="hub-row"><div><strong>${escapeHtml(profile.name || t("hub.unnamed_profile"))}</strong><span>${escapeHtml(profile.model || t("hub.default_model"))}</span></div><div class="hub-budget"><span>${escapeHtml(t("hub.budget_tools", { n: tools }))}</span><span>${escapeHtml(t("hub.budget_tokens", { n: tokens }))}</span></div></article>`;
  }).join("");
}

export function renderToolSchema(tool = {}) {
  const schema = tool.inputSchema || tool.input_schema || tool.schema || {};
  return `<section class="hub-schema"><div><strong>${escapeHtml(tool.name || t("hub.tool_schema"))}</strong><span>${escapeHtml(tool.description || t("hub.input_contract"))}</span></div><pre>${escapeHtml(JSON.stringify(schema, null, 2))}</pre></section>`;
}

export function renderImportPreview(batch = {}) {
  const servers = Array.isArray(batch.servers) ? batch.servers : [];
  const secrets = Array.isArray(batch.secretCandidates) ? batch.secretCandidates : [];
  const conflicts = Array.isArray(batch.conflicts) ? batch.conflicts : [];
  return `<div class="hub-preview-summary"><div><strong>${servers.length}</strong><span>${escapeHtml(t("hub.preview.servers_detected"))}</span></div><div><strong>${secrets.length}</strong><span>${escapeHtml(t("hub.preview.secrets"))} → ${escapeHtml(batch.vaultTarget || "Lumo Vault")}</span></div><div><strong>${conflicts.length}</strong><span>${escapeHtml(t("hub.preview.conflicts"))}</span></div></div>${servers.map((server) => `<div class="hub-preview-item"><strong>${escapeHtml(server.name || server.id || t("hub.unnamed_server"))}</strong><span>${escapeHtml(String(server.transport || "auto").toUpperCase())}</span></div>`).join("")}`;
}

const sectionTitle = (key) => t(`hub.nav.${key}`);

function overviewMarkup(data) {
  return `<div class="hub-metrics"><article><span>${escapeHtml(t("hub.metric.servers"))}</span><strong>${data.servers.length}</strong></article><article><span>${escapeHtml(t("hub.metric.skills"))}</span><strong>${data.skills.length}</strong></article><article><span>${escapeHtml(t("hub.metric.profiles"))}</span><strong>${data.profiles.length}</strong></article><article><span>${escapeHtml(t("hub.metric.vault"))}</span><strong>${escapeHtml(t("hub.metric.protected"))}</strong></article></div><div class="hub-grid"><section class="hub-card"><header><div><span class="hub-eyebrow">${escapeHtml(t("hub.eyebrow.runtime_mesh"))}</span><h3>${escapeHtml(t("hub.card.servers"))}</h3></div></header>${renderServerRows(data.servers)}</section><section class="hub-card"><header><div><span class="hub-eyebrow">${escapeHtml(t("hub.eyebrow.operator_layer"))}</span><h3>${escapeHtml(t("hub.card.profiles"))}</h3></div></header>${renderAgentProfileRows(data.profiles)}</section></div>`;
}


export function mountCapabilityHub({ call, root }) {
  if (!root) return { refresh: async () => {} };
  const data = { servers: [], skills: [], profiles: [], proposals: [], jobs: [], security: {}, errors: [] };
  let active = "overview";
  const content = root.querySelector("[data-hub-content]");
  const render = () => {
    // 静态骨架（导航、导入向导）由词表驱动，语言切换后一并刷新
    applyStaticTranslations(root);
    root.querySelectorAll("[data-hub-section]").forEach((button) => button.classList.toggle("is-active", button.dataset.hubSection === active));
    if (active === "overview") content.innerHTML = overviewMarkup(data);
    else if (active === "servers") content.innerHTML = `<section class="hub-card"><header><div><span class="hub-eyebrow">${escapeHtml(t("hub.eyebrow.protocol_fabric"))}</span><h3>${escapeHtml(t("hub.card.servers"))}</h3></div><button class="primary" data-open-import>${escapeHtml(t("hub.action.import_server"))}</button></header>${renderServerRows(data.servers)}<div data-mcp-tool-call></div></section>`;
    else if (active === "skills") content.innerHTML = `<section class="hub-card"><header><div><span class="hub-eyebrow">${escapeHtml(t("hub.eyebrow.local_intelligence"))}</span><h3>${escapeHtml(t("hub.card.skills"))}</h3></div></header><div class="hub-import-editor"><input data-skill-local-path placeholder="${escapeHtml(t("hub.skill.local_path"))}"><button data-skill-import-local>${escapeHtml(t("hub.skill.import_local"))}</button><input data-skill-git-url placeholder="${escapeHtml(t("hub.skill.git_url"))}"><input data-skill-git-revision placeholder="${escapeHtml(t("hub.skill.git_revision"))}"><button data-skill-import-git>${escapeHtml(t("hub.skill.import_git"))}</button></div>${renderSkillRows(data.skills)}</section>`;
    else if (active === "profiles") content.innerHTML = `<section class="hub-card"><header><div><span class="hub-eyebrow">${escapeHtml(t("hub.eyebrow.execution_policy"))}</span><h3>${escapeHtml(t("hub.card.profiles"))}</h3></div></header>${renderAgentProfileRows(data.profiles)}</section>`;
    else if (active === "jobs") content.innerHTML = `<section class="hub-card"><header><div><span class="hub-eyebrow">${escapeHtml(t("hub.eyebrow.scheduler"))}</span><h3>${escapeHtml(t("hub.card.jobs"))}</h3></div></header>${renderJobs(data.jobs)}</section>`;
    else if (active === "permissions") content.innerHTML = `<section class="hub-card">${renderSecurityCenter(data.security || {})}</section>`;
    else if (active === "proposals") content.innerHTML = `<section class="hub-card"><header><div><span class="hub-eyebrow">${escapeHtml(t("hub.eyebrow.evolution"))}</span><h3>${escapeHtml(t("hub.card.proposals"))}</h3><p>${escapeHtml(t("hub.proposals_note"))}</p></div></header>${renderImprovementProposals(data.proposals)}</section>`;
    else content.innerHTML = `<section class="hub-card hub-coming"><span class="hub-eyebrow">${escapeHtml(sectionTitle(active))}</span><h3>${escapeHtml(t("hub.coming.title"))}</h3><p>${escapeHtml(t("hub.coming.note"))}</p></section>`;
    const error = root.querySelector("[data-hub-errors]");
    error.textContent = data.errors.length ? t("hub.backend_error", { detail: data.errors.join(" · ") }) : t("hub.online");
    error.classList.toggle("is-warning", data.errors.length > 0);
  };
  subscribeLocale(() => render());
  root.addEventListener("click", async (event) => {
    const tab = event.target.closest("[data-hub-section]");
    if (tab) { active = tab.dataset.hubSection; render(); }
    const importButton = event.target.closest("[data-import-mode]");
    if (importButton) root.querySelector("[data-import-state]").textContent = t("hub.import.selected", { mode: tEnum("hub.import.mode", importButton.dataset.importMode, label(importButton.dataset.importMode)) });
    if (event.target.closest("[data-open-import]")) root.querySelector("[data-import-wizard]")?.scrollIntoView({ behavior: "smooth" });
    if (event.target.closest("[data-preview-mcp-import]")) {
      const preview = await call("preview_mcp_import", { sourceName: root.querySelector("[data-mcp-source-name]").value, content: root.querySelector("[data-mcp-import-content]").value });
      root.querySelector("[data-mcp-import-preview]").innerHTML = renderMcpImportWorkspace(preview);
      root.querySelector("[data-import-state]").textContent = t("hub.import.summary", { servers: preview.servers?.length || 0, secrets: preview.secrets?.length || 0 });
    }
    if (event.target.closest("[data-apply-mcp-import]")) {
      const previewRoot = root.querySelector("[data-import-token]");
      const vaultKeys = [...previewRoot.querySelectorAll("[data-secret-vault-key]")];
      const request = buildMcpApplyRequest({ token: previewRoot.dataset.importToken, serverInputs: [...previewRoot.querySelectorAll("[data-import-server]")].map((input) => ({ id: input.dataset.importServer, checked: input.checked })), secretInputs: vaultKeys.map((input) => ({ serverId: input.dataset.serverId, fieldPath: input.dataset.fieldPath, vaultKey: input.value, value: previewRoot.querySelector(`[data-secret-value][data-server-id="${CSS.escape(input.dataset.serverId)}"][data-field-path="${CSS.escape(input.dataset.fieldPath)}"]`)?.value })) });
      data.servers = await call("apply_mcp_import", request);
      active = "servers"; render();
    }
    const mcpAction = event.target.closest("[data-mcp-action]");
    if (mcpAction) {
      const id = mcpAction.dataset.serverId;
      if (mcpAction.dataset.mcpAction === "test") await call("test_mcp_server", { id });
      if (mcpAction.dataset.mcpAction === "discover") await call("discover_mcp_tools", { id });
      if (mcpAction.dataset.mcpAction === "toggle") await call("set_mcp_server_enabled", { id, enabled: mcpAction.dataset.enabled !== "true" });
      if (mcpAction.dataset.mcpAction === "delete") await call("delete_mcp_server", { id });
      data.servers = await call("list_mcp_servers"); render();
    }
    const tool = event.target.closest("[data-mcp-tool]");
    if (tool) {
      const server = data.servers.find((item) => item.id === tool.dataset.serverId);
      const definition = (server?.tools || []).find((item) => item.name === tool.dataset.mcpTool) || { name: tool.dataset.mcpTool };
      root.querySelector("[data-mcp-tool-call]").innerHTML = renderMcpToolCall(tool.dataset.serverId, definition);
    }
    const security = event.target.closest("[data-security-action]");
    if (security) await runSecurityAction({ action: security.dataset.securityAction, grantId: security.dataset.grantId }, call);
    const job = event.target.closest("[data-job-action]");
    if (job) {
      await runJobAction({ action: job.dataset.jobAction, jobId: job.dataset.jobId }, call);
      data.jobs = await call("job_list");
      render();
    }
    if (event.target.closest("[data-skill-import-local]")) {
      await call("skill_import_local", { path: root.querySelector("[data-skill-local-path]").value });
      data.skills = await call("list_skills"); render();
    }
    if (event.target.closest("[data-skill-import-git]")) {
      await call("skill_import_git", { url: root.querySelector("[data-skill-git-url]").value, revision: root.querySelector("[data-skill-git-revision]").value || null });
      data.skills = await call("list_skills"); render();
    }
    const skillAction = event.target.closest("[data-skill-action]");
    if (skillAction) {
      const action = skillAction.dataset.skillAction;
      if (action === "validate") await call("skill_validate", { name: skillAction.dataset.skillName, hash: skillAction.dataset.skillHash });
      if (action === "rollback") await call("skill_rollback", { name: skillAction.dataset.skillName });
      if (action === "toggle") await call("skill_set_enabled", { name: skillAction.dataset.skillName, enabled: skillAction.dataset.enabled !== "true" });
      data.skills = await call("list_skills"); render();
    }
    const governance = event.target.closest("[data-mcp-governance-action]");
    if (governance) await runMcpGovernanceAction({ action: governance.dataset.mcpGovernanceAction, serverId: governance.dataset.serverId, tool: governance.dataset.tool, schemaHash: governance.dataset.schemaHash }, call);
    const improvement = event.target.closest("[data-improvement-action]");
    if (improvement) {
      improvement.disabled = true;
      try {
        await runImprovementAction({ action: improvement.dataset.improvementAction, proposalId: improvement.dataset.proposalId, patchHash: improvement.dataset.patchHash }, call);
        const result = await call("list_improvement_proposals");
        data.proposals = Array.isArray(result) ? result : result?.items || [];
        render();
      } finally { improvement.disabled = false; }
    }
  });
  root.addEventListener("submit", async (event) => {
    const form = event.target.closest("[data-mcp-call-server]");
    if (!form) return;
    event.preventDefault();
    const result = await call("call_mcp_tool", { id: form.dataset.mcpCallServer, tool: form.dataset.mcpCallTool, arguments: collectMcpToolArguments(form.querySelectorAll("[data-mcp-argument]")) });
    form.querySelector("[data-mcp-call-result]").textContent = JSON.stringify(result, null, 2);
  });
  const refresh = async () => {
    data.errors = [];
    const requests = [["servers", "list_mcp_servers"], ["skills", "list_skills"], ["profiles", "list_agent_profiles"], ["proposals", "list_improvement_proposals"], ["jobs", "job_list"], ["security", "security_list"]];
    await Promise.all(requests.map(async ([key, command]) => {
      try { const result = await call(command); data[key] = Array.isArray(result) ? result : result?.items || []; }
      catch { data[key] = []; data.errors.push(label(key)); }
    }));
    render();
  };
  render();
  refresh();
  return { refresh };
}
