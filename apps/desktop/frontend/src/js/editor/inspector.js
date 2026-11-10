// Step selection + the right-rail inspector (schema-aware property editor).

import { $, html, toast } from "../dom.js";
import { state } from "../state.js";
import { extractSteps, findStepByPath, mutateStepAtPath, parseYaml, pathKey, stepIdChain, walkSteps } from "../yaml.js";
import { loadSchema, renderSchemaFields } from "./schema.js";
import { applyAdvancedValues, readAdvancedFields, renderAdvancedFields, wireAdvancedFields } from "./advanced-fields.js";
import { ACTION_PRESETS } from "../constants.js";
import { renderActiveView } from "./render.js";
import { renderGraph } from "./graph.js";
import { renderTree } from "./tree.js";
import { runStep } from "../runs.js";
import { isBreakpoint, toggleBreakpoint } from "../debug.js";
import { snapshotSource } from "./history.js";

let inspectorRenderToken = 0;

export function selectStep(path) {
  state.selectedStepPath = path;
  const step = path ? findStepByPath(extractSteps(state.ast), path) : null;
  state.selectedStepId = step?.id || null;
  if (state.viewMode === "graph") renderGraph();
  if (state.viewMode === "tree") renderTree();
  renderInspector();
}

export async function renderInspector() {
  const renderToken = ++inspectorRenderToken;
  const body = $("inspectorBody");
  const path = state.selectedStepPath ? [...state.selectedStepPath] : null;
  if (!path) {
    body.innerHTML = `<div class="prop-empty">选择一个节点查看属性</div>`;
    return;
  }
  const step = findStepByPath(extractSteps(state.ast), path);
  if (!step) {
    body.innerHTML = `<div class="prop-empty">该节点已不存在</div>`;
    return;
  }
  const bpKey = stepIdChain(extractSteps(state.ast), path) || step.id;
  const source = state.source;
  const flowPath = state.flowPath;
  const isCurrent = () => renderToken === inspectorRenderToken && state.source === source && state.flowPath === flowPath && pathKey(state.selectedStepPath) === pathKey(path);
  const isNested = bpKey !== step.id;
  const bpOn = isBreakpoint(bpKey);
  const presets = ACTION_PRESETS[step.action] || [];
  const presetBlock = presets.length
    ? `<div class="prop-field">
        <label>插入预设</label>
        <select id="presetSelect">
          <option value="">— 选择场景预设 —</option>
          ${presets.map((p, i) => `<option value="${i}">${html(p.name)}</option>`).join("")}
        </select>
        <span class="hint">选择后填充参数，再应用节点设置。</span>
      </div>`
    : "";
  body.innerHTML = `<div class="prop-form" id="propForm">
    <div class="prop-field">
      <label>Step ID</label>
      <input data-prop="id" value="${html(step.id || "")}" />
    </div>
    <div class="prop-field">
      <label>Action</label>
      <input data-prop="action" value="${html(step.action || "")}" />
      <span class="hint">家族：<strong>${html((step.action || "").split(".")[0] || "?")}</strong></span>
    </div>
    <div class="section-title">with: 参数（按 Action JSON Schema）</div>
    ${presetBlock}
    <div id="schemaFields"><em style="color: var(--faint); font-size: 11px">加载 schema…</em></div>
    ${renderAdvancedFields(step)}
    <div class="hint" role="alert" data-inspector-error hidden></div>
    <button class="primary" id="applyStepBtn" style="margin-top: 8px" disabled>应用节点设置</button>
    <div class="hint">应用后保存流程，即可在下次运行中使用。</div>
    <button id="runThisStepBtn" style="margin-top: 6px">▷ 单独运行此节点</button>
    <button id="toggleBpBtn" class="${bpOn ? "bp-on" : ""}" style="margin-top: 6px">${bpOn ? "🔴 断点已设 · 点此取消" : "⚪ 设为断点 (F-20)"}</button>
    <div class="hint">断点：调试运行会在此节点<strong>执行前</strong>暂停。${isNested ? `嵌套节点按完整路径 <code>${html(bpKey)}</code> 设断点。` : "顶层节点：id 即路径。"}</div>
  </div>`;
  const form = $("propForm");
  const applyButton = $("applyStepBtn");
  const fields = $("schemaFields");
  const actionControl = form.querySelector('[data-prop="action"]');
  const errorBox = form.querySelector("[data-inspector-error]");
  let schemaAction = null;
  let schema = null;
  let schemaRequest = 0;
  const setError = (error) => {
    errorBox.textContent = error ? String(error.message || error) : "";
    errorBox.hidden = !error;
  };
  wireAdvancedFields(form);
  const refreshSchema = async (action, parameters) => {
    const request = ++schemaRequest;
    schemaAction = null;
    applyButton.disabled = true;
    setError(null);
    try {
      if (!action) throw new Error("动作名称不能为空");
      const loaded = await loadSchema(action);
      if (!isCurrent() || request !== schemaRequest || actionControl.value.trim() !== action) return;
      schema = loaded;
      schemaAction = action;
      fields.innerHTML = renderSchemaFields(schema, parameters) || `<em style="color: var(--faint); font-size: 11px">该动作没有参数</em>`;
      applyButton.disabled = false;
    } catch (e) {
      if (!isCurrent() || request !== schemaRequest) return;
      fields.innerHTML = `<em style="color: var(--bad); font-size: 11px">参数加载失败: ${html(String(e))}</em>`;
      setError("参数加载失败，请重新选择节点后重试");
    }
  };
  actionControl.addEventListener("change", () => {
    const action = actionControl.value.trim();
    return refreshSchema(action, action === step.action ? step.with || {} : {});
  });
  const presetSelect = $("presetSelect");
  if (presetSelect) presetSelect.addEventListener("change", (event) => {
    try {
      if (!isCurrent() || !schemaAction || schemaAction !== step.action) return;
      const preset = presets[Number(event.currentTarget.value)];
      if (!preset || event.currentTarget.value === "") return;
      const merged = { ...readParameters(form, step.with || {}, schema), ...preset.with };
      fields.innerHTML = renderSchemaFields(schema, merged);
      event.currentTarget.value = "";
    } catch (error) { setError(error); }
  });

  applyButton.addEventListener("click", () => {
    try {
      if (!isCurrent()) throw new Error("流程内容或选中节点已变化，请重新选择节点后应用");
      if (!schemaAction || schemaAction !== actionControl.value.trim()) throw new Error("请等待当前动作参数加载完成");
      const newId = form.querySelector('[data-prop="id"]').value.trim();
      if (!newId) throw new Error("节点 ID 不能为空");
      let duplicate = false;
      walkSteps(extractSteps(state.ast), (candidate, info) => {
        if (candidate.id === newId && pathKey(info.path) !== pathKey(path)) duplicate = true;
      });
      if (newId !== step.id && duplicate) throw new Error("该节点 ID 已被使用，请选择其他名称");
      const original = schemaAction === step.action ? step.with || {} : {};
      const next = applyAdvancedValues({ ...step, id: newId, action: schemaAction, with: readParameters(form, original, schema) }, readAdvancedFields(form));
      const updated = mutateStepAtPath(state.source, path, next);
      const ast = parseYaml(updated);
      snapshotSource("inspector");
      state.source = updated;
      state.ast = ast;
      state.selectedStepId = newId;
      state.draftController?.capture();
      toast("节点设置已应用", "保存流程后生效", "ok");
      renderActiveView();
      renderInspector();
    } catch (error) { setError(error); }
  });
  $("runThisStepBtn").addEventListener("click", () => runStep(step.id));
  $("toggleBpBtn").addEventListener("click", (e) => {
    const on = toggleBreakpoint(bpKey);
    const btn = e.currentTarget;
    btn.classList.toggle("bp-on", on);
    btn.textContent = on ? "🔴 断点已设 · 点此取消" : "⚪ 设为断点 (F-20)";
  });
  await refreshSchema(step.action, step.with || {});
}

function readParameters(root, original, schema) {
  // Only rendered keys are edited; extra parameters remain intact.
  const result = { ...original };
  for (const field of root.querySelectorAll("[data-with-key]")) {
    const key = field.dataset.withKey;
    const type = field.dataset.type;
    if (type === "boolean") result[key] = field.checked;
    else if (field.value === "") delete result[key];
    else if (type === "integer" || type === "number") {
      const value = Number(field.value);
      if (!Number.isFinite(value) || (type === "integer" && !Number.isSafeInteger(value))) throw new Error(`${key}必须是有效的${type === "integer" ? "整数" : "数字"}`);
      result[key] = value;
    } else if (type === "object" || type === "array") {
      let value;
      try { value = JSON.parse(field.value); }
      catch { throw new Error(`${key}必须是有效的 JSON`); }
      if (value === null || (type === "array" ? !Array.isArray(value) : typeof value !== "object" || Array.isArray(value))) throw new Error(`${key}必须是${type === "array" ? "数组" : "对象"}`);
      result[key] = value;
    } else result[key] = field.value;
  }
  for (const key of schema?.required || []) {
    if (result[key] === undefined || result[key] === null || result[key] === "") throw new Error(`${key}不能为空`);
  }
  return result;
}
