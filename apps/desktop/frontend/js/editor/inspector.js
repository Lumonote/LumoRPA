// Step selection + the right-rail inspector (schema-aware property editor).

import { $, $$, html, toast } from "../dom.js";
import { state } from "../state.js";
import { extractSteps, findStepByPath, mutateStepInSource, parseYaml } from "../yaml.js";
import { loadSchema, renderSchemaFields, readInspectorWith } from "./schema.js";
import { renderActiveView } from "./render.js";
import { renderGraph } from "./graph.js";
import { renderTree } from "./tree.js";
import { runStep } from "../runs.js";

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
    <div id="schemaFields"><em style="color: var(--faint); font-size: 11px">加载 schema…</em></div>
    <button class="primary" id="applyStepBtn" style="margin-top: 8px">将变更写入 YAML</button>
    <div class="hint">编辑后点击 "写入 YAML"。Code 视图为权威源；此面板是 schema-aware 辅助。</div>
    <button id="runThisStepBtn" style="margin-top: 6px">▷ 单独运行此节点</button>
  </div>`;

  if (step.action) {
    try {
      const schema = await loadSchema(step.action);
      const fields = renderSchemaFields(schema, step.with || {});
      $("schemaFields").innerHTML = fields || `<em style="color: var(--faint); font-size: 11px">该动作未声明 properties</em>`;
    } catch (e) {
      $("schemaFields").innerHTML = `<em style="color: var(--bad); font-size: 11px">schema 加载失败: ${html(String(e))}</em>`;
    }
  } else {
    $("schemaFields").innerHTML = "";
  }

  $("applyStepBtn").addEventListener("click", () => applyInspectorEdits(step));
  $("runThisStepBtn").addEventListener("click", () => runStep(step.id));
}

function applyInspectorEdits(step) {
  const newId = $$('[data-prop="id"]')[0]?.value?.trim() || step.id;
  const newAction = $$('[data-prop="action"]')[0]?.value?.trim() || step.action;
  const newWith = readInspectorWith();
  // Locate the step block in the YAML by `- id: <step.id>` and rewrite the
  // shallow scalars + `with:` block in place. This is a "good-enough" textual
  // mutation that keeps comments and unknown keys intact.
  const updated = mutateStepInSource(state.source, step.id, { id: newId, action: newAction, with: newWith });
  state.source = updated;
  state.ast = parseYaml(state.source);
  state.selectedStepId = newId;
  toast("已应用到 YAML 缓冲区", "记得点 💾 保存", "ok");
  renderActiveView();
  renderInspector();
}
