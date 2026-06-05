// Step selection + the right-rail inspector (schema-aware property editor).

import { $, $$, html, toast } from "../dom.js";
import { state } from "../state.js";
import { extractSteps, findStepByPath, mutateStepInSource, parseYaml, stepIdChain } from "../yaml.js";
import { loadSchema, renderSchemaFields, readInspectorWith } from "./schema.js";
import { ACTION_PRESETS } from "../constants.js";
import { renderActiveView } from "./render.js";
import { renderGraph } from "./graph.js";
import { renderTree } from "./tree.js";
import { runStep } from "../runs.js";
import { isBreakpoint, toggleBreakpoint } from "../debug.js";

export function selectStep(path) {
  state.selectedStepPath = path;
  const step = path ? findStepByPath(extractSteps(state.ast), path) : null;
  state.selectedStepId = step?.id || null;
  if (state.viewMode === "graph") renderGraph();
  if (state.viewMode === "tree") renderTree();
  renderInspector();
}

export async function renderInspector() {
  const body = $("inspectorBody");
  const path = state.selectedStepPath;
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
        <span class="hint">选择后仅填充表单，仍需点 "写入 YAML" 才会生效。</span>
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
    <button class="primary" id="applyStepBtn" style="margin-top: 8px">将变更写入 YAML</button>
    <div class="hint">编辑后点击 "写入 YAML"。Code 视图为权威源；此面板是 schema-aware 辅助。</div>
    <button id="runThisStepBtn" style="margin-top: 6px">▷ 单独运行此节点</button>
    <button id="toggleBpBtn" class="${bpOn ? "bp-on" : ""}" style="margin-top: 6px">${bpOn ? "🔴 断点已设 · 点此取消" : "⚪ 设为断点 (F-20)"}</button>
    <div class="hint">断点：调试运行会在此节点<strong>执行前</strong>暂停。${isNested ? `嵌套节点按完整路径 <code>${html(bpKey)}</code> 设断点。` : "顶层节点：id 即路径。"}</div>
  </div>`;

  if (step.action) {
    try {
      const schema = await loadSchema(step.action);
      const fields = renderSchemaFields(schema, step.with || {});
      $("schemaFields").innerHTML = fields || `<em style="color: var(--faint); font-size: 11px">该动作未声明 properties</em>`;
      const presetSelect = $("presetSelect");
      if (presetSelect) {
        presetSelect.addEventListener("change", (e) => {
          const idx = e.currentTarget.value;
          if (idx === "") return;
          const preset = presets[Number(idx)];
          if (!preset) return;
          // Merge the preset over whatever is currently in the form so prior
          // tweaks survive, then re-render the schema fields with the merge.
          const merged = { ...readInspectorWith(), ...preset.with };
          $("schemaFields").innerHTML =
            renderSchemaFields(schema, merged) ||
            `<em style="color: var(--faint); font-size: 11px">该动作未声明 properties</em>`;
          e.currentTarget.value = "";
        });
      }
    } catch (e) {
      $("schemaFields").innerHTML = `<em style="color: var(--bad); font-size: 11px">schema 加载失败: ${html(String(e))}</em>`;
    }
  } else {
    $("schemaFields").innerHTML = "";
  }

  $("applyStepBtn").addEventListener("click", () => applyInspectorEdits(step));
  $("runThisStepBtn").addEventListener("click", () => runStep(step.id));
  $("toggleBpBtn").addEventListener("click", (e) => {
    const on = toggleBreakpoint(bpKey);
    const btn = e.currentTarget;
    btn.classList.toggle("bp-on", on);
    btn.textContent = on ? "🔴 断点已设 · 点此取消" : "⚪ 设为断点 (F-20)";
  });
}

function applyInspectorEdits(step) {
  const newId = $$('[data-prop="id"]')[0]?.value?.trim() || step.id;
  const newAction = $$('[data-prop="action"]')[0]?.value?.trim() || step.action;
  const newWith = readInspectorWith();
  // Locate the step block in the YAML by `- id: <step.id>` and rewrite the
  // shallow scalars + `with:` block in place. We spread the full existing
  // `step` so mutateStepInSource → emitStep re-emits the node's other keys
  // (when/bind/retry/ai/do/else/catch/finally) instead of silently wiping them.
  const updated = mutateStepInSource(state.source, step.id, { ...step, id: newId, action: newAction, with: newWith });
  state.source = updated;
  state.ast = parseYaml(state.source);
  state.selectedStepId = newId;
  toast("已应用到 YAML 缓冲区", "记得点 💾 保存", "ok");
  renderActiveView();
  renderInspector();
}
