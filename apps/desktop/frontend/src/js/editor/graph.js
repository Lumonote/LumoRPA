// The canvas projects the executable YAML tree; data links are template edits.
import { $ } from "../dom.js";
import { state, graph } from "../state.js";
import { extractSteps, pathKey } from "../yaml.js";
import { buildGraphModel, graphPortPoint } from "./graph-model.js";
import { graphNodeMarkup } from "./graph-markup.js";
import { fitGraphBounds } from "./graph-viewport.js";
import { bindGraphInteractions } from "./graph-interactions.js";
import { loadSchema } from "./schema.js";
import { getGraphRunState } from "./graph-run-state.js";
export { renderWithSummary } from "./graph-markup.js";

const NS = "http://www.w3.org/2000/svg";
const views = new Map();
const schemaRequests = new Set();
let renderedFlow = null;
let renderedSize = null;

export const graphLayoutKey = () => state.flowPath || state.flow?.path || state.flow?.id || state.ast?.metadata?.id || "__draft__";
export const currentGraphLayouts = () => {
  if (!state.graphLayouts || typeof state.graphLayouts !== "object" || Array.isArray(state.graphLayouts)) state.graphLayouts = {};
  const key = graphLayoutKey();
  if (!Object.hasOwn(state.graphLayouts, key) || !state.graphLayouts[key] || typeof state.graphLayouts[key] !== "object") {
    Object.defineProperty(state.graphLayouts, key, { value: {}, writable: true, enumerable: true, configurable: true });
  }
  return state.graphLayouts[key];
};
export function saveGraphLayouts() {
  try { localStorage.setItem("lumo.graphLayouts", JSON.stringify(state.graphLayouts || {})); } catch { /* Optional local presentation state. */ }
}
export function graphStatus(message) { const el = $("graphStatus"); if (el) { el.textContent = message; el.title = message; } }

export function edgePath(from, to) {
  const bend = Math.max(54, Math.abs(to.x - from.x) * 0.48);
  return `M ${from.x} ${from.y} C ${from.x + bend} ${from.y} ${to.x - bend} ${to.y} ${to.x} ${to.y}`;
}
export function graphEdgePoints(edge) {
  const from = graph.nodePositions?.get(edge.from), to = graph.nodePositions?.get(edge.to);
  if (!from || !to) return null;
  return {
    from: graphPortPoint(from, { direction: "output", kind: edge.kind === "data" ? "data" : edge.kind === "branch" ? "branch" : "execution", branchIndex: edge.branchIndex }),
    to: graphPortPoint(to, { kind: edge.kind === "data" ? "data" : "execution", input: edge.input }),
  };
}

function renderEdge(edge) {
  const group = document.createElementNS(NS, "g");
  group.dataset.edgeId = edge.id;
  const label = edge.label || "执行顺序 · 点击插入节点";
  for (const hit of [true, false]) {
    const path = document.createElementNS(NS, "path");
    path.setAttribute("class", hit ? "graph-edge-hit" : `graph-edge is-${edge.kind}`);
    group.appendChild(path);
  }
  const title = document.createElementNS(NS, "title"); title.textContent = label; group.appendChild(title);
  return group;
}

export function updateGraphGeometry() {
  const root = $("graphRoot"); if (!root || !graph.nodePositions) return;
  root.querySelectorAll(".graph-node-foreign").forEach((element) => {
    const node = graph.nodePositions.get(element.dataset.stepPath); if (!node) return;
    element.setAttribute("x", node.x); element.setAttribute("y", node.y);
  });
  root.querySelectorAll("[data-edge-id]").forEach((element) => {
    const edge = graph.edgeMap?.get(element.dataset.edgeId);
    const points = edge && graphEdgePoints(edge); if (!points) return;
    element.querySelectorAll("path").forEach((path) => path.setAttribute("d", edgePath(points.from, points.to)));
  });
}

export function updateGraphSelection() {
  const selected = state.selectedStepPaths?.size ? state.selectedStepPaths : new Set([pathKey(state.selectedStepPath || [])]);
  $("graphRoot")?.querySelectorAll(".graph-node").forEach((node) => {
    const active = selected.has(node.dataset.stepPath);
    node.classList.toggle("is-selected", active);
    node.classList.toggle("is-multi-selected", active && selected.size > 1);
  });
  $("graphRoot")?.querySelectorAll("[data-edge-id]").forEach((element) => {
    const edge = graph.edgeMap?.get(element.dataset.edgeId);
    const active = edge && (selected.has(edge.from) || selected.has(edge.to));
    element.classList.toggle("is-highlighted", !!active);
  });
  if ($("graphFocus")) $("graphFocus").disabled = !state.selectedStepPath && !state.selectedStepPaths?.size;
}

export function refreshGraphRunState() {
  $("graphRoot")?.querySelectorAll(".graph-node").forEach((element) => {
    const node = graph.nodePositions?.get(element.dataset.stepPath); if (!node) return;
    const result = getGraphRunState(node, state);
    for (const status of ["running", "ok", "failed", "skipped", "cancelled"]) element.classList.toggle(`is-${status}`, result.state === status);
    const label = element.querySelector("[data-node-run-state]");
    if (label) { label.textContent = result.label; label.title = result.durationMs == null ? result.label : `${result.label} · ${result.durationMs} ms`; }
  });
}

export function applyGraphTransform() {
  $("graphRoot")?.setAttribute("transform", `translate(${graph.tx} ${graph.ty}) scale(${graph.scale})`);
  const wrap = $("graphSvg")?.closest(".graph-wrap");
  const size = 24 * graph.scale;
  if (wrap) { wrap.style.backgroundSize = `${size}px ${size}px`; wrap.style.backgroundPosition = `${graph.tx % size}px ${graph.ty % size}px`; }
  const zoom = $("graphZoomReset"); if (zoom) zoom.textContent = `${Math.round(graph.scale * 100)}%`;
  if (renderedFlow) views.set(renderedFlow, { tx: graph.tx, ty: graph.ty, scale: graph.scale });
}

export function resizeGraphViewport() {
  const svg = $("graphSvg"); if (!svg) return;
  const rect = svg.getBoundingClientRect(); if (rect.width < 10 || rect.height < 10) return;
  if (renderedSize && graph.nodePositions?.size) { graph.tx += (rect.width - renderedSize.width) / 2; graph.ty += (rect.height - renderedSize.height) / 2; }
  renderedSize = { width: rect.width, height: rect.height };
  svg.setAttribute("viewBox", `0 0 ${rect.width} ${rect.height}`);
  applyGraphTransform();
}

export function fitGraphSelection(onlySelected = false) {
  const svg = $("graphSvg"); if (!svg || !graph.nodePositions?.size) return;
  const selected = state.selectedStepPaths?.size ? state.selectedStepPaths : new Set([pathKey(state.selectedStepPath || [])]);
  const nodes = [...graph.nodePositions.values()].filter((node) => !onlySelected || selected.has(node.key));
  if (!nodes.length) return;
  const x = Math.min(...nodes.map((node) => node.x)), y = Math.min(...nodes.map((node) => node.y));
  const bounds = { x, y, width: Math.max(...nodes.map((node) => node.x + node.w)) - x, height: Math.max(...nodes.map((node) => node.y + node.h)) - y };
  const rect = svg.getBoundingClientRect(); Object.assign(graph, fitGraphBounds(bounds, rect.width, rect.height)); applyGraphTransform();
}

function requestSchemas(model, flowKey) {
  for (const node of model.nodes.values()) {
    const action = node.step.action; if (!action || state.schemaCache.has(action) || schemaRequests.has(action)) continue;
    schemaRequests.add(action);
    loadSchema(action).then(() => {
      if (state.viewMode !== "graph" || graphLayoutKey() !== flowKey) return;
      if (graph.isInteracting) graph.needsSchemaRender = true;
      else renderGraph();
    }).catch(() => {}).finally(() => schemaRequests.delete(action));
  }
}

export function renderGraph() {
  const svg = $("graphSvg"), root = $("graphRoot"); if (!svg || !root) return;
  graph.interactionRevision = (graph.interactionRevision || 0) + 1;
  const flowKey = graphLayoutKey();
  const changedFlow = renderedFlow !== flowKey;
  const hasSavedView = views.has(flowKey);
  const focusedKey = document.activeElement?.closest?.(".graph-node")?.dataset.stepPath;
  const focusedIdentity = !changedFlow && focusedKey ? graph.nodePositions?.get(focusedKey)?.layoutKey : null;
  const focusedPort = document.activeElement?.matches?.("button.graph-port") ? { ...document.activeElement.dataset } : null;
  if (changedFlow) { renderedFlow = flowKey; Object.assign(graph, views.get(flowKey) || { tx: 60, ty: 90, scale: 1 }); renderedSize = null; graphStatus("画布就绪"); }
  const model = buildGraphModel(extractSteps(state.ast), { layouts: currentGraphLayouts(), schemas: state.schemaCache });
  graph.nodePositions = model.nodes; graph.edges = model.edges; graph.edgeMap = new Map(model.edges.map((edge) => [edge.id, edge])); graph.contentBounds = model.bounds;
  const fragment = document.createDocumentFragment();
  for (const edge of model.edges) fragment.appendChild(renderEdge(edge));
  [...model.nodes.values()].forEach((node, index) => {
    const element = document.createElementNS(NS, "foreignObject");
    for (const [name, value] of Object.entries({ x: node.x, y: node.y, width: node.w, height: node.h, class: "graph-node-foreign", "data-step-path": node.key })) element.setAttribute(name, value);
    element.innerHTML = graphNodeMarkup(node, index); fragment.appendChild(element);
  });
  root.replaceChildren(fragment);
  if ($("graphEmpty")) $("graphEmpty").hidden = model.nodes.size > 0;
  if ($("graphNodeCount")) $("graphNodeCount").textContent = `${model.nodes.size} 节点 · ${model.edges.filter((edge) => edge.kind === "data").length} 数据连接`;
  updateGraphGeometry(); updateGraphSelection(); refreshGraphRunState(); resizeGraphViewport();
  if (changedFlow && !hasSavedView && model.nodes.size) fitGraphSelection();
  if (focusedIdentity) root.querySelectorAll(".graph-node").forEach((element) => {
    if (model.nodes.get(element.dataset.stepPath)?.layoutKey !== focusedIdentity) return;
    const port = focusedPort && [...element.querySelectorAll("button.graph-port")].find((candidate) => candidate.dataset.portKind === focusedPort.portKind && candidate.dataset.portDirection === focusedPort.portDirection && candidate.dataset.portInput === focusedPort.portInput);
    (port || element).focus({ preventScroll: true });
  });
  requestSchemas(model, flowKey);
}

export function bindGraphPan() { return bindGraphInteractions(); }
