// Graph view: SVG flowchart with pan/zoom and drag-to-add. Layout is a simple
// vertical column at depth 0 with children expanding to the right.

import { $, html, pretty } from "../dom.js";
import { state, graph } from "../state.js";
import { childStepBlocks, extractSteps, pathKey, parsePathKey } from "../yaml.js";
import { appendStepToSource } from "./mutations.js";
import { selectStep } from "./inspector.js";
import { applyLayoutOverrides, moveNodeByScreenDelta, zoomGraphAt } from "./graph-viewport.js";

const GRID_SIZE = 18;
let activeNodeDrag = null;

export function renderGraph() {
  const svg = $("graphSvg");
  const root = $("graphRoot");
  root.innerHTML = "";
  updateGraphViewport(svg);
  if (!state.ast) {
    applyGraphTransform();
    return;
  }
  const steps = extractSteps(state.ast);
  if (!steps.length) {
    applyGraphTransform();
    return;
  }
  // Layout: simple vertical column at depth 0, children expand to the right.
  const NODE_W = 200;
  const NODE_H_HEAD = 60;
  const GAP_Y = 28;
  const CHILD_X = 240;
  const positions = new Map();
  let curY = 0;
  function layoutList(list, x, parentPath) {
    let maxY = curY;
    list.forEach((step, idx) => {
      const path = parentPath ? [...parentPath, idx] : [idx];
      const myY = curY;
      const id = pathKey(path);
      const childBlocks = childStepBlocks(step);
      const myH = NODE_H_HEAD + (childBlocks.length ? 14 : 0);
      positions.set(id, { x, y: myY, w: NODE_W, h: myH, step, path });
      curY += myH + GAP_Y;
      childBlocks.forEach((block) => {
        const childPath = [...path, ...block.path];
        layoutList(block.steps, x + CHILD_X, childPath);
        // Mark the kind block top so we can draw a label later if needed.
      });
      maxY = curY;
    });
    return maxY;
  }
  layoutList(steps, 0, null);
  applyLayoutOverrides(positions, currentGraphLayoutOverrides());
  graph.nodePositions = positions;

  // Determine viewbox.
  const items = [...positions.values()];
  const maxX = Math.max(...items.map((p) => p.x + p.w)) + 40;
  const maxY = Math.max(...items.map((p) => p.y + p.h)) + 40;
  graph.contentBounds = { width: Math.max(maxX, 100), height: Math.max(maxY, 100) };

  // Edges: each sibling step → next sibling step at same parent. Plus parent → first child for each child kind.
  const edges = [];
  function collectEdges(list, parentPath, parentId = null, parentKind = null) {
    list.forEach((step, idx) => {
      const path = parentPath ? [...parentPath, idx] : [idx];
      const key = pathKey(path);
      if (idx > 0) {
        const prevKey = pathKey(parentPath ? [...parentPath, idx - 1] : [idx - 1]);
        edges.push({ from: prevKey, to: key, kind: "seq" });
      } else if (parentId) {
        edges.push({ from: parentId, to: key, kind: parentKind === "do" ? "loop" : "control" });
      }
      childStepBlocks(step).forEach((block) => {
        const childPath = [...path, ...block.path];
        collectEdges(block.steps, childPath, key, block.kind);
      });
    });
  }
  collectEdges(steps, null);

  // Apply current graph transform.
  applyGraphTransform();

  // Draw edges first (under nodes).
  for (const e of edges) {
    const from = positions.get(e.from);
    const to = positions.get(e.to);
    if (!from || !to) continue;
    const fx = from.x + from.w / 2;
    const fy = from.y + from.h;
    const tx = to.x + to.w / 2;
    const ty = to.y;
    const path = document.createElementNS("http://www.w3.org/2000/svg", "path");
    const klass =
      e.kind === "loop"
        ? "graph-edge is-loop"
        : e.kind === "control"
          ? "graph-edge is-control"
          : "graph-edge";
    path.setAttribute("class", klass);
    const c1x = fx;
    const c1y = (fy + ty) / 2;
    const c2x = tx;
    const c2y = (fy + ty) / 2;
    path.setAttribute("d", `M ${fx} ${fy} C ${c1x} ${c1y} ${c2x} ${c2y} ${tx} ${ty}`);
    path.setAttribute("marker-end", "url(#arrowhead)");
    path.setAttribute("stroke", "currentColor");
    path.style.color =
      e.kind === "loop"
        ? "var(--accent)"
        : e.kind === "control"
          ? "var(--accent-2)"
          : "var(--line-strong)";
    root.appendChild(path);
  }

  // Draw nodes.
  for (const pos of positions.values()) {
    const family = pos.step.action ? pos.step.action.split(".")[0] : "misc";
    const key = pathKey(pos.path);
    const fo = document.createElementNS("http://www.w3.org/2000/svg", "foreignObject");
    fo.setAttribute("x", String(pos.x));
    fo.setAttribute("y", String(pos.y));
    fo.setAttribute("width", String(pos.w));
    fo.setAttribute("height", String(pos.h + 40));
    fo.setAttribute("class", "graph-node-foreign");
    fo.setAttribute("data-step-path", key);
    const withSummary = renderWithSummary(pos.step.with);
    const selected = pathKey(state.selectedStepPath || []) === key;
    fo.innerHTML = `
      <div xmlns="http://www.w3.org/1999/xhtml" class="graph-node family-${html(family)} ${selected ? "is-selected" : ""}" data-step-path="${html(key)}">
        <div class="graph-node-head">
          <span class="id">${html(pos.step.id || "(no id)")}</span>
          <span class="action">${html(pos.step.action || "?")}</span>
        </div>
        <div class="graph-node-body">${withSummary || "<em style=\"color: var(--faint)\">无参数</em>"}</div>
        ${pos.step.retry ? `<div class="graph-node-foot">retry × ${html(pos.step.retry.times || 1)}</div>` : ""}
      </div>`;
    root.appendChild(fo);
  }

  // Bind clicks (event delegation).
  svg.onclick = (event) => {
    const node = event.target.closest("[data-step-path]");
    if (node) {
      selectStep(parsePathKey(node.dataset.stepPath));
    }
  };
}

export function renderWithSummary(w) {
  if (!w || typeof w !== "object") return "";
  return Object.entries(w)
    .slice(0, 3)
    .map(([k, v]) => {
      let val = typeof v === "string" ? v : pretty(v);
      if (val.length > 38) val = val.slice(0, 36) + "…";
      return `<span><strong>${html(k)}</strong>: ${html(val)}</span>`;
    })
    .join("<br />");
}

export function applyGraphTransform() {
  $("graphRoot")?.setAttribute("transform", `translate(${graph.tx} ${graph.ty}) scale(${graph.scale})`);
  updateGraphGrid();
}

function updateGraphViewport(svg) {
  const rect = svg.getBoundingClientRect();
  const width = Math.max(Math.round(rect.width || 0), 100);
  const height = Math.max(Math.round(rect.height || 0), 100);
  svg.setAttribute("viewBox", `0 0 ${width} ${height}`);
}

function updateGraphGrid() {
  const svg = $("graphSvg");
  const wrap = svg?.closest(".graph-wrap");
  if (!wrap) return;
  const size = GRID_SIZE * graph.scale;
  wrap.style.backgroundSize = `${size}px ${size}px`;
  wrap.style.backgroundPosition = `${positiveModulo(graph.tx, size)}px ${positiveModulo(graph.ty, size)}px`;
}

function positiveModulo(value, size) {
  return ((value % size) + size) % size;
}

function currentGraphLayoutKey() {
  return state.flowPath || state.flow?.path || state.flow?.id || state.ast?.metadata?.id || "__draft__";
}

function currentGraphLayoutOverrides() {
  if (!state.graphLayouts || typeof state.graphLayouts !== "object" || Array.isArray(state.graphLayouts)) {
    state.graphLayouts = {};
  }
  const key = currentGraphLayoutKey();
  if (!state.graphLayouts[key] || typeof state.graphLayouts[key] !== "object") {
    state.graphLayouts[key] = {};
  }
  return state.graphLayouts[key];
}

function setGraphNodeLayoutOverride(nodeKey, position) {
  currentGraphLayoutOverrides()[nodeKey] = {
    x: Math.round(position.x),
    y: Math.round(position.y),
  };
}

function saveGraphLayouts() {
  try {
    localStorage.setItem("lumo.graphLayouts", JSON.stringify(state.graphLayouts || {}));
  } catch {
    // Local layout is convenience state; ignore storage quota/privacy failures.
  }
}

function beginGraphNodeDrag(event, nodeKey, position) {
  if (event.button !== undefined && event.button !== 0) return;
  const svg = $("graphSvg");
  activeNodeDrag = {
    key: nodeKey,
    startX: event.clientX,
    startY: event.clientY,
    startPosition: { x: position.x, y: position.y },
    // Pointer capture redirects the eventual click to the svg, so selection
    // is handled explicitly on pointerup; below-threshold presses select,
    // beyond-threshold presses drag.
    moved: false,
  };
  svg.classList.add("is-dragging-node");
  try {
    svg.setPointerCapture(event.pointerId);
  } catch {
    // Window-level drag listeners still handle foreignObject/browser edge cases.
  }
  event.preventDefault();
  event.stopPropagation();
}

function finishGraphNodeDrag(svg) {
  if (activeNodeDrag) {
    if (activeNodeDrag.moved) saveGraphLayouts();
    else selectStep(parsePathKey(activeNodeDrag.key));
  }
  activeNodeDrag = null;
  svg.classList.remove("is-dragging-node");
}

const DRAG_THRESHOLD_PX = 4;

function zoomFromCenter(svg, factor) {
  const rect = svg.getBoundingClientRect();
  const next = zoomGraphAt(graph, rect.width / 2, rect.height / 2, factor);
  graph.tx = next.tx;
  graph.ty = next.ty;
  graph.scale = next.scale;
  applyGraphTransform();
}

function resetGraphView() {
  graph.scale = 1;
  graph.tx = 24;
  graph.ty = 24;
  applyGraphTransform();
}

// Graph pan + zoom
export function bindGraphPan() {
  const svg = $("graphSvg");
  $("graphZoomIn")?.addEventListener("click", () => zoomFromCenter(svg, 1.15));
  $("graphZoomOut")?.addEventListener("click", () => zoomFromCenter(svg, 1 / 1.15));
  $("graphZoomReset")?.addEventListener("click", resetGraphView);
  let panning = false;
  let startX = 0;
  let startY = 0;
  let origTx = 0;
  let origTy = 0;
  document.addEventListener("pointerdown", (e) => {
    const node = e.target.closest?.(".graph-node[data-step-path]");
    if (!node) return;
    const nodeKey = node.dataset.stepPath;
    const pos = graph.nodePositions?.get(nodeKey);
    if (!pos) return;
    beginGraphNodeDrag(e, nodeKey, pos);
  }, true);
  svg.addEventListener("pointerdown", (e) => {
    if (e.button !== undefined && e.button !== 0) return;
    const node = e.target.closest?.(".graph-node[data-step-path]");
    if (node) {
      const nodeKey = node.dataset.stepPath;
      const pos = graph.nodePositions?.get(nodeKey);
      if (!pos) return;
      beginGraphNodeDrag(e, nodeKey, pos);
      return;
    }
    panning = true;
    svg.classList.add("is-panning");
    startX = e.clientX;
    startY = e.clientY;
    origTx = graph.tx;
    origTy = graph.ty;
    svg.setPointerCapture(e.pointerId);
  });
  const handlePointerMove = (e) => {
    if (activeNodeDrag) {
      const dx = e.clientX - activeNodeDrag.startX;
      const dy = e.clientY - activeNodeDrag.startY;
      if (!activeNodeDrag.moved && Math.hypot(dx, dy) < DRAG_THRESHOLD_PX) return;
      activeNodeDrag.moved = true;
      const next = moveNodeByScreenDelta(activeNodeDrag.startPosition, graph, dx, dy);
      setGraphNodeLayoutOverride(activeNodeDrag.key, next);
      renderGraph();
      return;
    }
    if (!panning) return;
    graph.tx = origTx + (e.clientX - startX);
    graph.ty = origTy + (e.clientY - startY);
    applyGraphTransform();
  };
  const handlePointerEnd = () => {
    finishGraphNodeDrag(svg);
    panning = false;
    svg.classList.remove("is-panning");
  };
  window.addEventListener("pointermove", handlePointerMove);
  window.addEventListener("pointerup", handlePointerEnd);
  window.addEventListener("pointercancel", handlePointerEnd);
  svg.addEventListener("wheel", (e) => {
    e.preventDefault();
    if (e.ctrlKey || e.metaKey) {
      const rect = svg.getBoundingClientRect();
      const factor = e.deltaY < 0 ? 1.08 : 1 / 1.08;
      const next = zoomGraphAt(graph, e.clientX - rect.left, e.clientY - rect.top, factor);
      graph.tx = next.tx;
      graph.ty = next.ty;
      graph.scale = next.scale;
    } else {
      graph.tx -= e.deltaX;
      graph.ty -= e.deltaY;
    }
    applyGraphTransform();
  }, { passive: false });

  // Drop target for actions library
  svg.addEventListener("dragover", (e) => { e.preventDefault(); });
  svg.addEventListener("drop", (e) => {
    e.preventDefault();
    const id = e.dataTransfer.getData("text/x-lumo-action");
    if (id) appendStepToSource(id);
  });
}
