// Provider (model source) CRUD panel: list, editor form, headers editor, test.

import { $, html, toast } from "./dom.js";
import { call } from "./api.js";
import { state } from "./state.js";

export async function refreshProviders() {
  const [providers, ocrModels] = await Promise.all([
    call("provider_status"),
    call("list_ocr_models").catch(() => []),
  ]);
  state.providers = providers;
  state.ocrModels = ocrModels;
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
  net.title = state.providers.networkEnabled ? "LLM 网络本次会话已开启" : "点击开启本次会话 LLM 网络";
  net.lastChild.textContent = state.providers.networkEnabled ? "LLM 网络: 已开启" : "LLM 网络: 未开启";
  const enableBtn = $("enableLlmNetworkBtn");
  if (enableBtn) {
    enableBtn.disabled = !!state.providers.networkEnabled;
    enableBtn.textContent = state.providers.networkEnabled ? "LLM 网络已开启" : "启用本次会话网络";
  }
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
      const keyState = p.kind === "local"
        ? '<span class="status-badge ready">local</span>'
        : p.hasKey ? '<span class="status-badge ready">key ✓</span>' : '<span class="status-badge partial">key ✗</span>';
      return `<div class="provider-card ${isActive ? "is-active" : ""}">
        <div class="provider-card-head">
          <span class="name">${html(p.name)}</span>
          ${isActive ? '<span class="status-badge ready">active</span>' : ""}
          ${keyState}
          <span class="meta">${html(p.kind)}${p.wireApi ? ` / ${p.wireApi}` : ""}</span>
        </div>
        <div class="provider-card-row"><span>default</span><span>${html(p.defaultModel || "—")}</span></div>
        <div class="provider-card-row"><span>vision</span><span>${html(p.visionModel || "—")}</span></div>
        <div class="provider-card-row"><span>ocr</span><span>${html(p.ocrModel || "—")}</span></div>
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
  const profiles = state.providers?.profiles || [];
  const existing = profiles.find((p) => p.name === name);
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
        visionModel: "",
        ocrModel: "",
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
          <option value="local" ${d.kind === "local" ? "selected" : ""}>Local OCR</option>
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
    <div class="row">
      <div class="field"><label>vision_model</label><input id="pVisionModel" value="${html(d.visionModel || "")}" placeholder="gpt-4o / claude-sonnet-4-6" /></div>
      <div class="field"><label>ocr_model</label><input id="pOcrModel" value="${html(d.ocrModel || "")}" placeholder="gpt-4o / modelscope/ZhipuAI/GLM-OCR" /></div>
    </div>
    ${renderOcrModelPicker(d.ocrModel || "")}
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
  wireOcrModelPicker();
  $("addHeaderBtn").addEventListener("click", () => {
    const k = prompt("Header 名");
    if (!k) return;
    state.providerDraft.headers[k] = "";
    renderHeadersEditor();
  });
}

function renderOcrModelPicker(selected) {
  const models = state.ocrModels || [];
  if (!models.length) {
    return `<div class="ocr-model-panel"><div class="prop-empty">OCR 模型列表不可用</div></div>`;
  }
  return `<div class="ocr-model-panel">
    <div class="ocr-model-panel-head">
      <span>ModelScope OCR 模型</span>
      <button type="button" data-refresh-ocr-models>刷新</button>
    </div>
    <div class="ocr-model-grid">
      ${models.map((m) => {
        const p = m.preset;
        const isSelected = selected === p.id || selected === p.repo;
        return `<div class="ocr-model-card ${isSelected ? "is-selected" : ""}">
          <div class="ocr-model-title">
            <span>${html(p.label)}</span>
            ${m.downloaded ? '<span class="status-badge ready">已下载</span>' : '<span class="status-badge partial">未下载</span>'}
            ${p.recommended ? '<span class="status-badge ready">推荐</span>' : ""}
          </div>
          <div class="ocr-model-meta">${html(p.repo)} · ${html(p.engine)} · ${html(p.sizeHint || "VLM")}</div>
          <div class="ocr-model-desc">${html(p.description)}</div>
          <div class="ocr-model-path">${html(m.cacheDir)}</div>
          <div class="ocr-model-actions">
            <button type="button" data-set-ocr-model="${html(p.id)}">选择</button>
            <button type="button" ${m.downloaded ? "disabled" : ""} data-download-ocr-model="${html(p.id)}">${m.downloaded ? "已下载" : "下载"}</button>
          </div>
        </div>`;
      }).join("")}
    </div>
  </div>`;
}

function wireOcrModelPicker() {
  $("providerEditBody").querySelectorAll("[data-set-ocr-model]").forEach((btn) =>
    btn.addEventListener("click", () => {
      const model = btn.dataset.setOcrModel;
      $("pOcrModel").value = model;
      state.providerDraft.ocrModel = model;
      if ($("pKind").value === "openai" && !$("pBase").value && !$("pModel").value) {
        $("pKind").value = "local";
        state.providerDraft.kind = "local";
      }
      toast("已选择 OCR 模型", model, "ok");
    })
  );
  $("providerEditBody").querySelectorAll("[data-download-ocr-model]").forEach((btn) =>
    btn.addEventListener("click", async () => {
      const model = btn.dataset.downloadOcrModel;
      collectProviderDraft();
      btn.disabled = true;
      btn.textContent = "下载中";
      try {
        const result = await call("download_ocr_model", { model });
        state.ocrModels = await call("list_ocr_models").catch(() => state.ocrModels || []);
        renderProviderEditor();
        toast("OCR 模型已下载", result.model?.cacheDir || model, "ok");
      } catch (e) {
        btn.disabled = false;
        btn.textContent = "下载";
        toast("下载失败", String(e), "bad");
      }
    })
  );
  $("providerEditBody").querySelectorAll("[data-refresh-ocr-models]").forEach((btn) =>
    btn.addEventListener("click", async () => {
      collectProviderDraft();
      state.ocrModels = await call("list_ocr_models").catch(() => state.ocrModels || []);
      renderProviderEditor();
      toast("OCR 模型状态已刷新", "", "ok");
    })
  );
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
  d.visionModel = $("pVisionModel").value.trim();
  d.ocrModel = $("pOcrModel").value.trim();
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

export async function enableLlmNetworkForSession() {
  state.providers = await call("enable_llm_network_for_session");
  renderProviderList();
  refreshActiveProviderPill();
  toast("已启用本次会话 LLM 网络", "当前应用进程内有效，无需重启。", "ok");
}

export async function testProvider() {
  if (!state.providerDraft) { toast("先保存 / 选择模型源", "", "warn"); return; }
  const draft = collectProviderDraft();
  if (!draft.name) { toast("先保存 / 选择模型源", "", "warn"); return; }
  if (draft.kind === "local") {
    toast("本地 OCR 无需连接测试", "选择并下载 OCR 模型后，运行 image.ocr 流程即可验证。", "ok");
    return;
  }
  if (state.providers && !state.providers.networkEnabled) {
    await enableLlmNetworkForSession();
  }
  const r = await call("test_provider", { name: draft.name });
  if (r.ok) {
    toast("✓ 测试通过", `${r.provider}/${r.model} · ${r.inputTokens}↑/${r.outputTokens}↓`, "ok");
  } else {
    toast("✗ 测试失败", r.error || "unknown", "bad");
  }
}
