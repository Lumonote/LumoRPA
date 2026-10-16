// Step source-mutation operations (insert / delete / move / append plus
// duplicate / clipboard / batch delete). All edit `state.source` textually,
// re-parse the AST, then re-render the active view. Every mutation snapshots
// the pre-edit source into the undo history first (editor/history.js).

import { toast } from "../dom.js";
import { state } from "../state.js";
import {
  parseYaml, extractSteps, findStepByPath, pathKey,
  findStepRange, emitStep, extractStepBlock, insertStepBlocks,
  removeStepBlocks, orderPathKeysByDocument,
} from "../yaml.js";
import { renderActiveView } from "./render.js";
import { renderInspector } from "./inspector.js";
import { snapshotSource } from "./history.js";

const EMPTY_FLOW_HEADER = "apiVersion: lumorpa.io/v1\nkind: Flow\nmetadata:\n  id: untitled\n  version: 0.1.0\nspec:\n  steps:\n";
const STEP_CLIPBOARD_KEY = "lumo.stepClipboard";

function commitSource(nextSource) {
  state.source = nextSource;
  state.ast = parseYaml(state.source);
  state.selectedStepPaths?.clear();
}

export function insertStepAfterPath(path, actionId) {
  const step = findStepByPath(extractSteps(state.ast), path);
  if (!step) return;
  const lines = state.source.split(/\r?\n/);
  const range = findStepRange(lines, step.id);
  if (!range) return;
  const useAction = actionId || "control.log";
  const id = actionId
    ? `${actionId.replace(/\./g, "_")}_${Math.floor(Math.random() * 999)}`
    : `step_${Math.floor(Math.random() * 9999)}`;
  const body = actionId ? { id, action: useAction, with: {} } : { id, action: useAction, with: { message: "TODO" } };
  const block = emitStep(body, range.baseIndent).split("\n");
  const newLines = lines.slice(0, range.endIdx).concat(block).concat(lines.slice(range.endIdx));
  snapshotSource("insert");
  commitSource(newLines.join("\n"));
  toast("已插入步骤", `${id} (${useAction})`, "ok");
  renderActiveView();
}

export function insertNewStepNear(path, actionId) {
  const step = findStepByPath(extractSteps(state.ast), path);
  if (!step) { appendStepToSource(actionId); return; }
  const lines = state.source.split(/\r?\n/);
  const range = findStepRange(lines, step.id);
  if (!range) { appendStepToSource(actionId); return; }
  const id = `${actionId.replace(/\./g, "_")}_${Math.floor(Math.random() * 999)}`;
  const block = emitStep({ id, action: actionId, with: {} }, range.baseIndent).split("\n");
  const newLines = lines.slice(0, range.endIdx).concat(block).concat(lines.slice(range.endIdx));
  snapshotSource("insert");
  commitSource(newLines.join("\n"));
  toast("已添加节点", `${id} (${actionId})`, "ok");
  renderActiveView();
}

export function deleteStepByPath(path) {
  const step = findStepByPath(extractSteps(state.ast), path);
  if (!step) return;
  const lines = state.source.split(/\r?\n/);
  const range = findStepRange(lines, step.id);
  if (!range) return;
  const newLines = lines.slice(0, range.startIdx).concat(lines.slice(range.endIdx));
  snapshotSource("delete");
  commitSource(newLines.join("\n"));
  if (pathKey(state.selectedStepPath || []) === pathKey(path)) {
    state.selectedStepPath = null;
    state.selectedStepId = null;
  }
  toast("已删除步骤", step.id, "warn");
  renderActiveView();
  renderInspector();
}

export function moveStepBefore(srcPath, dstPath) {
  const srcStep = findStepByPath(extractSteps(state.ast), srcPath);
  const dstStep = findStepByPath(extractSteps(state.ast), dstPath);
  if (!srcStep || !dstStep) return;
  const lines = state.source.split(/\r?\n/);
  const srcR = findStepRange(lines, srcStep.id);
  if (!srcR) return;
  const block = lines.slice(srcR.startIdx, srcR.endIdx);
  const remaining = lines.slice(0, srcR.startIdx).concat(lines.slice(srcR.endIdx));
  const dstR = findStepRange(remaining, dstStep.id);
  if (!dstR) return;
  const final = remaining.slice(0, dstR.startIdx).concat(block).concat(remaining.slice(dstR.startIdx));
  snapshotSource("move");
  commitSource(final.join("\n"));
  toast("已重排", `${srcStep.id} → before ${dstStep.id}`, "ok");
  renderActiveView();
}

export function appendStepToSource(actionId) {
  snapshotSource("append");
  if (!state.source) {
    state.source = EMPTY_FLOW_HEADER;
  }
  const id = `${actionId.replace(/\./g, "_")}_${Math.floor(Math.random() * 1000)}`;
  // Append at end of file with 4-space indent (matches typical examples).
  commitSource(state.source + `\n    - id: ${id}\n      action: ${actionId}\n      with: {}\n`);
  renderActiveView();
  toast("已添加节点", `${id} (${actionId}) — 切到代码视图后点 💾 保存`, "ok");
}

export function appendStepWithSelector(actionId, element) {
  snapshotSource("append");
  if (!state.source) {
    state.source = EMPTY_FLOW_HEADER;
  }
  const id = `${actionId.replace(/\./g, "_")}_${Math.floor(Math.random() * 1000)}`;
  const label = element?.label ? `  # ${element.label}` : "";
  commitSource(state.source + `\n    - id: ${id}${label}\n      action: ${actionId}\n${withBlockForElement(actionId, element)}`);
  renderActiveView();
  toast("已添加节点", `${id} → ${element?.label || actionId}`, "ok");
}

// ─── Duplicate / clipboard / batch delete ────────────────────────────────────

export function duplicateStepByPath(path) {
  const step = findStepByPath(extractSteps(state.ast), path);
  if (!step) return;
  const block = extractStepBlock(state.source, step.id);
  if (!block) return;
  snapshotSource("duplicate");
  commitSource(insertStepBlocks(state.source, String(step.id), [block.text]));
  toast("已创建步骤副本", String(step.id), "ok");
  renderActiveView();
}

function selectionPathKeys() {
  if (state.selectedStepPaths?.size) return [...state.selectedStepPaths];
  if (state.selectedStepPath) return [pathKey(state.selectedStepPath)];
  return [];
}

export function copySelectedSteps() {
  const keys = selectionPathKeys();
  if (!keys.length || typeof state.source !== "string") return 0;
  const { orderedIds } = orderPathKeysByDocument(state.source, keys);
  const blocks = orderedIds
    .map((id) => extractStepBlock(state.source, id)?.text)
    .filter(Boolean);
  if (!blocks.length) return 0;
  try {
    localStorage.setItem(STEP_CLIPBOARD_KEY, JSON.stringify({ blocks }));
  } catch {
    toast("复制失败", "本地存储不可用", "bad");
    return 0;
  }
  toast(`已复制 ${blocks.length} 个步骤`, "Ctrl/Cmd+V 粘贴（可跨流程）", "ok");
  return blocks.length;
}

export function pasteStepsFromClipboard() {
  let payload = null;
  try {
    payload = JSON.parse(localStorage.getItem(STEP_CLIPBOARD_KEY) || "");
  } catch {
    payload = null;
  }
  const blocks = Array.isArray(payload?.blocks)
    ? payload.blocks.filter((b) => typeof b === "string" && b.trim())
    : [];
  if (!blocks.length) {
    toast("剪贴板没有步骤", "先选中步骤按 Ctrl/Cmd+C 复制", "warn");
    return 0;
  }
  const target = state.selectedStepPath
    ? findStepByPath(extractSteps(state.ast), state.selectedStepPath)
    : null;
  snapshotSource("paste");
  if (!state.source) state.source = EMPTY_FLOW_HEADER;
  commitSource(insertStepBlocks(state.source, target ? String(target.id) : null, blocks));
  toast(`已粘贴 ${blocks.length} 个步骤`, target ? `已插入到 ${target.id} 之后` : "已追加到流程末尾", "ok");
  renderActiveView();
  return blocks.length;
}

export function deleteSelectedSteps() {
  const keys = selectionPathKeys();
  if (!keys.length || typeof state.source !== "string") return 0;
  const { orderedIds } = orderPathKeysByDocument(state.source, keys);
  if (!orderedIds.length) return 0;
  snapshotSource("batch-delete");
  commitSource(removeStepBlocks(state.source, orderedIds));
  state.selectedStepPath = null;
  state.selectedStepId = null;
  toast(`已删除 ${orderedIds.length} 个步骤`, orderedIds.join("、").slice(0, 120), "warn");
  renderActiveView();
  renderInspector();
  return orderedIds.length;
}

function withBlockForElement(actionId, element) {
  if (actionId === "browser.extract") {
    const css = element?.fingerprints?.css || "";
    return `      with:\n        selector: ${JSON.stringify(css)}\n        all: false\n`;
  }
  if (actionId === "browser.type") {
    return `${browserSelectorBlock(element)}        text: ""\n        clear: true\n`;
  }
  if (actionId === "browser.wait") {
    return `${browserSelectorBlock(element)}        condition: "visible"\n        timeout_ms: 30000\n`;
  }
  if (actionId === "browser.click") {
    return browserSelectorBlock(element);
  }
  if (actionId === "desktop.click") {
    const bounds = element?.bounds;
    if (bounds) {
      const x = Math.round(bounds.x + bounds.w / 2);
      const y = Math.round(bounds.y + bounds.h / 2);
      return `      with:\n        x: ${x}\n        y: ${y}\n        button: "left"\n`;
    }
    return `      with:\n        button: "left"\n`;
  }
  if (actionId === "desktop.type") {
    return `      with:\n        text: ""\n`;
  }
  return `      with: {}\n`;
}

function browserSelectorBlock(element) {
  const selectors = selectorFields(element);
  const prompt = elementPrompt(element);
  if (!selectors.length) {
    return `      with:\n        selector: ""\n${prompt ? `        prompt: ${JSON.stringify(prompt)}\n` : ""}`;
  }
  const lines = [
    "      with:",
    "        selectors:",
    ...selectors.map(([key, value]) => `          ${key}: ${JSON.stringify(value)}`),
  ];
  if (prompt) lines.push(`        prompt: ${JSON.stringify(prompt)}`);
  return `${lines.join("\n")}\n`;
}

function selectorFields(element) {
  const fp = element?.fingerprints || {};
  return ["id", "data_testid", "css", "aria_label", "text_includes", "xpath"]
    .filter((key) => fp[key])
    .map((key) => [key, fp[key]]);
}

function elementPrompt(element) {
  return [element?.label, element?.role, element?.tag, element?.fingerprints?.visual]
    .filter(Boolean)
    .join(" / ");
}
