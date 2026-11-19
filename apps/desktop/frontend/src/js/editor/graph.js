// The canvas projects the executable YAML tree; data links are template edits.
import { $, html } from "../dom.js";
import { state, graph } from "../state.js";
import { extractSteps, pathKey } from "../yaml.js";
import { buildGraphModel, graphPortPoint } from "./graph-model.js";
import { graphNodeMarkup } from "./graph-markup.js";
import { fitGraphBounds } from "./graph-viewport.js";
import { bindGraphInteractions } from "./graph-interactions.js";
import { loadSchema } from "./schema.js";
import { getGraphRunState, getGraphRunIndex } from "./graph-run-state.js";
import { projectGraphPresentation, graphBounds, studioLayout, normalizeGraphLayouts, isValidGraphView } from "./graph-presentation.js";
import { persistGraphLayout, scheduleGraphLayoutSave } from "./graph-layout-store.js";
import { mountGraphNavigation, renderGraphMinimap, updateGraphMinimapViewport, updateGraphMinimapState } from "./graph-navigation.js";
export { renderWithSummary } from "./graph-markup.js";

const NS = "http://www.w3.org/2000/svg";
const views = new Map();
const schemaRequests = new Set();
let renderedFlow = null;
let renderedSize = null;
const nodeElements = new Map(), edgeElements = new Map(), incidentEdges = new Map();
let statusFrame = null;
const normalizedLayouts = new WeakSet();

export const graphLayoutKey = () => state.flowPath || state.flow?.path || state.flow?.id || state.ast?.metadata?.id || "__draft__";
export const currentGraphLayouts = () => {
  if (!state.graphLayouts || typeof state.graphLayouts !== "object" || Array.isArray(state.graphLayouts)) state.graphLayouts = {};
  const key = graphLayoutKey();
  if (!Object.hasOwn(state.graphLayouts, key) || !state.graphLayouts[key] || typeof state.graphLayouts[key] !== "object" || Array.isArray(state.graphLayouts[key])) {
    Object.defineProperty(state.graphLayouts, key, { value: {}, writable: true, enumerable: true, configurable: true });
  }
  const layouts = state.graphLayouts[key];
  if (!normalizedLayouts.has(layouts)) { normalizeGraphLayouts(layouts); normalizedLayouts.add(layouts); }
  return layouts;
};
export function saveGraphLayouts() {
  return persistGraphLayout(graphLayoutKey(), currentGraphLayouts());
}
export function graphStatus(message) { const el = $("graphStatus"); if (el) { el.textContent = message; el.title = message; } }

export function edgePath(from, to, route) {
  if (route && Number.isFinite(route.x) && Number.isFinite(route.y)) return `${edgePath(from, route)} ${edgePath(route, to)}`;
  if (to.x < from.x + 40) {
    const lane = Math.min(from.y, to.y) - 46;
    return `M ${from.x} ${from.y} C ${from.x + 54} ${from.y} ${from.x + 54} ${lane} ${from.x + 54} ${lane} L ${to.x - 54} ${lane} C ${to.x - 54} ${lane} ${to.x - 54} ${to.y} ${to.x} ${to.y}`;
  }
  const bend = Math.max(54, Math.abs(to.x - from.x) * 0.48);
  return `M ${from.x} ${from.y} C ${from.x + bend} ${from.y} ${to.x - bend} ${to.y} ${to.x} ${to.y}`;
}
export function graphEdgePoints(edge) {
  const from = graph.nodePositions?.get(edge.from), to = graph.nodePositions?.get(edge.to);
  if (!from || !to) return null;
  const boundaryPoint = (node, direction) => {
    const port = node.boundaryPorts?.find((port) => port.edgeId === edge.id);
    return port ? { x: node.x + (direction === "output" ? node.w : 0), y: node.y + 72 + 14 + port.row * 28 } : null;
  };
  return {
    from: boundaryPoint(from, "output") || graphPortPoint(from, { direction: "output", kind: edge.kind === "data" ? "data" : edge.kind === "branch" ? "branch" : "execution", branchIndex: edge.branchIndex, outputPath: edge.outputPath }),
    to: boundaryPoint(to, "input") || graphPortPoint(to, { kind: edge.kind === "data" ? "data" : "execution", input: edge.input }),
  };
}

function renderEdge(edge) {
  const group = document.createElementNS(NS, "g");
  group.dataset.edgeId = edge.id;
  const label = `${edge.label || "执行顺序 · 点击插入节点"} · Alt 点击设置中继点`;
  for (const hit of [true, false]) {
    const path = document.createElementNS(NS, "path");
    path.setAttribute("class", hit ? "graph-edge-hit" : `graph-edge is-${edge.kind}`);
    group.appendChild(path);
  }
  const title = document.createElementNS(NS, "title"); title.textContent = label; group.appendChild(title);
  const route = studioLayout(currentGraphLayouts()).routes[edge.routeKey];
  if (route) {
    const point = document.createElementNS(NS, "circle"); point.setAttribute("class", "graph-route-point"); point.setAttribute("r", "6"); point.dataset.routeKey = edge.routeKey; group.appendChild(point);
  }
  edgeElements.set(edge.id, group);
  for (const key of [edge.from, edge.to]) { if (!incidentEdges.has(key)) incidentEdges.set(key, new Set()); incidentEdges.get(key).add(edge.id); }
  return group;
}

export function updateGraphGeometry(changedKeys) {
  if (!graph.nodePositions) return;
  const keys = changedKeys ? [...changedKeys] : [...nodeElements.keys()];
  const edges = changedKeys ? new Set(keys.flatMap((key) => [...(incidentEdges.get(key) || [])])) : new Set(edgeElements.keys());
  for (const key of keys) {
    const node = graph.nodePositions.get(key), element = nodeElements.get(key)?.foreign; if (!node || !element) continue;
    element.setAttribute("x", node.x); element.setAttribute("y", node.y);
  }
  for (const id of edges) {
    const element = edgeElements.get(id), edge = graph.edgeMap?.get(id), points = edge && graphEdgePoints(edge); if (!points || !element) continue;
    const route = studioLayout(currentGraphLayouts()).routes[edge.routeKey];
    const path = edgePath(points.from, points.to, route);
    element.querySelectorAll("path").forEach((line) => line.setAttribute("d", path));
    const point = element.querySelector("circle"); if (point && route) { point.setAttribute("cx", route.x); point.setAttribute("cy", route.y); }
  }
  // Only groups touching the current drag need new bounds.
  for (const frame of graph.groupFrames || []) {
    if (changedKeys && !frame.nodeKeys.some((key) => keys.includes(key))) continue;
    const bounds = graphBounds(frame.nodeKeys.map((key) => graph.nodePositions.get(key)).filter(Boolean), 22);
    Object.assign(frame, bounds);
    const element = frame.element; if (!element) continue;
    const rect = element.querySelector("rect"), header = element.querySelector("foreignObject");
    for (const [name, value] of Object.entries({ x: bounds.x, y: bounds.y - 30, width: bounds.width, height: bounds.height + 30 })) rect.setAttribute(name, value);
    header.setAttribute("x", bounds.x); header.setAttribute("y", bounds.y - 30); header.setAttribute("width", bounds.width);
  }
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
  if (statusFrame !== null) return;
  const paint = () => {
    statusFrame = null;
    const index = getGraphRunIndex(state);
    for (const [key, { card }] of nodeElements) {
      const node = graph.nodePositions?.get(key); if (!node || !card) continue;
      let result = getGraphRunState(node, state, index);
      if (node.members?.length) {
        const results = [result, ...node.members.map((member) => getGraphRunState(graph.allNodePositions.get(member), state, index))];
        result = results.find((item) => item.state === "failed") || results.find((item) => item.state === "running") || result;
        const done = results.filter((item) => item.state === "ok").length;
        result = { ...result, label: `${result.label} · ${done}/${results.length}` };
      }
      node.runState = result.state;
      const signature = `${result.state}:${result.label}:${result.durationMs}`;
      if (card.dataset.runSignature === signature) continue;
      card.dataset.runSignature = signature;
      updateGraphMinimapState(key, result.state);
      for (const status of ["running", "ok", "failed", "skipped", "cancelled"]) card.classList.toggle(`is-${status}`, result.state === status);
      const label = card.querySelector("[data-node-run-state]");
      if (label) { label.textContent = result.label; label.title = result.durationMs == null ? result.label : `${result.label} · ${result.durationMs} ms`; }
    }
  };
  if (typeof requestAnimationFrame === "function") statusFrame = requestAnimationFrame(paint); else paint();
}

export function applyGraphTransform() {
  $("graphRoot")?.setAttribute("transform", `translate(${graph.tx} ${graph.ty}) scale(${graph.scale})`);
  const wrap = $("graphSvg")?.closest(".graph-wrap");
  const size = Math.max(6, 24 * graph.scale);
  if (wrap) { wrap.dataset.detail = graph.scale < 0.2 ? "overview" : graph.scale < 0.55 ? "compact" : "full"; wrap.style.backgroundSize = `${size}px ${size}px`; wrap.style.backgroundPosition = `${graph.tx % size}px ${graph.ty % size}px`; }
  const zoom = $("graphZoomReset"); if (zoom) zoom.textContent = `${graph.scale < 0.01 ? (graph.scale * 100).toFixed(2) : Math.round(graph.scale * 100)}%`;
  if (renderedFlow && renderedFlow === graphLayoutKey() && state.viewMode === "graph") {
    const view = { tx: graph.tx, ty: graph.ty, scale: graph.scale }, flow = renderedFlow, layouts = currentGraphLayouts();
    views.set(flow, view); studioLayout(layouts).view = view;
    scheduleGraphLayoutSave(flow);
  }
  updateGraphMinimapViewport();
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
  const frames = onlySelected ? [] : (graph.groupFrames || []).map((frame) => ({ x: frame.x, y: frame.y - 30, w: frame.width, h: frame.height + 30 }));
  const bounds = graphBounds([...nodes, ...frames]);
  const rect = svg.getBoundingClientRect(), drawer = $("graphResultDrawer");
  const reserved = drawer && !drawer.hidden ? drawer.getBoundingClientRect().width + 28 : 0;
  Object.assign(graph, fitGraphBounds(bounds, Math.max(100, rect.width - reserved), rect.height)); applyGraphTransform();
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
  mountGraphNavigation();
  graph.interactionRevision = (graph.interactionRevision || 0) + 1;
  const flowKey = graphLayoutKey();
  const changedFlow = renderedFlow !== flowKey;
  const cachedView = views.get(flowKey);
  const savedView = isValidGraphView(cachedView) ? cachedView : currentGraphLayouts().__studio?.view;
  const hasSavedView = !!savedView;
  const focusedKey = document.activeElement?.closest?.(".graph-node")?.dataset.stepPath;
  const focusedIdentity = !changedFlow && focusedKey ? graph.nodePositions?.get(focusedKey)?.layoutKey : null;
  const focusedPort = document.activeElement?.matches?.("button.graph-port") ? { ...document.activeElement.dataset } : null;
  if (changedFlow) {
    renderedFlow = flowKey; Object.assign(graph, savedView || { tx: 60, ty: 90, scale: 1 }); renderedSize = null; graphStatus("画布就绪");
    const drawer = $("graphResultDrawer"); if (drawer) { drawer.hidden = true; drawer.closest(".graph-wrap")?.classList.remove("has-result"); }
  }
  const allModel = buildGraphModel(extractSteps(state.ast), { layouts: currentGraphLayouts(), schemas: state.schemaCache });
  graph.modelFlowKey = flowKey; graph.modelSource = state.source;
  graph.allNodePositions = allModel.nodes;
  const model = projectGraphPresentation(allModel, currentGraphLayouts());
  graph.groupFrames = model.frames;
  nodeElements.clear(); edgeElements.clear(); incidentEdges.clear();
  if ($("graphSnap")) $("graphSnap").setAttribute("aria-pressed", String(studioLayout(currentGraphLayouts()).snap !== false));
  graph.nodePositions = model.nodes; graph.edges = model.edges; graph.edgeMap = new Map(model.edges.map((edge) => [edge.id, edge])); graph.contentBounds = model.bounds;
  const fragment = document.createDocumentFragment();
  for (const frame of model.frames) {
    const element = document.createElementNS(NS, "g"); element.setAttribute("class", `graph-group-frame color-${["sage", "blue", "amber", "rose"].includes(frame.color) ? frame.color : "sage"}`);
    const rect = document.createElementNS(NS, "rect"); rect.setAttribute("rx", "12"); element.appendChild(rect);
    const header = document.createElementNS(NS, "foreignObject"); header.setAttribute("height", "30");
    header.innerHTML = `<div xmlns="http://www.w3.org/1999/xhtml" class="graph-group-head"><button type="button" data-collapse-group="${html(frame.id)}" title="折叠分组">▾ ${html(frame.title)} <small>${frame.nodeKeys.length}</small></button><button type="button" data-delete-group="${html(frame.id)}" aria-label="解除分组">×</button></div>`;
    element.appendChild(header); frame.element = element; fragment.appendChild(element);
  }
  for (const edge of model.edges) fragment.appendChild(renderEdge(edge));
  [...model.nodes.values()].forEach((node, index) => {
    const element = document.createElementNS(NS, "foreignObject");
    for (const [name, value] of Object.entries({ x: node.x, y: node.y, width: node.w, height: node.h, class: "graph-node-foreign", "data-step-path": node.key })) element.setAttribute(name, value);
    element.innerHTML = graphNodeMarkup(node, index); nodeElements.set(node.key, { foreign: element, card: element.querySelector(".graph-node") }); fragment.appendChild(element);
  });
  root.replaceChildren(fragment);
  if ($("graphEmpty")) $("graphEmpty").hidden = model.nodes.size > 0;
  if ($("graphNodeCount")) $("graphNodeCount").textContent = `${model.nodes.size}/${allModel.nodes.size} 节点 · ${model.edges.filter((edge) => edge.kind === "data").length} 数据连接`;
  updateGraphGeometry(); updateGraphSelection(); refreshGraphRunState(); resizeGraphViewport(); renderGraphMinimap();
  if (changedFlow && !hasSavedView && model.nodes.size) fitGraphSelection();
  if (focusedIdentity) root.querySelectorAll(".graph-node").forEach((element) => {
    if (model.nodes.get(element.dataset.stepPath)?.layoutKey !== focusedIdentity) return;
    const port = focusedPort && [...element.querySelectorAll("button.graph-port")].find((candidate) => candidate.dataset.portKind === focusedPort.portKind && candidate.dataset.portDirection === focusedPort.portDirection && candidate.dataset.portInput === focusedPort.portInput && candidate.dataset.portOutput === focusedPort.portOutput);
    (port || element).focus({ preventScroll: true });
  });
  requestSchemas(model, flowKey);
}

export function bindGraphPan() { return bindGraphInteractions(); }
