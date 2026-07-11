const clone = (value) => value == null ? value : structuredClone(value);

export function createRunProjection(runId) {
  return { runId, seq: 0, status: "idle", nodes: {}, edges: [], events: [], replans: [], activeNodeId: null, startedAt: null, finishedAt: null };
}

function projectPlan(projection, event, revised) {
  const nodes = { ...projection.nodes };
  for (const item of event.payload?.nodes || []) {
    const previous = nodes[item.id] || {};
    nodes[item.id] = {
      ...previous,
      ...clone(item),
      capabilityId: item.capabilityId || item.capability_id || previous.capabilityId || "unknown",
      dependsOn: [...(item.dependsOn || item.depends_on || previous.dependsOn || [])],
      status: item.status || previous.status || "queued",
      attempt: item.attempt || previous.attempt || 1,
      payload: previous.payload || {},
    };
  }
  return {
    ...projection,
    nodes,
    edges: Object.values(nodes).flatMap((node) => (node.dependsOn || []).map((from) => ({ from, to: node.id }))),
    status: revised ? "replanning" : "planned",
    replans: revised ? [...projection.replans, { seq: event.seq, reason: event.payload?.reason || "Plan revised" }] : projection.replans,
  };
}

export function applyAgentEvent(projection, event) {
  if (!event || !Number.isFinite(event.seq) || event.seq <= projection.seq) return projection;
  let next = { ...projection, seq: event.seq, events: [...projection.events, clone(event)].slice(-300) };
  if (event.kind === "plan.created") next = projectPlan(next, event, false);
  if (event.kind === "plan.revised") next = projectPlan(next, event, true);

  const nodeId = event.nodeId || event.node_id;
  if (nodeId) {
    const statusByKind = {
      "node.queued": "queued", "node.started": "running", "node.progress": "running",
      "tool.called": "running", "tool.result": "running", "node.completed": "completed",
      "node.failed": "failed", "node.cancelled": "cancelled", "node.waiting": "waiting",
      "node.unknown": "unknown",
    };
    const current = next.nodes[nodeId] || { id: nodeId, capabilityId: "unknown", dependsOn: [], status: "queued", attempt: 1, payload: {} };
    const payload = { ...(current.payload || {}), ...(clone(event.payload) || {}) };
    const attempt = event.payload?.attempt || current.attempt || 1;
    next = { ...next, nodes: { ...next.nodes, [nodeId]: { ...current, status: statusByKind[event.kind] || current.status, payload, attempt } } };
    if (["node.started", "node.progress", "tool.called", "tool.result"].includes(event.kind)) next.activeNodeId = nodeId;
    if (["node.completed", "node.failed", "node.cancelled"].includes(event.kind) && next.activeNodeId === nodeId) next.activeNodeId = null;
  }

  const runStates = {
    "session.started": "running", "run.started": "running", "run.paused": "paused", "run.resumed": "running",
    "run.completed": "completed", "run.failed": "failed", "run.cancelled": "cancelled",
    "permission.requested": "waiting", "permission.resolved": "running",
  };
  if (runStates[event.kind]) next.status = runStates[event.kind];
  if (event.kind === "session.started" || event.kind === "run.started") next.startedAt = event.timestamp || Date.now();
  if (["run.completed", "run.failed", "run.cancelled"].includes(event.kind)) next.finishedAt = event.timestamp || Date.now();
  return next;
}

export function readyTopology(projection) {
  const nodes = projection.nodes || {};
  const memo = new Map();
  const visiting = new Set();
  const depth = (id) => {
    if (memo.has(id)) return memo.get(id);
    if (visiting.has(id)) return 0;
    visiting.add(id);
    const deps = nodes[id]?.dependsOn || [];
    const value = deps.length ? Math.max(...deps.map((dep) => depth(dep) + 1)) : 0;
    visiting.delete(id); memo.set(id, value); return value;
  };
  const lanes = [];
  Object.values(nodes).sort((a, b) => a.id.localeCompare(b.id)).forEach((node) => {
    const level = depth(node.id); (lanes[level] ||= []).push(node);
  });
  return { lanes, edges: [...(projection.edges || [])], activeNodeId: projection.activeNodeId };
}
