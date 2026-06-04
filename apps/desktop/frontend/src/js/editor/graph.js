// Graph view: SVG flowchart with pan/zoom and drag-to-add. Layout is a simple
// vertical column at depth 0 with children expanding to the right.

import { $, html, pretty } from "../dom.js";
import { state, graph } from "../state.js";
import { extractSteps, pathKey, parsePathKey } from "../yaml.js";
import { appendStepToSource } from "./mutations.js";
import { selectStep } from "./inspector.js";

export function renderGraph() {
  const svg = $("graphSvg");
  const root = $("graphRoot");
  root.innerHTML = "";
  if (!state.ast) {
    return;
  }
  const steps = extractSteps(state.ast);
  if (!steps.length) {
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
      const childKinds = ["do", "else", "catch", "finally"].filter((k) => Array.isArray(step[k]) && step[k].length);
      const myH = NODE_H_HEAD + (childKinds.length ? 14 : 0);
      positions.set(id, { x, y: myY, w: NODE_W, h: myH, step, path });
      curY += myH + GAP_Y;
      childKinds.forEach((kind) => {
        const childPath = [...path, kind];
        layoutList(step[kind], x + CHILD_X, childPath);
        // Mark the kind block top so we can draw a label later if needed.
      });
      maxY = curY;
    });
    return maxY;
  }
  layoutList(steps, 0, null);

  // Determine viewbox.
  const items = [...positions.values()];
  const maxX = Math.max(...items.map((p) => p.x + p.w)) + 40;
  const maxY = Math.max(...items.map((p) => p.y + p.h)) + 40;

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
      ["do", "else", "catch", "finally"].forEach((kind) => {
        if (Array.isArray(step[kind]) && step[kind].length) {
          const childPath = [...path, kind];
          collectEdges(step[kind], childPath, key, kind);
        }
      });
    });
  }
  collectEdges(steps, null);

  // Apply current graph transform.
  root.setAttribute("transform", `translate(${graph.tx} ${graph.ty}) scale(${graph.scale})`);
  svg.setAttribute("viewBox", `0 0 ${Math.max(maxX, 100)} ${Math.max(maxY, 100)}`);

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
    const fo = document.createElementNS("http://www.w3.org/2000/svg", "foreignObject");
    fo.setAttribute("x", String(pos.x));
    fo.setAttribute("y", String(pos.y));
    fo.setAttribute("width", String(pos.w));
    fo.setAttribute("height", String(pos.h + 40));
    fo.setAttribute("class", "graph-node-foreign");
    const withSummary = renderWithSummary(pos.step.with);
    const selected = pathKey(state.selectedStepPath || []) === pathKey(pos.path);
    fo.innerHTML = `
      <div xmlns="http://www.w3.org/1999/xhtml" class="graph-node family-${html(family)} ${selected ? "is-selected" : ""}" data-step-path="${html(pathKey(pos.path))}">
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

// Graph pan + zoom
export function bindGraphPan() {
  const svg = $("graphSvg");
  let panning = false;
  let startX = 0;
  let startY = 0;
  let origTx = 0;
  let origTy = 0;
  svg.addEventListener("pointerdown", (e) => {
    if (e.target.closest("[data-step-path]")) return; // node drag handled elsewhere (selection)
    panning = true;
    svg.classList.add("is-panning");
    startX = e.clientX;
    startY = e.clientY;
    origTx = graph.tx;
    origTy = graph.ty;
    svg.setPointerCapture(e.pointerId);
  });
  svg.addEventListener("pointermove", (e) => {
    if (!panning) return;
    graph.tx = origTx + (e.clientX - startX);
    graph.ty = origTy + (e.clientY - startY);
    $("graphRoot").setAttribute("transform", `translate(${graph.tx} ${graph.ty}) scale(${graph.scale})`);
  });
  svg.addEventListener("pointerup", () => { panning = false; svg.classList.remove("is-panning"); });
  svg.addEventListener("pointercancel", () => { panning = false; svg.classList.remove("is-panning"); });
  svg.addEventListener("wheel", (e) => {
    if (!e.ctrlKey && !e.metaKey) return;
    e.preventDefault();
    const factor = e.deltaY < 0 ? 1.08 : 1 / 1.08;
    graph.scale = Math.max(0.4, Math.min(2.5, graph.scale * factor));
    $("graphRoot").setAttribute("transform", `translate(${graph.tx} ${graph.ty}) scale(${graph.scale})`);
  }, { passive: false });

  // Drop target for actions library
  svg.addEventListener("dragover", (e) => { e.preventDefault(); });
  svg.addEventListener("drop", (e) => {
    e.preventDefault();
    const id = e.dataTransfer.getData("text/x-lumo-action");
    if (id) appendStepToSource(id);
  });
}
