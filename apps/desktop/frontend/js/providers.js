// Provider (model source) CRUD panel: list, editor form, headers editor, test.

import { $, html, toast } from "./dom.js";
import { call } from "./api.js";
import { state } from "./state.js";

export async function refreshProviders() {
  state.providers = await call("provider_status");
  renderProviderList();
  refreshActiveProviderPill();
}

export function refreshActiveProviderPill() {
  const pill = $("activeProviderPill");
  if (!state.providers) return;
  const active = state.providers.active;
  pill.querySelector(".dot")?.style?.setProperty("background", active ? "var(--ok)" : "var(--warn)");
  pill.lastChild.textContent = active ? `模型源 · ${active}` : "未激活模型源";
  // Net pill
  const net = $("netPill");
  net.querySelector(".dot")?.style?.setProperty("background", state.providers.networkEnabled ? "var(--ok)" : "var(--warn)");
  net.classList.toggle("warn", !state.providers.networkEnabled);
  net.lastChild.textContent = state.providers.networkEnabled ? "LLM 网络: 已开启" : "LLM 网络: 未开启";
}

export function renderProviderList() {
  const list = $("providerList");
  const profiles = state.providers?.profiles || [];
  if (!profiles.length) {
    list.innerHTML = `<div class="prop-empty">尚未配置模型源。点击右上角 "+ 新增" 或 "重置为默认"。</div>`;
    return;
  }
  list.innerHTML = profiles
    .map((p) => {
      const isActive = state.providers.active === p.name;
      const keyState = p.hasKey ? '<span class="status-badge ready">key ✓</span>' : '<span class="status-badge partial">key ✗</span>';
      return `<div class="provider-card ${isActive ? "is-active" : ""}">
        <div class="provider-card-head">
          <span class="name">${html(p.name)}</span>
          ${isActive ? '<span class="status-badge ready">active</span>' : ""}
          ${keyState}
          <span class="meta">${html(p.kind)}${p.wireApi ? ` / ${p.wireApi}` : ""}</span>
        </div>
        <div class="provider-card-row"><span>default</span><span>${html(p.defaultModel || "—")}</span></div>
        <div class="provider-card-row"><span>base_url</span><span>${html(p.baseUrl || "—")}</span></div>
        <div class="provider-card-row"><span>api_key_env</span><span>${html(p.apiKeyEnv || (p.hasInlineKey ? "(inline)" : "—"))}</span></div>
        <div class="provider-card-actions">
          <button data-edit-provider="${html(p.name)}">编辑</button>
          <button data-use-provider="${html(p.name)}">设为默认</button>
          <button class="danger" data-remove-provider="${html(p.name)}">删除</button>
        </div>
      </div>`;
    })
    .join("");
  list.querySelectorAll("[data-edit-provider]").forEach((b) =>
    b.addEventListener("click", () => openProviderEditor(b.dataset.editProvider))
  );
  list.querySelectorAll("[data-use-provider]").forEach((b) =>
    b.addEventListener("click", async () => {
      try {
        state.providers = await call("use_provider", { name: b.dataset.useProvider });
        renderProviderList();
        refreshActiveProviderPill();
        toast("已切换默认模型源", b.dataset.useProvider, "ok");
      } catch (e) { toast("切换失败", String(e), "bad"); }
    })
  );
  list.querySelectorAll("[data-remove-provider]").forEach((b) =>
    b.addEventListener("click", async () => {
      if (!confirm(`删除模型源 "${b.dataset.removeProvider}"?`)) return;
      try {
        state.providers = await call("remove_provider", { name: b.dataset.removeProvider });
        renderProviderList();
        refreshActiveProviderPill();
        toast("已删除", b.dataset.removeProvider, "ok");
      } catch (e) { toast("删除失败", String(e), "bad"); }
    })
  );
}

export function openProviderEditor(name) {
  const existing = state.providers.profiles.find((p) => p.name === name);
  state.providerDraft = existing
    ? JSON.parse(JSON.stringify(existing))
    : {
        name: "",
        kind: "openai",
        wireApi: "chat",
        baseUrl: "",
        apiKey: "",
        apiKeyEnv: "",
        defaultModel: "",
        reasoningEffort: "",
        models: [],
        headers: {},
        notes: "",
      };
  renderProviderEditor();
}

function renderProviderEditor() {
  const d = state.providerDraft;
  $("providerEditorTitle").textContent = d?.name ? `编辑 · ${d.name}` : "新建模型源";
  if (!d) {
    $("providerEditBody").innerHTML = `<div class="prop-empty">从左侧选择一个模型源，或点击右上角"+ 新增"</div>`;
    return;
  }
  $("providerEditBody").innerHTML = `
    <div class="row">
      <div class="field"><label>name</label><input id="pName" value="${html(d.name)}" placeholder="如 deepseek / claude / azure-east" /></div>
      <div class="field"><label>kind</label>
        <select id="pKind">
          <option value="openai" ${d.kind === "openai" ? "selected" : ""}>OpenAI 兼容</option>
          <option value="anthropic" ${d.kind === "anthropic" ? "selected" : ""}>Anthropic</option>
        </select>
      </div>
    </div>
    <div class="row three">
      <div class="field"><label>wire_api</label>
        <select id="pWire">
          <option value="">—</option>
          <option value="chat" ${d.wireApi === "chat" ? "selected" : ""}>chat (/chat/completions)</option>
          <option value="responses" ${d.wireApi === "responses" ? "selected" : ""}>responses (Responses API)</option>
        </select>
      </div>
      <div class="field"><label>default_model</label><input id="pModel" value="${html(d.defaultModel || "")}" placeholder="gpt-4o-mini / claude-opus-4-7" /></div>
      <div class="field"><label>reasoning_effort</label>
        <select id="pEffort">
          <option value="">—</option>
          ${["low", "medium", "high"].map((v) => `<option ${d.reasoningEffort === v ? "selected" : ""}>${v}</option>`).join("")}
        </select>
      </div>
    </div>
    <div class="field"><label>base_url</label><input id="pBase" value="${html(d.baseUrl || "")}" placeholder="https://api.example.com/v1" /></div>
    <div class="row">
      <div class="field"><label>api_key_env</label><input id="pEnv" value="${html(d.apiKeyEnv || "")}" placeholder="如 OPENAI_API_KEY" /></div>
      <div class="field"><label>api_key (内联，谨慎)</label><input type="password" id="pInline" value="${html(d.apiKey || "")}" placeholder="留空优先使用环境变量" /></div>
    </div>
    <div class="field"><label>models (可选 · 逗号分隔)</label><input id="pModels" value="${html((d.models || []).join(", "))}" placeholder="gpt-4o, gpt-4o-mini" /></div>
    <div class="field">
      <label>额外 headers</label>
      <div class="headers-editor" id="pHeaders"></div>
      <button id="addHeaderBtn" style="margin-top: 4px">+ 添加 header</button>
    </div>
    <div class="field"><label>备注</label><textarea id="pNotes" style="min-height: 50px">${html(d.notes || "")}</textarea></div>
    <label class="toggle"><input type="checkbox" id="pActivate" ${d.name === state.providers?.active ? "checked" : ""}/> 保存后设为默认</label>
  `;
  renderHeadersEditor();
  $("addHeaderBtn").addEventListener("click", () => {
    const k = prompt("Header 名");
    if (!k) return;
    state.providerDraft.headers[k] = "";
    renderHeadersEditor();
  });
}

function renderHeadersEditor() {
  const wrap = $("pHeaders");
  const entries = Object.entries(state.providerDraft.headers || {});
  if (!entries.length) {
    wrap.innerHTML = `<div style="font-size: 11px; color: var(--faint)">尚无 header</div>`;
    return;
  }
  wrap.innerHTML = entries
    .map(
      ([k, v], i) => `<div class="header-row">
      <input value="${html(k)}" data-hk="${i}" />
      <input value="${html(v)}" data-hv="${i}" />
      <button class="icon danger" data-hd="${i}">×</button>
    </div>`
    )
    .join("");
  wrap.querySelectorAll("[data-hd]").forEach((b) =>
    b.addEventListener("click", () => {
      const idx = Number(b.dataset.hd);
      const key = Object.keys(state.providerDraft.headers)[idx];
      delete state.providerDraft.headers[key];
      renderHeadersEditor();
    })
  );
  wrap.querySelectorAll("[data-hk]").forEach((inp) =>
    inp.addEventListener("change", () => {
      const idx = Number(inp.dataset.hk);
      const entries = Object.entries(state.providerDraft.headers);
      const [oldK, oldV] = entries[idx];
      const newK = inp.value.trim();
      if (newK && newK !== oldK) {
        delete state.providerDraft.headers[oldK];
        state.providerDraft.headers[newK] = oldV;
        renderHeadersEditor();
      }
    })
  );
  wrap.querySelectorAll("[data-hv]").forEach((inp) =>
    inp.addEventListener("change", () => {
      const idx = Number(inp.dataset.hv);
      const key = Object.keys(state.providerDraft.headers)[idx];
      state.providerDraft.headers[key] = inp.value;
    })
  );
}

function collectProviderDraft() {
  const d = state.providerDraft;
  d.name = $("pName").value.trim();
  d.kind = $("pKind").value;
  d.wireApi = $("pWire").value || null;
  d.defaultModel = $("pModel").value.trim();
  d.reasoningEffort = $("pEffort").value || null;
  d.baseUrl = $("pBase").value.trim();
  d.apiKeyEnv = $("pEnv").value.trim();
  d.apiKey = $("pInline").value;
  d.models = $("pModels").value.split(",").map((s) => s.trim()).filter(Boolean);
  d.notes = $("pNotes").value;
  d.activate = $("pActivate").checked;
  return d;
}

export async function saveProvider() {
  if (!state.providerDraft) return;
  const draft = collectProviderDraft();
  if (!draft.name) { toast("名称不能为空", "", "warn"); return; }
  try {
    state.providers = await call("save_provider", { profile: draft });
    renderProviderList();
    refreshActiveProviderPill();
    toast("已保存模型源", draft.name, "ok");
  } catch (e) {
    toast("保存失败", String(e), "bad");
  }
}

export async function testProvider() {
  if (!state.providerDraft?.name) { toast("先保存 / 选择模型源", "", "warn"); return; }
  const r = await call("test_provider", { name: state.providerDraft.name });
  if (r.ok) {
    toast("✓ 测试通过", `${r.provider}/${r.model} · ${r.inputTokens}↑/${r.outputTokens}↓`, "ok");
  } else {
    toast("✗ 测试失败", r.error || "unknown", "bad");
  }
}
