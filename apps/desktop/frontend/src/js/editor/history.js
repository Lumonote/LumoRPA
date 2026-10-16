// Editor undo/redo wiring: one snapshot stack over `state.source`. Mutation
// entry points call `snapshotSource(label)` BEFORE overwriting the source so
// undo restores the pre-edit text (plus selection); typing in the code view
// coalesces into a single entry per burst.

import { $, setStatus } from "../dom.js";
import { state } from "../state.js";
import { parseYaml, parsePathKey, pathKey } from "../yaml.js";
import { createHistoryCore } from "./history-core.js";
import { renderActiveView } from "./render.js";
import { renderInspector } from "./inspector.js";

const core = createHistoryCore();

function currentSnapshot() {
  return {
    source: state.source,
    selectedKey: pathKey(state.selectedStepPath || []),
    selectedId: state.selectedStepId,
  };
}

export function snapshotSource(label, coalesceMs = 0) {
  if (typeof state.source !== "string") return;
  core.push(currentSnapshot(), { label, coalesceMs });
  updateHistoryButtons();
}

export function resetHistory() {
  core.reset();
  updateHistoryButtons();
}

function applySnapshot(snap) {
  state.source = snap.source;
  state.ast = parseYaml(state.source);
  state.selectedStepPath = snap.selectedKey ? parsePathKey(snap.selectedKey) : null;
  state.selectedStepId = snap.selectedId ?? null;
  state.selectedStepPaths?.clear();
  renderActiveView();
  renderInspector();
  updateHistoryButtons();
}

export function undoEdit() {
  const snap = core.undo(currentSnapshot());
  if (!snap) {
    setStatus("没有可撤销的编辑", "warn");
    return false;
  }
  applySnapshot(snap);
  setStatus("已撤销", "ok");
  return true;
}

export function redoEdit() {
  const snap = core.redo(currentSnapshot());
  if (!snap) {
    setStatus("没有可重做的编辑", "warn");
    return false;
  }
  applySnapshot(snap);
  setStatus("已重做", "ok");
  return true;
}

export function updateHistoryButtons() {
  const undoBtn = $("undoBtn");
  const redoBtn = $("redoBtn");
  if (undoBtn) undoBtn.disabled = !core.canUndo();
  if (redoBtn) redoBtn.disabled = !core.canRedo();
}
