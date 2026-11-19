import { $, html } from "../dom.js";
import { state, graph } from "../state.js";
import { pathKey } from "../yaml.js";
import { switchEditorMode, switchTopView } from "../views.js";
import { selectStep } from "./inspector.js";
import { snapshotSource } from "./history.js";
import { graphBounds, searchGraphNodes, alignedPositions, studioLayout } from "./graph-presentation.js";
import { currentGraphLayouts, saveGraphLayouts, renderGraph, fitGraphSelection, applyGraphTransform, graphStatus, graphLayoutKey } from "./graph.js";

const NS = "http://www.w3.org/2000/svg";
const mounted = new WeakSet();
const miniElements = new Map();
let backView = null;
const selectedNodes = () => {
  const keys = state.selectedStepPaths?.size ? state.selectedStepPaths : new Set([pathKey(state.selectedStepPath || [])]);
  return [...(graph.nodePositions?.values() || [])].filter((node) => keys.has(node.key));
};
const savePresentation = (message) => { saveGraphLayouts(); renderGraph(); graphStatus(message); };

function ensureCurrentGraph() {
  if (state.currentView !== "design") switchTopView("design");
  if (state.viewMode !== "graph") switchEditorMode("graph");
  else if (graph.modelFlowKey !== graphLayoutKey() || graph.modelSource !== state.source) renderGraph();
}

export function focusGraphNode(key, { remember = true } = {}) {
  ensureCurrentGraph();
  if (graph.modelFlowKey !== graphLayoutKey() || graph.modelSource !== state.source) return;
  const node = graph.allNodePositions?.get(key); if (!node) return;
  if (remember) backView = { flow: graphLayoutKey(), tx: graph.tx, ty: graph.ty, scale: graph.scale };
  const layouts = currentGraphLayouts();
  let expanded = false;
  for (const parent of graph.allNodePositions.values()) {
    if (node.path.length > parent.path.length && parent.path.every((part, i) => node.path[i] === part) && layouts[parent.layoutKey]?.collapsed) { layouts[parent.layoutKey].collapsed = false; expanded = true; }
  }
  for (const group of studioLayout(layouts).groups) if (group.collapsed && group.members.includes(node.layoutKey)) { group.collapsed = false; expanded = true; }
  if (expanded) { saveGraphLayouts(); renderGraph(); }
  selectStep(node.path);
  // Navigation keeps its return/search controls reachable in compact windows.
  $("rightRail")?.classList.remove("is-graph-open"); $("graphInspectorToggle")?.setAttribute("aria-expanded", "false");
  fitGraphSelection(true);
  $("graphSearchPanel")?.setAttribute("hidden", "");
  const card = [...($("graphRoot")?.querySelectorAll(".graph-node") || [])].find((el) => el.dataset.stepPath === key);
  card?.focus({ preventScroll: true });
  if ($("graphBack")) $("graphBack").disabled = false;
}

export function toggleGraphCollapse(nodeKey, groupId) {
  const layouts = currentGraphLayouts();
  if (groupId) {
    const group = studioLayout(layouts).groups.find((entry) => entry.id === groupId); if (!group) return;
    snapshotSource("graph-group-collapse"); group.collapsed = !group.collapsed;
  } else {
    const node = graph.allNodePositions?.get(nodeKey); if (!node?.layoutKey || !node.branches.length) return;
    snapshotSource("graph-collapse"); layouts[node.layoutKey] = { ...layouts[node.layoutKey], collapsed: !layouts[node.layoutKey]?.collapsed };
  }
  savePresentation("已更新折叠视图 · 执行顺序保持一致");
}

export function renderGraphMinimap() {
  const minimap = $("graphMinimap"); if (!minimap || !graph.nodePositions) return;
  const bounds = graphBounds(graph.nodePositions.values(), 36);
  minimap.setAttribute("viewBox", `${bounds.x} ${bounds.y} ${Math.max(1, bounds.width)} ${Math.max(1, bounds.height)}`);
  const fragment = document.createDocumentFragment();
  miniElements.clear();
  for (const node of graph.nodePositions.values()) {
    const rect = document.createElementNS(NS, "rect");
    for (const [name, value] of Object.entries({ x: node.x, y: node.y, width: node.w, height: node.h, rx: 9, "data-mini-node": node.key, class: `graph-mini-node${node.runState === "failed" ? " is-failed" : node.runState === "running" ? " is-running" : ""}` })) rect.setAttribute(name, value);
    fragment.appendChild(rect); miniElements.set(node.key, rect);
  }
  const viewport = document.createElementNS(NS, "rect"); viewport.id = "graphMinimapViewport"; viewport.setAttribute("class", "graph-mini-viewport"); fragment.appendChild(viewport);
  minimap.replaceChildren(fragment); updateGraphMinimapViewport();
}

export function updateGraphMinimapState(key, status) {
  const element = miniElements.get(key); if (!element) return;
  element.classList.toggle("is-failed", status === "failed"); element.classList.toggle("is-running", status === "running");
}

export function updateGraphMinimapViewport() {
  const rect = $("graphSvg")?.getBoundingClientRect(), view = $("graphMinimapViewport");
  if (!rect || !view) return;
  for (const [name, value] of Object.entries({ x: -graph.tx / graph.scale, y: -graph.ty / graph.scale, width: rect.width / graph.scale, height: rect.height / graph.scale })) view.setAttribute(name, value);
}

export function mountGraphNavigation() {
  const wrap = $("graphSvg")?.closest(".graph-wrap"), toolbar = wrap?.querySelector(".graph-studio-actions");
  if (!toolbar || mounted.has(wrap)) return;
  mounted.add(wrap);
  document.addEventListener("lumo:focus-node", (event) => {
    ensureCurrentGraph();
    const path = event.detail?.path;
    const node = [...(graph.allNodePositions?.values() || [])].find((node) => node.runtimePath === path || node.key === event.detail?.key);
    if (node) focusGraphNode(node.key);
  });
  const drawer = document.createElement("aside"); drawer.id = "graphResultDrawer"; drawer.className = "graph-result-drawer"; drawer.hidden = true; drawer.setAttribute("aria-label", "节点执行结果");
  drawer.innerHTML = '<header><div><small>EXECUTION INSPECTOR</small><strong>节点执行结果</strong></div><button id="graphResultClose" type="button" aria-label="关闭节点结果">×</button></header><div id="graphResultDetail" class="timeline-detail"></div>';
  wrap.appendChild(drawer);
  const closeResult = () => { drawer.hidden = true; wrap.classList.remove("has-result"); document.dispatchEvent(new CustomEvent("lumo:graph-result-closed")); $("graphSvg")?.focus({ preventScroll: true }); };
  $("graphResultClose").onclick = closeResult;
  drawer.onkeydown = (event) => { if (event.key === "Escape") { event.stopPropagation(); closeResult(); } };
  document.addEventListener("lumo:graph-result-shown", () => {
    drawer.hidden = false; wrap.classList.add("has-result");
    $("rightRail")?.classList.remove("is-graph-open"); $("graphInspectorToggle")?.setAttribute("aria-expanded", "false");
  });
  document.addEventListener("lumo:step-selected", (event) => { if (!event.detail?.path) { drawer.hidden = true; wrap.classList.remove("has-result"); } });
  for (const [id, text, title] of [["graphSearchToggle", "查找", "查找节点 · /"], ["graphGroup", "分组", "将选中的节点放入命名分组"], ["graphAlign", "对齐", "选中节点顶部对齐；Shift 点击左对齐"], ["graphSnap", "吸附", "按 12 单位网格吸附；拖动时按 Alt 临时关闭"], ["graphBack", "返回", "返回查找前的视图"]]) {
    const button = document.createElement("button"); Object.assign(button, { id, textContent: text, title, type: "button" }); toolbar.appendChild(button);
  }
  $("graphBack").disabled = true;
  const search = document.createElement("section"); search.id = "graphSearchPanel"; search.className = "graph-palette graph-node-search"; search.hidden = true; search.setAttribute("aria-label", "查找已有节点");
  search.innerHTML = '<header><span>查找节点</span><button type="button" data-search-close aria-label="关闭查找">×</button></header><label class="graph-palette-search"><input id="graphNodeSearch" type="search" aria-label="搜索节点 ID、动作和参数" placeholder="节点 ID、动作或参数" autocomplete="off" /></label><div id="graphNodeSearchResults" class="graph-palette-results"></div><footer>Enter 定位 · Esc 关闭 · 返回可恢复原视图</footer>';
  wrap.appendChild(search);
  const results = $("graphNodeSearchResults"), query = $("graphNodeSearch");
  const updateSearch = () => {
    const matches = searchGraphNodes(graph.allNodePositions || new Map(), query.value);
    results.innerHTML = matches.map((node) => `<button type="button" data-search-node="${html(node.key)}"><strong>${html(node.step.id)}</strong><small>${html(node.step.action)}${graph.nodePositions.has(node.key) ? "" : " · 已折叠"}</small></button>`).join("") || '<p class="graph-palette-empty">输入关键词查找节点，最多显示 60 项</p>';
  };
  $("graphSearchToggle").onclick = () => { search.hidden = !search.hidden; if (!search.hidden) { updateSearch(); query.focus(); } };
  query.oninput = updateSearch;
  search.querySelector("[data-search-close]").onclick = () => { search.hidden = true; $("graphSearchToggle").focus(); };
  results.onclick = (event) => { const button = event.target.closest("[data-search-node]"); if (button) focusGraphNode(button.dataset.searchNode); };
  search.onkeydown = (event) => {
    if (event.key === "Escape") { event.stopPropagation(); search.hidden = true; $("graphSearchToggle").focus(); }
    if (event.key === "Enter" && event.target === query) { event.preventDefault(); results.querySelector("button")?.click(); }
    if (event.key === "ArrowDown" && event.target === query) { event.preventDefault(); results.querySelector("button")?.focus(); }
  };
  $("graphBack").onclick = () => { if (backView?.flow === graphLayoutKey()) { Object.assign(graph, { tx: backView.tx, ty: backView.ty, scale: backView.scale }); applyGraphTransform(); } backView = null; $("graphBack").disabled = true; };
  $("graphSnap").onclick = () => { const studio = studioLayout(currentGraphLayouts()); studio.snap = studio.snap === false; $("graphSnap").setAttribute("aria-pressed", String(studio.snap)); saveGraphLayouts(); };
  $("graphAlign").onclick = (event) => {
    const nodes = selectedNodes(); if (nodes.length < 2) { graphStatus("请先多选至少两个节点，再对齐"); return; }
    snapshotSource("graph-align"); const layouts = currentGraphLayouts();
    for (const [key, position] of alignedPositions(nodes, event.shiftKey ? "left" : "top")) {
      const node = graph.nodePositions.get(key);
      for (const member of [node, ...(node.members || []).map((key) => graph.allNodePositions.get(key))]) {
        if (member?.layoutKey) layouts[member.layoutKey] = { ...layouts[member.layoutKey], x: member.x + position.x - node.x, y: member.y + position.y - node.y };
      }
    }
    savePresentation("已对齐选中节点 · 可撤销");
  };
  const groupForm = document.createElement("form"); groupForm.id = "graphGroupForm"; groupForm.className = "graph-connect-confirm"; groupForm.hidden = true;
  groupForm.innerHTML = '<label>分组名称<input name="title" required maxlength="80" value="新分组" /></label><label>标记颜色<select name="color"><option value="sage">绿</option><option value="blue">蓝</option><option value="amber">金</option><option value="rose">红</option></select></label><div><button type="button" data-group-cancel>取消</button><button type="submit" class="graph-add-button">保存分组</button></div>';
  wrap.appendChild(groupForm);
  let pendingGroup = null;
  $("graphGroup").onclick = () => {
    const selected = selectedNodes();
    const roots = [...selected, ...selected.flatMap((node) => (node.members || []).map((key) => graph.allNodePositions.get(key)).filter(Boolean))];
    const members = [...(graph.allNodePositions?.values() || [])].filter((node) => roots.some((parent) => parent.path.every((part, index) => node.path[index] === part))).map((node) => node.layoutKey).filter(Boolean);
    if (members.length < 2) { graphStatus("按 Shift 框选或 Ctrl / ⌘ 多选至少两个节点"); return; }
    pendingGroup = { flow: graphLayoutKey(), source: state.source, members }; groupForm.hidden = false; groupForm.elements.title.focus(); groupForm.elements.title.select();
  };
  groupForm.querySelector("[data-group-cancel]").onclick = () => { groupForm.hidden = true; pendingGroup = null; };
  groupForm.onkeydown = (event) => { if (event.key === "Escape") { event.preventDefault(); groupForm.hidden = true; pendingGroup = null; } };
  groupForm.onsubmit = (event) => {
    event.preventDefault(); const pending = pendingGroup; groupForm.hidden = true; pendingGroup = null;
    if (!pending || pending.flow !== graphLayoutKey() || pending.source !== state.source) { graphStatus("流程已变化，请重新选择分组节点"); return; }
    const title = groupForm.elements.title.value.trim(); if (!title) return;
    snapshotSource("graph-group"); const studio = studioLayout(currentGraphLayouts());
    // Keep one visual owner per node, so collapsing cannot produce ambiguous membership.
    for (const group of studio.groups) group.members = group.members.filter((key) => !pending.members.includes(key));
    studio.groups = studio.groups.filter((group) => group.members.length);
    studio.groups.push({ id: `group-${Date.now()}-${Math.random().toString(36).slice(2, 7)}`, title, color: groupForm.elements.color.value, members: pending.members, collapsed: false });
    savePresentation("已创建分组 · 点击分组标题折叠，× 解除分组");
  };
  const mini = document.createElement("div"); mini.className = "graph-minimap";
  mini.innerHTML = '<span>流程概览</span><svg id="graphMinimap" role="img" aria-label="流程缩略图，点击定位；也可使用查找按钮导航"></svg>';
  wrap.appendChild(mini);
  $("graphMinimap").onclick = (event) => {
    const key = event.target.dataset.miniNode; if (key) { focusGraphNode(key); return; }
    const matrix = $("graphMinimap").getScreenCTM(); if (!matrix) return;
    const point = new DOMPoint(event.clientX, event.clientY).matrixTransform(matrix.inverse()), rect = $("graphSvg").getBoundingClientRect();
    graph.tx = rect.width / 2 - point.x * graph.scale; graph.ty = rect.height / 2 - point.y * graph.scale; applyGraphTransform();
  };
  $("graphRoot").addEventListener("click", (event) => {
    const control = event.target.closest("[data-collapse-node],[data-collapse-group],[data-delete-group]"); if (!control) return;
    event.preventDefault(); event.stopPropagation();
    if (control.dataset.deleteGroup) {
      snapshotSource("graph-ungroup"); const studio = studioLayout(currentGraphLayouts()); studio.groups = studio.groups.filter((g) => g.id !== control.dataset.deleteGroup); savePresentation("已解除分组 · 可撤销");
    } else toggleGraphCollapse(control.dataset.collapseNode, control.dataset.collapseGroup);
  });
}
