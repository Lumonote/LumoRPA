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
    return `<article class="hub-row"><div><strong>${escapeHtml(server.name || server.id || "Unnamed server")}</strong><span>${escapeHtml(server.description || `${server.tools ?? 0} tools available`)}</span></div><div class="hub-tags"><span class="hub-tag">${escapeHtml(String(server.transport || "unknown").toUpperCase())}</span><span class="hub-tag health-${escapeHtml(health)}">${escapeHtml(label(health))}</span></div></article>`;
  }).join("");
}

export function renderSkillRows(skills = []) {
  if (!skills.length) return emptyRow("No skills installed");
  return skills.map((skill) => `<article class="hub-row"><div><strong>${escapeHtml(skill.name || "Unnamed skill")}</strong><span>${escapeHtml(skill.description || "No description")}</span></div><span class="hub-tag">${escapeHtml(label(skill.status || "available"))}</span></article>`).join("");
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
  profiles: "Agent Profiles", permissions: "Permissions", audit: "Audit", proposals: "Improvement Proposals",
};

function overviewMarkup(data) {
  return `<div class="hub-metrics"><article><span>Connected servers</span><strong>${data.servers.length}</strong></article><article><span>Installed skills</span><strong>${data.skills.length}</strong></article><article><span>Agent profiles</span><strong>${data.profiles.length}</strong></article><article><span>Vault posture</span><strong>Protected</strong></article></div><div class="hub-grid"><section class="hub-card"><header><div><span class="hub-eyebrow">Runtime mesh</span><h3>MCP Servers</h3></div></header>${renderServerRows(data.servers)}</section><section class="hub-card"><header><div><span class="hub-eyebrow">Operator layer</span><h3>Agent Profiles</h3></div></header>${renderAgentProfileRows(data.profiles)}</section></div>`;
}

export function mountCapabilityHub({ call, root }) {
  if (!root) return { refresh: async () => {} };
  const data = { servers: [], skills: [], profiles: [], errors: [] };
  let active = "overview";
  const content = root.querySelector("[data-hub-content]");
  const render = () => {
    root.querySelectorAll("[data-hub-section]").forEach((button) => button.classList.toggle("is-active", button.dataset.hubSection === active));
    if (active === "overview") content.innerHTML = overviewMarkup(data);
    else if (active === "servers") content.innerHTML = `<section class="hub-card"><header><div><span class="hub-eyebrow">Protocol fabric</span><h3>MCP Servers</h3></div><button class="primary" data-open-import>Import server</button></header>${renderServerRows(data.servers)}</section>`;
    else if (active === "skills") content.innerHTML = `<section class="hub-card"><header><div><span class="hub-eyebrow">Local intelligence</span><h3>Skills</h3></div></header>${renderSkillRows(data.skills)}</section>`;
    else if (active === "profiles") content.innerHTML = `<section class="hub-card"><header><div><span class="hub-eyebrow">Execution policy</span><h3>Agent Profiles</h3></div></header>${renderAgentProfileRows(data.profiles)}</section>`;
    else content.innerHTML = `<section class="hub-card hub-coming"><span class="hub-eyebrow">${escapeHtml(sections[active])}</span><h3>Policy-ready workspace</h3><p>This surface is ready for backend data and actions. Empty state is intentional while commands are being connected.</p></section>`;
    const error = root.querySelector("[data-hub-errors]");
    error.textContent = data.errors.length ? `Backend not connected: ${data.errors.join(" · ")}` : "Capability services online";
    error.classList.toggle("is-warning", data.errors.length > 0);
  };
  root.addEventListener("click", (event) => {
    const tab = event.target.closest("[data-hub-section]");
    if (tab) { active = tab.dataset.hubSection; render(); }
    const importButton = event.target.closest("[data-import-mode]");
    if (importButton) root.querySelector("[data-import-state]").textContent = `${label(importButton.dataset.importMode)} selected — awaiting source input.`;
    if (event.target.closest("[data-open-import]")) root.querySelector("[data-import-wizard]")?.scrollIntoView({ behavior: "smooth" });
  });
  const refresh = async () => {
    data.errors = [];
    const requests = [["servers", "list_mcp_servers"], ["skills", "list_skills"], ["profiles", "list_agent_profiles"]];
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
