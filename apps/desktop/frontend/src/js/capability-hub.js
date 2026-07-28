import { renderImprovementProposals, runImprovementAction } from "./improvements.js";
import { buildMcpApplyRequest, collectMcpToolArguments, renderMcpGovernance, renderMcpImportWorkspace, renderMcpToolCall, runMcpGovernanceAction } from "./mcp-manager.js";
import { renderSecurityCenter, runSecurityAction } from "./security-center.js";
import { renderJobs, runJobAction } from "./job-manager.js";

const escapeHtml = (value) => String(value ?? "")
  .replaceAll("&", "&amp;").replaceAll("<", "&lt;").replaceAll(">", "&gt;")
  .replaceAll('"', "&quot;").replaceAll("'", "&#39;");

const label = (value) => String(value || "unknown").replaceAll(/[_-]+/g, " ")
  .replace(/\b\w/g, (char) => char.toUpperCase());

export function redactedValue() { return "••••••••"; }

function emptyRow(message) {
  return `<div class="hub-empty"><strong>${escapeHtml(message)}</strong><span>Connect or import a capability to populate this view.</span></div>`;
}

export function renderServerRows(servers = []) {
  if (!servers.length) return emptyRow("No MCP servers configured");
  return servers.map((server) => {
    const health = server.health || server.status || "unknown";
    const tools = Array.isArray(server.tools) ? server.tools : [];
    return `<article class="hub-row"><div><strong>${escapeHtml(server.name || server.id || "Unnamed server")}</strong><span>${escapeHtml(server.description || `${tools.length || server.tools || 0} tools available`)}</span><div class="hub-tool-chips">${tools.slice(0, 6).map((tool) => `<button data-mcp-tool="${escapeHtml(tool.name)}" data-server-id="${escapeHtml(server.id)}">${escapeHtml(tool.name)}</button>`).join("")}</div>${renderMcpGovernance(server)}</div><div class="hub-tags"><span class="hub-tag">${escapeHtml(String(server.transport || "unknown").toUpperCase())}</span><span class="hub-tag health-${escapeHtml(health)}">${escapeHtml(label(health))}</span><button data-mcp-action="test" data-server-id="${escapeHtml(server.id)}">Test</button><button data-mcp-action="discover" data-server-id="${escapeHtml(server.id)}">Discover</button><button data-mcp-action="toggle" data-enabled="${Boolean(server.enabled)}" data-server-id="${escapeHtml(server.id)}">${server.enabled === false ? "Enable" : "Disable"}</button><button data-mcp-action="delete" data-server-id="${escapeHtml(server.id)}">Delete</button></div></article>`;
  }).join("");
}

export function renderSkillRows(skills = []) {
  if (!skills.length) return emptyRow("No skills installed");
  return skills.map((skill) => `<article class="hub-row"><div><strong>${escapeHtml(skill.name || "Unnamed skill")}</strong><span>${escapeHtml(skill.description || "No description")}</span><small>${escapeHtml(skill.hash || skill.version || "legacy")}</small></div><div class="hub-tags"><span class="hub-tag">${escapeHtml(label(skill.status || (skill.enabled === false ? "disabled" : "active")))}</span>${skill.hash ? `<button data-skill-action="validate" data-skill-name="${escapeHtml(skill.name)}" data-skill-hash="${escapeHtml(skill.hash)}">Validate</button><button data-skill-action="rollback" data-skill-name="${escapeHtml(skill.name)}">Rollback</button><button data-skill-action="toggle" data-skill-name="${escapeHtml(skill.name)}" data-enabled="${Boolean(skill.enabled)}">${skill.enabled === false ? "Enable" : "Disable"}</button>` : ""}</div></article>`).join("");
}

export function renderAgentProfileRows(profiles = []) {
  if (!profiles.length) return emptyRow("No agent profiles created");
  return profiles.map((profile) => {
    const budgets = profile.budgets || profile.budget || {};
    const tools = Number(budgets.tools ?? profile.toolBudget ?? 0).toLocaleString("en-US");
    const tokens = Number(budgets.tokens ?? profile.tokenBudget ?? 0).toLocaleString("en-US");
    return `<article class="hub-row"><div><strong>${escapeHtml(profile.name || "Unnamed profile")}</strong><span>${escapeHtml(profile.model || "Default model")}</span></div><div class="hub-budget"><span>${tools} tools</span><span>${tokens} tokens</span></div></article>`;
  }).join("");
}

export function renderToolSchema(tool = {}) {
  const schema = tool.inputSchema || tool.input_schema || tool.schema || {};
  return `<section class="hub-schema"><div><strong>${escapeHtml(tool.name || "Tool schema")}</strong><span>${escapeHtml(tool.description || "Input contract")}</span></div><pre>${escapeHtml(JSON.stringify(schema, null, 2))}</pre></section>`;
}

export function renderImportPreview(batch = {}) {
  const servers = Array.isArray(batch.servers) ? batch.servers : [];
  const secrets = Array.isArray(batch.secretCandidates) ? batch.secretCandidates : [];
  const conflicts = Array.isArray(batch.conflicts) ? batch.conflicts : [];
  return `<div class="hub-preview-summary"><div><strong>${servers.length}</strong><span>servers detected</span></div><div><strong>${secrets.length} ${secrets.length === 1 ? "secret" : "secrets"}</strong><span>→ ${escapeHtml(batch.vaultTarget || "Lumo Vault")}</span></div><div><strong>${conflicts.length}</strong><span>conflicts</span></div></div>${servers.map((server) => `<div class="hub-preview-item"><strong>${escapeHtml(server.name || server.id || "Unnamed server")}</strong><span>${escapeHtml(String(server.transport || "auto").toUpperCase())}</span></div>`).join("")}`;
}

const sections = {
  overview: "Overview", skills: "Skills", servers: "MCP Servers", catalog: "Capability Catalog",
  profiles: "Agent Profiles", jobs: "Jobs", permissions: "Permissions", audit: "Audit", proposals: "Improvement Proposals",
};

function overviewMarkup(data) {
  return `<div class="hub-metrics"><article><span>Connected servers</span><strong>${data.servers.length}</strong></article><article><span>Installed skills</span><strong>${data.skills.length}</strong></article><article><span>Agent profiles</span><strong>${data.profiles.length}</strong></article><article><span>Vault posture</span><strong>Protected</strong></article></div><div class="hub-grid"><section class="hub-card"><header><div><span class="hub-eyebrow">Runtime mesh</span><h3>MCP Servers</h3></div></header>${renderServerRows(data.servers)}</section><section class="hub-card"><header><div><span class="hub-eyebrow">Operator layer</span><h3>Agent Profiles</h3></div></header>${renderAgentProfileRows(data.profiles)}</section></div>`;
}

export function mountCapabilityHub({ call, root }) {
  if (!root) return { refresh: async () => {} };
  const data = { servers: [], skills: [], profiles: [], proposals: [], jobs: [], security: {}, errors: [] };
  let active = "overview";
  const content = root.querySelector("[data-hub-content]");
  const render = () => {
    root.querySelectorAll("[data-hub-section]").forEach((button) => button.classList.toggle("is-active", button.dataset.hubSection === active));
    if (active === "overview") content.innerHTML = overviewMarkup(data);
    else if (active === "servers") content.innerHTML = `<section class="hub-card"><header><div><span class="hub-eyebrow">Protocol fabric</span><h3>MCP Servers</h3></div><button class="primary" data-open-import>Import server</button></header>${renderServerRows(data.servers)}<div data-mcp-tool-call></div></section>`;
    else if (active === "skills") content.innerHTML = `<section class="hub-card"><header><div><span class="hub-eyebrow">Local intelligence</span><h3>Skills</h3></div></header><div class="hub-import-editor"><input data-skill-local-path placeholder="本地 SKILL.md 或目录路径"><button data-skill-import-local>导入本地 Skill</button><input data-skill-git-url placeholder="Git repository URL"><input data-skill-git-revision placeholder="revision（可选）"><button data-skill-import-git>导入 Git Skill</button></div>${renderSkillRows(data.skills)}</section>`;
    else if (active === "profiles") content.innerHTML = `<section class="hub-card"><header><div><span class="hub-eyebrow">Execution policy</span><h3>Agent Profiles</h3></div></header>${renderAgentProfileRows(data.profiles)}</section>`;
    else if (active === "jobs") content.innerHTML = `<section class="hub-card"><header><div><span class="hub-eyebrow">Durable scheduler</span><h3>Agent Jobs</h3></div></header>${renderJobs(data.jobs)}</section>`;
    else if (active === "permissions") content.innerHTML = `<section class="hub-card">${renderSecurityCenter(data.security || {})}</section>`;
    else if (active === "proposals") content.innerHTML = `<section class="hub-card"><header><div><span class="hub-eyebrow">Supervised evolution</span><h3>改进提案</h3><p>所有变更先评估、再人工批准，并以新版本应用。</p></div></header>${renderImprovementProposals(data.proposals)}</section>`;
    else content.innerHTML = `<section class="hub-card hub-coming"><span class="hub-eyebrow">${escapeHtml(sections[active])}</span><h3>Policy-ready workspace</h3><p>This surface is ready for backend data and actions. Empty state is intentional while commands are being connected.</p></section>`;
    const error = root.querySelector("[data-hub-errors]");
    error.textContent = data.errors.length ? `Backend not connected: ${data.errors.join(" · ")}` : "Capability services online";
    error.classList.toggle("is-warning", data.errors.length > 0);
  };
  root.addEventListener("click", async (event) => {
    const tab = event.target.closest("[data-hub-section]");
    if (tab) { active = tab.dataset.hubSection; render(); }
    const importButton = event.target.closest("[data-import-mode]");
    if (importButton) root.querySelector("[data-import-state]").textContent = `${label(importButton.dataset.importMode)} selected — awaiting source input.`;
    if (event.target.closest("[data-open-import]")) root.querySelector("[data-import-wizard]")?.scrollIntoView({ behavior: "smooth" });
    if (event.target.closest("[data-preview-mcp-import]")) {
      const preview = await call("preview_mcp_import", { sourceName: root.querySelector("[data-mcp-source-name]").value, content: root.querySelector("[data-mcp-import-content]").value });
      root.querySelector("[data-mcp-import-preview]").innerHTML = renderMcpImportWorkspace(preview);
      root.querySelector("[data-import-state]").textContent = `${preview.servers?.length || 0} servers · ${preview.secrets?.length || 0} secrets`;
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
