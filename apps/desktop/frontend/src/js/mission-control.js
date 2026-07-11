import { readyTopology } from "./agent-events.js";
import { renderConfirmation, renderRunControls, runAgentControl } from "./agent-confirmation.js";
import { renderAgentLog } from "./agent-feedback.js";

const escapeHtml = (value) => String(value ?? "").replaceAll("&", "&amp;").replaceAll("<", "&lt;").replaceAll(">", "&gt;").replaceAll('"', "&quot;").replaceAll("'", "&#39;");
const secretKey = (key) => /(token|secret|password|authorization|cookie|api.?key)/i.test(key);

function redact(value, key = "") {
  if (secretKey(key)) return "••••••••";
  if (Array.isArray(value)) return value.map((item) => redact(item));
  if (value && typeof value === "object") return Object.fromEntries(Object.entries(value).map(([k, v]) => [k, redact(v, k)]));
  return value;
}

export function renderTopology(topology) {
  if (!topology.lanes?.length) return '<div class="mc-empty">等待 Agent 执行事件…</div>';
  return `<div class="mc-lanes">${topology.lanes.map((lane, index) => `<section class="mc-lane" data-level="${index}"><span class="mc-lane-label">L${index}</span>${lane.map((node) => `<button class="mc-node is-${escapeHtml(node.status)} ${topology.activeNodeId === node.id ? "is-active" : ""}" data-node-id="${escapeHtml(node.id)}" aria-label="${escapeHtml(node.capabilityId)} ${escapeHtml(node.status)}"><i></i><strong>${escapeHtml(node.capabilityId)}</strong><span>${escapeHtml(node.id)} · #${escapeHtml(node.attempt || 1)}</span></button>`).join("")}</section>`).join('<div class="mc-flow-line" aria-hidden="true"></div>')}</div>`;
}

export function renderNodeDetail(node) {
  if (!node) return '<div class="mc-empty">选择节点查看输入、权限、耗时和结果</div>';
  return `<div class="mc-detail-head"><span>${escapeHtml(node.status).toUpperCase()}</span><h3>${escapeHtml(node.capabilityId)}</h3><p>${escapeHtml(node.id)} · attempt ${escapeHtml(node.attempt || 1)}</p></div><pre>${escapeHtml(JSON.stringify(redact(node.payload || {}), null, 2))}</pre>`;
}

export function renderRunMetrics(projection) {
  const values = Object.values(projection.nodes || {});
  const completed = values.filter((node) => node.status === "completed").length;
  const failed = values.filter((node) => node.status === "failed").length;
  return `<span>${escapeHtml(String(projection.status || "idle").toUpperCase())}</span><strong>${completed}/${values.length}</strong><span>${failed} failed · seq ${projection.seq || 0}</span>`;
}

export function updateMissionControl(root, projection) {
  if (!root) return;
  const topology = readyTopology(projection);
  root.querySelector("[data-mc-topology]").innerHTML = renderTopology(topology);
  const selected = root.dataset.selectedNode || topology.activeNodeId;
  const pending = [...(projection.events || [])].reverse().find((event) => event.kind === "permission.requested" && !event.resolved);
  root.querySelector("[data-mc-detail]").innerHTML = `${pending ? renderConfirmation({ runId: projection.runId, nodeId: pending.nodeId || pending.node_id, ...(pending.payload || {}) }) : renderNodeDetail(projection.nodes?.[selected])}${renderRunControls(projection)}`;
  root.querySelector("[data-mc-metrics]").innerHTML = renderRunMetrics(projection);
  root.querySelector("[data-mc-log]").innerHTML = renderAgentLog(projection.events || []);
  root.classList.toggle("is-replanning", projection.status === "replanning");
}

export function mountMissionControl(root, projection, call = async () => {}) {
  if (!root) return;
  root.addEventListener("click", (event) => {
    const control = event.target.closest("[data-agent-action]");
    if (control) {
      control.disabled = true;
      runAgentControl({ action: control.dataset.agentAction, runId: control.dataset.runId, nodeId: control.dataset.nodeId, planHash: control.dataset.planHash }, call, () => root.querySelector("[data-plan-hash]")?.dataset.planHash).finally(() => { control.disabled = false; });
      return;
    }
    const node = event.target.closest("[data-node-id]");
    if (!node) return;
    root.dataset.selectedNode = node.dataset.nodeId;
    updateMissionControl(root, projection);
  });
  updateMissionControl(root, projection);
}
