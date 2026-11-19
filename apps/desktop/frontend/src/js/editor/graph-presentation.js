// Presentation only: grouping, routing and collapse never rewrite the program.
export const GRAPH_GROUP_COLORS = ["sage", "blue", "amber", "rose"];
const isRecord = (value) => value !== null && typeof value === "object" && !Array.isArray(value);
const hasPosition = (value) => isRecord(value) && Number.isFinite(value.x) && Number.isFinite(value.y);

export const isValidGraphView = (view) => isRecord(view) && Number.isFinite(view.tx) && Number.isFinite(view.ty) && Number.isFinite(view.scale) && view.scale > 0;

// Layouts can come from old local caches or edited sidecars. Keep usable objects
// in place: active gestures and group controls may still hold their references.
export function normalizeGraphLayouts(layouts) {
  if (!isRecord(layouts)) return {};
  for (const [key, position] of Object.entries(layouts)) {
    if (key === "__studio") continue;
    if (!isRecord(position)) { delete layouts[key]; continue; }
    if (!hasPosition(position)) { delete position.x; delete position.y; }
    if (typeof position.collapsed !== "boolean") delete position.collapsed;
  }
  if (!Object.hasOwn(layouts, "__studio")) return layouts;
  const studio = studioLayout(layouts);
  const groupIds = new Set();
  let count = 0;
  for (const group of studio.groups) {
    if (!isRecord(group) || typeof group.id !== "string" || !group.id.trim() || !Array.isArray(group.members) || groupIds.has(group.id)) continue;
    groupIds.add(group.id);
    const members = new Set();
    let memberCount = 0;
    for (const key of group.members) {
      if (typeof key !== "string" || !key.trim() || members.has(key)) continue;
      members.add(key); group.members[memberCount++] = key;
    }
    group.members.length = memberCount;
    if (typeof group.title !== "string" || !group.title.trim()) group.title = "分组";
    if (!GRAPH_GROUP_COLORS.includes(group.color)) group.color = GRAPH_GROUP_COLORS[0];
    if (typeof group.collapsed !== "boolean") group.collapsed = false;
    studio.groups[count++] = group;
  }
  studio.groups.length = count;
  for (const [key, route] of Object.entries(studio.routes)) if (!hasPosition(route)) delete studio.routes[key];
  if (!isValidGraphView(studio.view)) delete studio.view;
  return layouts;
}

export function studioLayout(layouts) {
  layouts = isRecord(layouts) ? layouts : {};
  const saved = layouts.__studio;
  if (!isRecord(saved) || saved.version !== 1) layouts.__studio = { version: 1, groups: [], routes: {}, snap: true };
  const studio = layouts.__studio;
  if (!Array.isArray(studio.groups)) studio.groups = [];
  if (!isRecord(studio.routes)) studio.routes = {};
  if (typeof studio.snap !== "boolean") studio.snap = true;
  return studio;
}

export function graphBounds(nodes, padding = 0) {
  let x = Infinity, y = Infinity, right = -Infinity, bottom = -Infinity;
  for (const node of nodes) { x = Math.min(x, node.x); y = Math.min(y, node.y); right = Math.max(right, node.x + node.w); bottom = Math.max(bottom, node.y + node.h); }
  return Number.isFinite(x) ? { x: x - padding, y: y - padding, width: right - x + 2 * padding, height: bottom - y + 2 * padding } : { x: 0, y: 0, width: 0, height: 0 };
}

export const graphRouteKey = (edge, nodes) => JSON.stringify([nodes.get(edge.from)?.layoutKey, nodes.get(edge.to)?.layoutKey, edge.kind, edge.input || "", edge.inputPath || "", edge.outputPath || []]);
const isChild = (parent, child) => child.path.length > parent.path.length && parent.path.every((part, index) => child.path[index] === part);

export function projectGraphPresentation(model, layouts = {}) {
  layouts = normalizeGraphLayouts(layouts);
  const nodes = new Map([...model.nodes].map(([key, node]) => [key, { ...node }]));
  const studio = layouts.__studio?.version === 1 ? layouts.__studio : { groups: [], routes: {} };
  const hidden = new Map();
  // Outer controls win; descendants of an already hidden node share its owner.
  for (const node of nodes.values()) {
    if (!node.layoutKey || !layouts[node.layoutKey]?.collapsed || !node.branches.length || hidden.has(node.key)) continue;
    node.collapsed = true;
    node.members = [...nodes.values()].filter((child) => isChild(node, child)).map((child) => child.key);
    for (const key of node.members) hidden.set(key, node.key);
  }
  const frames = [];
  for (const group of studio.groups || []) {
    const members = new Set(Array.isArray(group.members) ? group.members : []);
    const visible = [...nodes.values()].filter((node) => members.has(node.layoutKey) && !hidden.has(node.key));
    if (!visible.length) continue;
    if (group.collapsed) {
      const owner = visible[0]; owner.group = group; owner.collapsed = true;
      owner.members = [...new Set(visible.flatMap((node) => [node.key, ...(node.members || [])]))].filter((key) => key !== owner.key);
      for (const key of owner.members) hidden.set(key, owner.key);
    } else frames.push({ ...group, ...graphBounds(visible, 22), nodeKeys: visible.map((node) => node.key) });
  }
  const ownerOf = (key) => { const seen = new Set(); while (hidden.has(key) && !seen.has(key)) { seen.add(key); key = hidden.get(key); } return key; };
  for (const key of hidden.keys()) nodes.delete(key);
  const edges = [];
  for (const edge of model.edges) {
    const from = ownerOf(edge.from), to = ownerOf(edge.to);
    if (from === to) continue;
    edges.push({ ...edge, from, to, originalFrom: edge.from, originalTo: edge.to, boundary: from !== edge.from || to !== edge.to, routeKey: graphRouteKey(edge, model.nodes) });
  }
  for (const node of nodes.values()) if (node.collapsed) {
    node.boundaryPorts = edges.filter((edge) => edge.from === node.key || edge.to === node.key).map((edge, row) => {
      const output = edge.from === node.key;
      const original = model.nodes.get(output ? edge.originalFrom : edge.originalTo);
      return { edgeId: edge.id, row, output, kind: edge.kind, label: `${original?.step.id || ""} · ${edge.kind === "data" ? output ? "result" : edge.input : "执行"}` };
    });
    node.h = 72 + Math.max(1, node.boundaryPorts.length) * 28 + 42;
  }
  return { nodes, edges, frames, hidden, bounds: graphBounds([...nodes.values(), ...frames.map((f) => ({ x: f.x, y: f.y - 30, w: f.width, h: f.height + 30 }))]) };
}

export function searchGraphNodes(nodes, query, limit = 60) {
  const needle = String(query).trim().toLocaleLowerCase();
  if (!needle) return [];
  const result = [];
  for (const node of nodes.values()) {
    let values = "";
    try { values = JSON.stringify(node.step.with || {}).slice(0, 10000); } catch { /* Recursive YAML parameters. */ }
    if (`${node.step.id} ${node.step.action} ${values}`.toLocaleLowerCase().includes(needle)) result.push(node);
    if (result.length >= limit) break;
  }
  return result;
}

export function alignedPositions(nodes, direction = "top") {
  const selected = [...nodes];
  const bounds = graphBounds(selected);
  return new Map(selected.map((node) => [node.key, { x: direction === "left" ? bounds.x : node.x, y: direction === "top" ? bounds.y : node.y }]));
}

export function snappedDelta(position, dx, dy, scale, enabled = true) {
  const delta = { x: dx / scale, y: dy / scale };
  if (!enabled) return delta;
  return { x: Math.round((position.x + delta.x) / 12) * 12 - position.x, y: Math.round((position.y + delta.y) / 12) * 12 - position.y };
}
