// Step source-mutation operations (insert / delete / move / append). All edit
// `state.source` textually, re-parse the AST, then re-render the active view.

import { toast } from "../dom.js";
import { state } from "../state.js";
import {
  parseYaml, extractSteps, findStepByPath, pathKey,
  findStepRange, emitStep,
} from "../yaml.js";
import { renderActiveView } from "./render.js";
import { renderInspector } from "./inspector.js";

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
  state.source = newLines.join("\n");
  state.ast = parseYaml(state.source);
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
  state.source = newLines.join("\n");
  state.ast = parseYaml(state.source);
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
  state.source = newLines.join("\n");
  state.ast = parseYaml(state.source);
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
  state.source = final.join("\n");
  state.ast = parseYaml(state.source);
  toast("已重排", `${srcStep.id} → before ${dstStep.id}`, "ok");
  renderActiveView();
}

export function appendStepToSource(actionId) {
  if (!state.source) {
    state.source = `apiVersion: lumorpa.io/v1\nkind: Flow\nmetadata:\n  id: untitled\n  version: 0.1.0\nspec:\n  steps:\n`;
  }
  const id = `${actionId.replace(/\./g, "_")}_${Math.floor(Math.random() * 1000)}`;
  // Append at end of file with 4-space indent (matches typical examples).
  state.source += `\n    - id: ${id}\n      action: ${actionId}\n      with: {}\n`;
  state.ast = parseYaml(state.source);
  renderActiveView();
  toast("已添加节点", `${id} (${actionId}) — 切到代码视图后点 💾 保存`, "ok");
}

export function appendStepWithSelector(actionId, element) {
  if (!state.source) {
    state.source = `apiVersion: lumorpa.io/v1\nkind: Flow\nmetadata:\n  id: untitled\n  version: 0.1.0\nspec:\n  steps:\n`;
  }
  const id = `${actionId.replace(/\./g, "_")}_${Math.floor(Math.random() * 1000)}`;
  const css = element?.fingerprints?.css || "";
  const label = element?.label ? `  # ${element.label}` : "";
  state.source += `\n    - id: ${id}${label}\n      action: ${actionId}\n      with:\n        selector: ${JSON.stringify(css)}\n`;
  state.ast = parseYaml(state.source);
  renderActiveView();
  toast("已添加节点", `${id} → ${element?.label || actionId}`, "ok");
}
