import { $, html } from "../dom.js";
import { state, graph } from "../state.js";
import { zhAction } from "../constants.js";
import { appendStepToSource, insertStepAfterPath } from "./mutations.js";
import { selectStep } from "./inspector.js";
import { currentGraphLayouts, saveGraphLayouts, renderGraph, graphLayoutKey, graphStatus, fitGraphSelection } from "./graph.js";

let context = null;
let returnFocus = null;

export function closeGraphPalette() {
  const palette = $("graphPalette"); if (!palette || palette.hidden) return;
  palette.hidden = true; context = null;
  if (returnFocus?.isConnected) returnFocus.focus({ preventScroll: true });
}

export function addGraphAction(action, { point, afterPath } = {}) {
  try {
  const before = new Set([...graph.nodePositions?.values() || []].map((node) => node.step.id));
  if (afterPath) insertStepAfterPath(afterPath, action); else appendStepToSource(action);
  const created = [...graph.nodePositions?.values() || []].find((node) => !before.has(node.step.id));
  if (!created) return;
  if (point && created.layoutKey) { currentGraphLayouts()[created.layoutKey] = { x: Math.round(point.x), y: Math.round(point.y) }; saveGraphLayouts(); renderGraph(); }
  selectStep(created.path);
  if (!point) fitGraphSelection(true);
  requestAnimationFrame(() => document.querySelectorAll(".graph-node").forEach((element) => { if (element.dataset.stepPath === created.key) element.focus({ preventScroll: true }); }));
  } catch (error) { graphStatus(error.message || String(error)); }
}

export function openGraphPalette(options = {}) {
  const palette = $("graphPalette"), search = $("graphActionSearch"); if (!palette || !search) return;
  context = { ...options, flowKey: graphLayoutKey(), source: state.source };
  returnFocus = document.activeElement;
  palette.hidden = false; search.value = "";
  const render = () => {
    const query = search.value.trim().toLowerCase();
    const actions = state.actions.filter((action) => `${action.id} ${action.summary || ""} ${zhAction(action.id).label}`.toLowerCase().includes(query)).slice(0, 60);
    $("graphActionResults").innerHTML = actions.length ? actions.map((action) => `<button type="button" data-graph-action="${html(action.id)}"><strong>${html(zhAction(action.id).label)}</strong><code>${html(action.id)}</code><span>${html(action.summary || "")}</span></button>`).join("") : '<p class="graph-palette-empty">未找到动作，请尝试动作名称或英文 ID。</p>';
  };
  search.oninput = render;
  palette.onclick = (event) => {
    const action = event.target.closest("[data-graph-action]")?.dataset.graphAction;
    if (!action) return;
    const saved = context; closeGraphPalette();
    if (!saved || saved.flowKey !== graphLayoutKey() || saved.source !== state.source) return;
    addGraphAction(action, saved);
  };
  palette.onkeydown = (event) => {
    if (event.isComposing) return;
    if (event.key === "Escape") { event.preventDefault(); event.stopPropagation(); closeGraphPalette(); }
    if (event.key === "ArrowDown" && event.target === search) { event.preventDefault(); palette.querySelector("[data-graph-action]")?.focus(); }
    if (event.key === "Enter" && event.target === search) { event.preventDefault(); palette.querySelector("[data-graph-action]")?.click(); }
    if (event.key === "Tab") {
      const items = [search, ...palette.querySelectorAll("[data-graph-action]")];
      if (event.shiftKey && event.target === items[0]) { event.preventDefault(); items.at(-1)?.focus(); }
      else if (!event.shiftKey && event.target === items.at(-1)) { event.preventDefault(); search.focus(); }
    }
  };
  render(); search.focus();
}
