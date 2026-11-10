import { $ } from "../dom.js";
import { graph, state } from "../state.js";
import { pathKey, parsePathKey } from "../yaml.js";
import { zoomGraphAt, screenToWorld, moveNodeByScreenDelta } from "./graph-viewport.js";
import { graphPortPoint } from "./graph-model.js";
import { renderGraph, updateGraphGeometry, applyGraphTransform, fitGraphSelection, resizeGraphViewport, currentGraphLayouts, saveGraphLayouts, graphStatus, edgePath, updateGraphSelection, graphLayoutKey } from "./graph.js";
import { selectStep } from "./inspector.js";
import { snapshotSource } from "./history.js";
import { openGraphPalette, closeGraphPalette, addGraphAction } from "./graph-palette.js";
import { connectGraphPorts, closeGraphConfirmation } from "./graph-connect-ui.js";

const mounted = new WeakMap();
const editable = (target) => target?.isContentEditable || ["INPUT", "TEXTAREA", "SELECT"].includes(target?.tagName);
const nodeElement = (target) => target?.closest?.(".graph-node[data-step-path]");
const isSiblingBefore = (a, b) => a.path.length === b.path.length && a.path.slice(0, -1).every((part, i) => part === b.path[i]) && a.path.at(-1) < b.path.at(-1);

export function bindGraphInteractions() {
  const svg = $("graphSvg"); if (!svg || mounted.has(svg)) return mounted.get(svg);
  const listeners = [];
  const on = (element, name, handler, options) => { if (!element) return; element.addEventListener(name, handler, options); listeners.push(() => element.removeEventListener(name, handler, options)); };
  let gesture = null, connection = null, preview = null, frame = null, lastEvent = null, space = false;
  const relative = (event) => { const rect = svg.getBoundingClientRect(); return { x: event.clientX - rect.left, y: event.clientY - rect.top }; };
  const world = (event) => { const point = relative(event); return screenToWorld(graph, point.x, point.y); };
  const cancelConnection = () => {
    connection = null; preview?.remove(); preview = null; svg.classList.remove("is-connecting");
    svg.querySelectorAll(".is-valid-target,.is-connecting").forEach((el) => el.classList.remove("is-valid-target", "is-connecting"));
    graph.isInteracting = !!gesture;
    if (graph.needsSchemaRender) queueMicrotask(() => { if (!graph.isInteracting) { graph.needsSchemaRender = false; renderGraph(); } });
  };
  const startConnection = (port, event) => {
    cancelConnection();
    const node = graph.nodePositions?.get(port.dataset.nodeKey); if (!node) return;
    connection = { node, kind: port.dataset.portKind, source: state.source, flowKey: graphLayoutKey() };
    graph.isInteracting = true;
    preview = document.createElementNS("http://www.w3.org/2000/svg", "path"); preview.setAttribute("class", `graph-edge is-${connection.kind === "data" ? "data" : "sequence"} is-connecting`); preview.style.pointerEvents = "none"; $("graphRoot").appendChild(preview);
    const point = graphPortPoint(node, { direction: "output", kind: connection.kind }); preview.setAttribute("d", edgePath(point, event ? world(event) : { x: point.x + 80, y: point.y }));
    svg.classList.add("is-connecting"); port.classList.add("is-connecting");
    svg.querySelectorAll(`[data-port-direction="input"][data-port-kind="${connection.kind}"]`).forEach((input) => {
      const target = graph.nodePositions.get(input.dataset.nodeKey);
      if (target && (connection.kind === "execution" ? node.key !== target.key && node.path.length === target.path.length && node.path.slice(0, -1).every((part, i) => part === target.path[i]) : isSiblingBefore(node, target))) input.classList.add("is-valid-target");
    });
    graphStatus(connection.kind === "execution" ? "选择同一分支的执行输入 · 将调整流程顺序" : "选择后序节点的输入端口 · Esc 取消");
  };
  const finishConnection = (port) => {
    const source = connection; if (!source || port?.dataset.portDirection !== "input") return;
    if (source.kind !== port.dataset.portKind) { graphStatus("执行端口和数据端口不能混接 · Esc 取消"); return; }
    const target = graph.nodePositions?.get(port.dataset.nodeKey);
    cancelConnection();
    if (!target || source.source !== state.source || source.flowKey !== graphLayoutKey()) { graphStatus("流程已变化，请重新连线"); return; }
    connectGraphPorts({ fromPath: source.node.path, toPath: target.path, input: port.dataset.portInput, kind: source.kind });
  };
  const chooseNode = (element, additive = false) => {
    const path = parsePathKey(element.dataset.stepPath), key = pathKey(path);
    if (additive) {
      if (!state.selectedStepPaths?.size && state.selectedStepPath) state.selectedStepPaths.add(pathKey(state.selectedStepPath));
      if (state.selectedStepPaths.has(key)) state.selectedStepPaths.delete(key); else state.selectedStepPaths.add(key);
      if (!state.selectedStepPaths.size) selectStep(null);
      else selectStep(state.selectedStepPaths.has(key) ? path : parsePathKey([...state.selectedStepPaths].at(-1)), { preserveMulti: true });
    } else selectStep(path);
    updateGraphSelection();
    if ((window.matchMedia?.("(max-width: 1080px)").matches || svg.closest(".shell")?.classList.contains("is-canvas-focus")) && state.selectedStepPath) {
      $("rightRail")?.classList.add("is-graph-open"); $("graphInspectorToggle")?.setAttribute("aria-expanded", "true");
    }
  };
  const zoom = (factor) => {
    const rect = svg.getBoundingClientRect(); Object.assign(graph, zoomGraphAt(graph, rect.width / 2, rect.height / 2, factor)); applyGraphTransform();
  };
  const clearGesture = (cancelled = false) => {
    if (!gesture) return;
    const saved = gesture; gesture = null;
    const stale = saved.flowKey !== graphLayoutKey() || saved.source !== state.source || saved.revision !== graph.interactionRevision;
    if (stale) cancelled = true;
    if (frame !== null) { cancelAnimationFrame(frame); frame = null; }
    if (!cancelled && lastEvent) paintGesture(lastEvent, saved);
    lastEvent = null;
    saved.marquee?.remove();
    if (saved.type === "nodes" && saved.moved && !cancelled) {
      snapshotSource("graph-layout");
      const layouts = currentGraphLayouts();
      for (const key of saved.positions.keys()) { const node = graph.nodePositions.get(key); if (node?.layoutKey) layouts[node.layoutKey] = { x: Math.round(node.x), y: Math.round(node.y) }; }
      saveGraphLayouts(); graphStatus(`已移动 ${saved.positions.size} 个节点 · 可撤销`);
    } else if (!cancelled && saved.type === "nodes" && !saved.moved) chooseNode(saved.element, saved.additive);
    else if (!cancelled && saved.type === "pan" && !saved.moved) { state.selectedStepPaths.clear(); selectStep(null); }
    else if (!stale && saved.type === "select") {
      if (cancelled) state.selectedStepPaths = new Set(saved.original);
      const primary = [...state.selectedStepPaths].at(-1);
      selectStep(primary ? parsePathKey(primary) : null, { preserveMulti: true });
    }
    if (cancelled && !stale && saved.type === "pan") { graph.tx = saved.tx; graph.ty = saved.ty; applyGraphTransform(); }
    graph.isInteracting = false;
    svg.classList.remove("is-panning", "is-dragging-node");
    try { if (svg.hasPointerCapture(saved.pointerId)) svg.releasePointerCapture(saved.pointerId); } catch { /* Pointer ended outside the window. */ }
    if (cancelled || graph.needsSchemaRender) { graph.needsSchemaRender = false; renderGraph(); }
  };
  function paintGesture(event, active = gesture) {
    if (!active) return;
    if (active.flowKey !== graphLayoutKey() || active.source !== state.source || active.revision !== graph.interactionRevision) return;
    const dx = event.clientX - active.x, dy = event.clientY - active.y;
    if (!active.moved && Math.hypot(dx, dy) < 4) return;
    if (!active.moved && active.type === "nodes" && !state.selectedStepPaths?.has(active.key) && pathKey(state.selectedStepPath || []) !== active.key) chooseNode(active.element);
    active.moved = true;
    if (active.type === "nodes") {
      for (const [key, position] of active.positions) { const node = graph.nodePositions.get(key); if (node) Object.assign(node, moveNodeByScreenDelta(position, graph, dx, dy)); }
      updateGraphGeometry();
    } else if (active.type === "pan") { graph.tx = active.tx + dx; graph.ty = active.ty + dy; applyGraphTransform(); }
    else if (active.type === "select") {
      const point = world(event), x = Math.min(active.start.x, point.x), y = Math.min(active.start.y, point.y), width = Math.abs(point.x - active.start.x), height = Math.abs(point.y - active.start.y);
      for (const [name, value] of Object.entries({ x, y, width, height })) active.marquee.setAttribute(name, value);
      const keys = [...graph.nodePositions.values()].filter((node) => node.x + node.w >= x && node.x <= x + width && node.y + node.h >= y && node.y <= y + height).map((node) => node.key);
      state.selectedStepPaths = new Set([...active.original, ...keys]); updateGraphSelection();
    }
  }
  on(svg, "pointerdown", (event) => {
    if (event.button !== 0 && event.button !== 1) return;
    const port = event.target.closest?.("button.graph-port");
    if (port && event.button === 0 && !space) { event.preventDefault(); if (port.dataset.portDirection === "output") startConnection(port, event); else finishConnection(port); return; }
    if (connection) { cancelConnection(); graphStatus("已取消连线"); return; }
    if (event.target.closest?.("[data-edge-id]") && event.button === 0) return;
    const element = nodeElement(event.target), node = element && graph.nodePositions?.get(element.dataset.stepPath);
    if (event.target.closest?.("button,input,textarea") && event.button === 0 && !space) return;
    event.preventDefault(); svg.focus({ preventScroll: true });
    const base = { x: event.clientX, y: event.clientY, pointerId: event.pointerId, moved: false, flowKey: graphLayoutKey(), source: state.source, revision: graph.interactionRevision };
    if (node && event.button === 0 && !space) {
      if (!node.layoutKey) { chooseNode(element); graphStatus("节点 ID 缺失或重复，请先修复 ID 再移动"); return; }
      const keys = state.selectedStepPaths?.has(node.key) ? [...state.selectedStepPaths] : [node.key];
      gesture = { ...base, type: "nodes", key: node.key, element, additive: event.ctrlKey || event.metaKey || event.shiftKey, positions: new Map(keys.map((key) => [key, { x: graph.nodePositions.get(key)?.x, y: graph.nodePositions.get(key)?.y }]).filter(([,pos]) => Number.isFinite(pos.x))) };
      svg.classList.add("is-dragging-node");
    } else if (event.shiftKey && event.button === 0) {
      const marquee = document.createElementNS("http://www.w3.org/2000/svg", "rect"); marquee.setAttribute("class", "graph-selection-rect"); $("graphRoot").appendChild(marquee);
      gesture = { ...base, type: "select", start: world(event), marquee, original: event.ctrlKey || event.metaKey ? [...state.selectedStepPaths] : [] };
    } else { gesture = { ...base, type: "pan", tx: graph.tx, ty: graph.ty }; svg.classList.add("is-panning"); }
    graph.isInteracting = true;
    try { svg.setPointerCapture(event.pointerId); } catch { /* Window listeners finish foreignObject gestures. */ }
  });
  on(window, "pointermove", (event) => {
    if (connection) { const point = graphPortPoint(connection.node, { direction: "output", kind: connection.kind }); preview?.setAttribute("d", edgePath(point, world(event))); }
    if (!gesture || event.pointerId !== gesture.pointerId) return;
    lastEvent = event;
    if (frame === null) frame = requestAnimationFrame(() => { frame = null; paintGesture(lastEvent); });
  });
  on(window, "pointerup", (event) => {
    if (connection) { const port = document.elementFromPoint(event.clientX, event.clientY)?.closest?.('button.graph-port[data-port-direction="input"]'); if (port) finishConnection(port); }
    if (gesture && event.pointerId === gesture.pointerId) { lastEvent = event; clearGesture(); }
  });
  on(window, "pointercancel", () => { clearGesture(true); cancelConnection(); });
  on(window, "blur", () => { space = false; clearGesture(true); cancelConnection(); });
  on(svg, "click", (event) => {
    const port = event.target.closest?.("button.graph-port");
    if (port && event.detail === 0) { if (port.dataset.portDirection === "output") startConnection(port); else finishConnection(port); return; }
    const edge = graph.edges?.find((item) => item.id === event.target.closest?.("[data-edge-id]")?.dataset.edgeId);
    if (!edge) return;
    if (edge.kind === "sequence") openGraphPalette({ afterPath: graph.nodePositions.get(edge.from).path });
    else if (edge.kind === "data") connectGraphPorts({ fromPath: graph.nodePositions.get(edge.from).path, toPath: graph.nodePositions.get(edge.to).path, input: edge.input, disconnect: true });
    else { selectStep(graph.nodePositions.get(edge.to).path); graphStatus(`${edge.label} · 在属性中编辑控制节点`); }
  });
  on(svg, "dblclick", (event) => { if (!nodeElement(event.target) && !event.target.closest?.("[data-edge-id]")) openGraphPalette({ point: world(event) }); });
  on(svg, "wheel", (event) => {
    event.preventDefault(); if (gesture) return;
    if (event.ctrlKey || event.metaKey) { const point = relative(event); Object.assign(graph, zoomGraphAt(graph, point.x, point.y, Math.exp(-Math.max(-100, Math.min(100, event.deltaY)) * 0.008))); }
    else { const unit = event.deltaMode === 1 ? 16 : event.deltaMode === 2 ? svg.clientHeight : 1; graph.tx -= event.deltaX * unit; graph.ty -= event.deltaY * unit; }
    applyGraphTransform();
  }, { passive: false });
  on(svg, "keydown", (event) => {
    if (event.isComposing || editable(event.target)) return;
    if (event.target.closest?.("button") && event.key !== "Escape") return;
    if ((event.ctrlKey || event.metaKey || event.altKey) && event.key !== "Escape") return;
    if (event.key === "Escape") {
      const active = gesture || connection || state.selectedStepPath || state.selectedStepPaths.size;
      event.preventDefault(); event.stopPropagation(); clearGesture(true); cancelConnection(); closeGraphPalette(); closeGraphConfirmation(); state.selectedStepPaths.clear(); selectStep(null);
      if (!active && svg.closest(".shell")?.classList.contains("is-canvas-focus")) $("graphFocusMode")?.click();
    }
    else if (event.key === " ") { event.preventDefault(); space = true; }
    else if (event.key.toLowerCase() === "f") { event.preventDefault(); fitGraphSelection(event.shiftKey); }
    else if (event.key === "+" || event.key === "=") { event.preventDefault(); zoom(1.2); }
    else if (event.key === "-") { event.preventDefault(); zoom(1 / 1.2); }
    else if (event.key === "0") { event.preventDefault(); graph.scale = 1; applyGraphTransform(); }
    else if (event.key === "Enter" && nodeElement(event.target)) { event.preventDefault(); chooseNode(nodeElement(event.target)); $("inspectorBody")?.querySelector("input")?.focus(); }
    else if (event.key.toLowerCase() === "a" && !(event.metaKey || event.ctrlKey || event.altKey)) { event.preventDefault(); openGraphPalette(); }
  });
  on(window, "keyup", (event) => { if (event.key === " ") space = false; });
  on(svg, "dragover", (event) => { if ([...event.dataTransfer.types].includes("text/x-lumo-action")) { event.preventDefault(); event.dataTransfer.dropEffect = "copy"; } });
  on(svg, "drop", (event) => {
    const action = event.dataTransfer.getData("text/x-lumo-action"); if (!action) return;
    event.preventDefault();
    const edge = graph.edges?.find((item) => item.id === event.target.closest?.("[data-edge-id]")?.dataset.edgeId);
    addGraphAction(action, edge?.kind === "sequence" ? { afterPath: graph.nodePositions.get(edge.from).path } : { point: world(event) });
  });
  on($("graphZoomIn"), "click", () => zoom(1.2)); on($("graphZoomOut"), "click", () => zoom(1 / 1.2));
  on($("graphZoomReset"), "click", () => { const rect = svg.getBoundingClientRect(); Object.assign(graph, zoomGraphAt(graph, rect.width / 2, rect.height / 2, 1 / graph.scale)); applyGraphTransform(); });
  on($("graphFit"), "click", () => fitGraphSelection()); on($("graphFocus"), "click", () => fitGraphSelection(true));
  on($("graphArrange"), "click", () => { if (!graph.nodePositions?.size) return; snapshotSource("graph-arrange"); state.graphLayouts[graphLayoutKey()] = {}; saveGraphLayouts(); renderGraph(); fitGraphSelection(); graphStatus("已自动布局 · 可撤销"); });
  on($("graphAddNode"), "click", () => openGraphPalette());
  on($("graphFocusMode"), "click", () => {
    const on = svg.closest(".shell").classList.toggle("is-canvas-focus");
    $("graphFocusMode").setAttribute("aria-pressed", String(on)); $("graphFocusMode").textContent = on ? "退出专注" : "专注画布";
    $("rightRail")?.classList.remove("is-graph-open"); $("graphInspectorToggle")?.setAttribute("aria-expanded", "false");
    requestAnimationFrame(() => { resizeGraphViewport(); fitGraphSelection(); });
  });
  on($("graphInspectorToggle"), "click", () => {
    const open = $("rightRail")?.classList.toggle("is-graph-open");
    $("graphInspectorToggle").setAttribute("aria-expanded", String(!!open));
  });
  if ($("rightRail") && !$("graphInspectorClose")) {
    const close = document.createElement("button"); close.id = "graphInspectorClose"; close.className = "graph-inspector-close"; close.type = "button"; close.textContent = "关闭节点属性 ×";
    $("rightRail").prepend(close);
    on(close, "click", () => { $("rightRail").classList.remove("is-graph-open"); $("graphInspectorToggle")?.setAttribute("aria-expanded", "false"); svg.focus(); });
  }
  svg.closest(".graph-wrap")?.querySelectorAll("[data-graph-add]").forEach((button) => on(button, "click", () => openGraphPalette()));
  const observer = typeof ResizeObserver === "function" ? new ResizeObserver(resizeGraphViewport) : null; observer?.observe(svg);
  const dispose = () => { clearGesture(true); cancelConnection(); observer?.disconnect(); for (const off of listeners) off(); mounted.delete(svg); };
  on(window, "pagehide", dispose); mounted.set(svg, dispose); return dispose;
}
