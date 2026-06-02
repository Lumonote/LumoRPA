// Steps view: YingDao-style linear flowchart with inline expand + drag/drop
// reorder and drag-to-insert from the action library.

import { $, html, toast } from "../dom.js";
import { state } from "../state.js";
import {
  extractSteps, findStepByPath, walkSteps, pathKey, parsePathKey, cssEscape,
  mutateStepInSource, parseYaml,
} from "../yaml.js";
import { zhAction, AI_LABEL } from "../constants.js";
import { renderWithSummary } from "./graph.js";
import { loadSchema, renderSchemaFields, readWithFromContainer } from "./schema.js";
import { renderActiveView } from "./render.js";
import { selectStep } from "./inspector.js";
import { openAiDrawer } from "./ai-drawer.js";
import {
  appendStepToSource, insertStepAfterPath, insertNewStepNear,
  deleteStepByPath, moveStepBefore,
} from "./mutations.js";

export function renderStepList() {
  const root = $("stepList");
  root.innerHTML = "";
  if (!state.ast) {
    root.innerHTML = `
      <div class="flow-dropzone flow-dropzone-empty" id="emptyDropZone">
        <div class="flow-dropzone-icon">🧩</div>
        <div class="flow-dropzone-title">从左侧指令面板 <strong>拖拽指令</strong> 到这里开始设计流程</div>
        <div class="flow-dropzone-sub">支持鼠标拖动重排、AI 模式一键开启、嵌套循环 / 条件分支</div>
        <div class="step-add-row" style="margin-top:14px"><button id="addFirstStepBtn">+ 或点此创建第一个步骤</button></div>
      </div>`;
    $("addFirstStepBtn")?.addEventListener("click", () => appendStepToSource("control.log"));
    bindFlowDropZone($("emptyDropZone"));
    return;
  }
  const steps = extractSteps(state.ast);
  if (!steps.length) {
    root.innerHTML = `
      <div class="flow-dropzone flow-dropzone-empty" id="emptyDropZone">
        <div class="flow-dropzone-icon">🧩</div>
        <div class="flow-dropzone-title">把左侧动作 <strong>拖到这里</strong> 组装流程</div>
        <div class="flow-dropzone-sub">或点击下方 + 立即添加一个 control.log 占位步骤</div>
        <div class="step-add-row" style="margin-top:14px"><button id="addFirstStepBtn">+ 添加步骤</button></div>
      </div>`;
    $("addFirstStepBtn")?.addEventListener("click", () => appendStepToSource("control.log"));
    bindFlowDropZone($("emptyDropZone"));
    return;
  }
  const items = [];
  walkSteps(steps, (step, info) => items.push({ step, ...info }));

  // Build flowchart: START → row → connector → row → connector ... → END
  const parts = [`<div class="flow-node flow-node-terminal flow-node-start">▶ 开始</div>`];
  parts.push(`<div class="flow-connector"><span class="flow-arrow">▼</span></div>`);
  items.forEach((it, i) => {
    parts.push(renderStepRow(it, i));
    parts.push(`<div class="flow-connector" data-insert-after="${html(pathKey(it.path))}" title="拖动作到这里 = 在此处插入">
      <span class="flow-arrow">▼</span>
      <button class="flow-insert-here" data-insert-after-btn="${html(pathKey(it.path))}" title="在此处插入步骤">+</button>
    </div>`);
  });
  parts.push(`<div class="flow-node flow-node-terminal flow-node-end">■ 结束</div>`);
  parts.push(`<div class="step-add-row"><button id="addStepBottomBtn">+ 末尾添加步骤</button></div>`);

  root.innerHTML = parts.join("");
  bindStepListEvents();
  bindFlowDropZone(root);
}

export function bindFlowDropZone(zone) {
  if (!zone) return;
  zone.addEventListener("dragover", (e) => {
    if (e.dataTransfer.types.includes("text/x-lumo-action")) {
      e.preventDefault();
      zone.classList.add("is-drop-hover");
    }
  });
  zone.addEventListener("dragleave", () => zone.classList.remove("is-drop-hover"));
  zone.addEventListener("drop", (e) => {
    zone.classList.remove("is-drop-hover");
    const actionId = e.dataTransfer.getData("text/x-lumo-action");
    if (actionId) { e.preventDefault(); appendStepToSource(actionId); }
  });
}

function renderStepRow({ step, depth, path }, idx) {
  const ai = step.ai || {};
  const aiMode = (ai.mode || "off").toLowerCase();
  const selectedKey = pathKey(state.selectedStepPath || []);
  const myKey = pathKey(path);
  const selected = selectedKey === myKey;
  const family = (step.action || "misc").split(".")[0];
  const zh = zhAction(step.action);
  const summary = renderWithSummary(step.with);
  const indent = depth * 16 + 4;
  return `<div class="step-row family-${html(family)} ${selected ? "is-selected" : ""}"
              data-step-path="${html(myKey)}" data-step-idx="${idx}"
              draggable="true"
              style="padding-left: ${indent}px">
    <span class="step-handle" title="拖动重排（同级）">⋮⋮</span>
    <span class="step-num">${idx + 1}</span>
    <span class="step-id" title="${html(step.id || '')}">${html(step.id || "(no id)")}</span>
    <span class="step-action" title="${html(step.action || '?')}">
      <span class="step-action-zh">${html(zh.label)}</span>
      <span class="step-action-code">${html(step.action || "?")}</span>
    </span>
    <span class="step-summary">${summary || '<em style="color: var(--faint)">无参数</em>'}</span>
    <button class="step-ai-btn ai-state-${html(aiMode)}" data-ai-toggle="${html(myKey)}"
            title="AI 模式：${html(AI_LABEL[aiMode] || aiMode)} · 点击打开 AI 抽屉">✨</button>
    <button class="step-icon-btn step-expand-btn" data-expand="${html(myKey)}" title="展开配置">⤢</button>
    <button class="step-icon-btn step-insert-btn" data-insert="${html(myKey)}" title="在此后插入">+</button>
    <button class="step-icon-btn step-del-btn" data-del="${html(myKey)}" title="删除">×</button>
    <div class="step-expand-body" data-expand-body="${html(myKey)}" hidden></div>
  </div>`;
}

function bindStepListEvents() {
  const root = $("stepList");
  // Container drop: action library → append new step at end
  root.addEventListener("dragover", (e) => {
    if (e.dataTransfer.types.includes("text/x-lumo-action")) e.preventDefault();
  });
  root.addEventListener("drop", (e) => {
    if (e.target !== root) return; // children handle their own
    const actionId = e.dataTransfer.getData("text/x-lumo-action");
    if (actionId) { e.preventDefault(); appendStepToSource(actionId); }
  });
  // Flow-connector insert buttons (click + between steps)
  root.querySelectorAll("[data-insert-after-btn]").forEach((b) => {
    b.addEventListener("click", (e) => {
      e.stopPropagation();
      insertStepAfterPath(parsePathKey(b.dataset.insertAfterBtn));
    });
  });
  // Drop on connector = insert action there
  root.querySelectorAll(".flow-connector[data-insert-after]").forEach((c) => {
    c.addEventListener("dragover", (e) => {
      if (e.dataTransfer.types.includes("text/x-lumo-action")) {
        e.preventDefault();
        c.classList.add("is-drop-hover");
      }
    });
    c.addEventListener("dragleave", () => c.classList.remove("is-drop-hover"));
    c.addEventListener("drop", (e) => {
      c.classList.remove("is-drop-hover");
      const actionId = e.dataTransfer.getData("text/x-lumo-action");
      if (!actionId) return;
      e.preventDefault();
      insertStepAfterPath(parsePathKey(c.dataset.insertAfter), actionId);
    });
  });
  // Row click → select
  root.querySelectorAll(".step-row").forEach((row) => {
    row.addEventListener("click", (e) => {
      if (e.target.closest("button") || e.target.closest("[data-expand-body]")) return;
      selectStep(parsePathKey(row.dataset.stepPath));
    });
  });
  // AI ✨ → open drawer
  root.querySelectorAll("[data-ai-toggle]").forEach((b) => {
    b.addEventListener("click", (e) => {
      e.stopPropagation();
      openAiDrawer(parsePathKey(b.dataset.aiToggle));
    });
  });
  // Expand
  root.querySelectorAll("[data-expand]").forEach((b) => {
    b.addEventListener("click", (e) => {
      e.stopPropagation();
      toggleStepExpand(parsePathKey(b.dataset.expand));
    });
  });
  // Insert
  root.querySelectorAll("[data-insert]").forEach((b) => {
    b.addEventListener("click", (e) => {
      e.stopPropagation();
      insertStepAfterPath(parsePathKey(b.dataset.insert));
    });
  });
  // Delete
  root.querySelectorAll("[data-del]").forEach((b) => {
    b.addEventListener("click", (e) => {
      e.stopPropagation();
      const path = parsePathKey(b.dataset.del);
      const step = findStepByPath(extractSteps(state.ast), path);
      if (step && confirm(`删除步骤 "${step.id || step.action}"？`)) deleteStepByPath(path);
    });
  });
  // Drag / drop reorder (sibling-level only)
  let dragKey = null;
  root.querySelectorAll(".step-row").forEach((row) => {
    row.addEventListener("dragstart", (e) => {
      dragKey = row.dataset.stepPath;
      e.dataTransfer.effectAllowed = "move";
      e.dataTransfer.setData("text/x-lumo-step", dragKey);
      row.classList.add("is-dragging");
    });
    row.addEventListener("dragend", () => {
      row.classList.remove("is-dragging");
      root.querySelectorAll(".is-drop-target").forEach((r) => r.classList.remove("is-drop-target"));
    });
    row.addEventListener("dragover", (e) => {
      const srcKey = dragKey || e.dataTransfer.getData("text/x-lumo-step");
      if (!srcKey || srcKey === row.dataset.stepPath) return;
      // Only allow dropping on same-parent sibling.
      if (sameParent(parsePathKey(srcKey), parsePathKey(row.dataset.stepPath))) {
        e.preventDefault();
        row.classList.add("is-drop-target");
      }
    });
    row.addEventListener("dragleave", () => row.classList.remove("is-drop-target"));
    row.addEventListener("drop", (e) => {
      e.preventDefault();
      const srcKey = e.dataTransfer.getData("text/x-lumo-step") || dragKey;
      const dstKey = row.dataset.stepPath;
      row.classList.remove("is-drop-target");
      if (!srcKey || !dstKey || srcKey === dstKey) return;
      const srcPath = parsePathKey(srcKey);
      const dstPath = parsePathKey(dstKey);
      if (sameParent(srcPath, dstPath)) moveStepBefore(srcPath, dstPath);
    });
    // Drop from action library: insert as new step under the same parent
    row.addEventListener("drop", (e) => {
      const actionId = e.dataTransfer.getData("text/x-lumo-action");
      if (actionId) {
        e.preventDefault();
        insertNewStepNear(parsePathKey(row.dataset.stepPath), actionId);
      }
    });
  });
  // Bottom add
  $("addStepBottomBtn")?.addEventListener("click", () => appendStepToSource("control.log"));
}

function sameParent(a, b) {
  if (!a || !b || !a.length || !b.length) return true;
  return pathKey(a.slice(0, -1)) === pathKey(b.slice(0, -1));
}

async function toggleStepExpand(path) {
  const key = pathKey(path);
  const body = $("stepList").querySelector(`[data-expand-body="${cssEscape(key)}"]`);
  if (!body) return;
  if (!body.hidden) {
    body.hidden = true;
    body.innerHTML = "";
    return;
  }
  const step = findStepByPath(extractSteps(state.ast), path);
  if (!step) return;
  body.hidden = false;
  body.innerHTML = `<div class="step-expand-inner"><em style="color: var(--faint); font-size: 11px">加载 schema…</em></div>`;
  try {
    const schema = await loadSchema(step.action);
    const fields = renderSchemaFields(schema, step.with || {});
    body.querySelector(".step-expand-inner").innerHTML = `
      <div class="prop-form">
        ${fields || '<em style="color: var(--faint); font-size: 11px">该动作未声明 properties</em>'}
        <button class="primary" data-apply-expand="${html(key)}" style="margin-top: 8px">将变更写入 YAML</button>
      </div>`;
    body.querySelector("[data-apply-expand]").addEventListener("click", () => {
      const newWith = readWithFromContainer(body);
      const updated = mutateStepInSource(state.source, step.id, {
        id: step.id, action: step.action, with: newWith, ai: step.ai, retry: step.retry, when: step.when, bind: step.bind,
        do: step.do, else: step.else, catch: step.catch, finally: step.finally,
      });
      state.source = updated;
      state.ast = parseYaml(state.source);
      toast("已应用到 YAML 缓冲区", "记得点 💾 保存", "ok");
      renderActiveView();
    });
  } catch (e) {
    body.querySelector(".step-expand-inner").innerHTML = `<em style="color: var(--bad); font-size: 11px">schema 加载失败: ${html(String(e))}</em>`;
  }
}
