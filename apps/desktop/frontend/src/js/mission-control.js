import { applyAgentEvent, createRunProjection, readyTopology } from "./agent-events.js";
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

export function summarizeTopology(topology) {
  const lanes = topology.lanes || [];
  const nodes = lanes.reduce((total, lane) => total + lane.length, 0);
  const parallel = lanes.filter((lane) => lane.length > 1).length;
  return `${lanes.length} stages, ${nodes} nodes, ${parallel} parallel ${parallel === 1 ? "branch" : "branches"}`;
}

export function virtualizeTopology(topology, { startLane = 0, laneCount = 12, maxNodes = 160 } = {}) {
  const lanes = topology.lanes || [];
  const totalNodes = lanes.reduce((total, lane) => total + lane.length, 0);
  if (totalNodes <= maxNodes) return { ...topology, totalNodes, hiddenNodes: 0, visibleLevels: lanes.map((_, index) => index) };
  const activeLevel = lanes.findIndex((lane) => lane.some((node) => node.id === topology.activeNodeId));
  const indexes = new Set(Array.from({ length: laneCount }, (_, index) => startLane + index).filter((index) => index >= 0 && index < lanes.length));
  if (activeLevel >= 0) indexes.add(activeLevel);
  const visibleLevels = [...indexes].sort((a, b) => a - b);
  const visible = visibleLevels.map((index) => lanes[index]);
  const visibleNodes = visible.reduce((total, lane) => total + lane.length, 0);
  return { ...topology, lanes: visible, visibleLevels, totalNodes, hiddenNodes: totalNodes - visibleNodes };
}

export function renderTopology(topology) {
  if (!topology.lanes?.length) return '<div class="mc-empty">等待 Agent 执行事件…</div>';
  let position = 0;
  const total = topology.totalNodes || topology.lanes.reduce((sum, lane) => sum + lane.length, 0);
  const lanes = topology.lanes.map((lane, index) => {
    const level = topology.visibleLevels?.[index] ?? index;
    return `<section class="mc-lane" data-level="${level}"><span class="mc-lane-label">L${level}</span>${lane.map((node) => { position += 1; return `<button tabindex="0" class="mc-node is-${escapeHtml(node.status)} ${topology.activeNodeId === node.id ? "is-active" : ""}" data-node-id="${escapeHtml(node.id)}" aria-label="${escapeHtml(node.capabilityId)} ${escapeHtml(node.status)}" aria-posinset="${position}" aria-setsize="${total}"><i></i><strong>${escapeHtml(node.capabilityId)}</strong><span>${escapeHtml(node.id)} · #${escapeHtml(node.attempt || 1)}</span></button>`; }).join("")}</section>`;
  }).join('<div class="mc-flow-line" aria-hidden="true"></div>');
  return `<div class="mc-topology-tools"><span class="mc-topology-summary">${escapeHtml(summarizeTopology({ lanes: topology.lanes }))}</span><button data-mc-focus-active>FOCUS ACTIVE</button><button data-mc-zoom="out">−</button><button data-mc-zoom="in">+</button></div><div class="mc-minimap" data-mc-minimap aria-hidden="true"><i style="width:${Math.max(4, Math.round((total - (topology.hiddenNodes || 0)) / total * 100))}%"></i><span>${topology.hiddenNodes || 0} hidden</span></div><div class="mc-lanes" role="list" aria-label="${escapeHtml(summarizeTopology({ lanes: topology.lanes }))}">${lanes}</div>`;
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

export function renderRunHistoryOptions(runs = [], selected = "") {
  return runs.map((run) => `<option value="${escapeHtml(run.id)}"${run.id === selected ? " selected" : ""}>${escapeHtml((run.utterance || run.id).slice(0, 48))} · ${escapeHtml(run.state || "unknown")}</option>`).join("");
}

export function renderVoiceHistory(entries = []) {
  if (!entries.length) return '<div class="mc-empty">还没有语音指令记录，说一句「打开任务中心」试试。</div>';
  return `<ul class="mc-voice-list">${entries.slice(0, 20).map((entry) => `<li class="${entry.ok === false ? "is-bad" : "is-ok"}"><time>${escapeHtml(new Date(Number(entry.atMs) || 0).toLocaleTimeString())}</time><strong>${escapeHtml(entry.utterance || "")}</strong><span>${escapeHtml(entry.intent || "")}</span><em>${escapeHtml(entry.message || "")}</em></li>`).join("")}</ul>`;
}

export async function refreshVoiceHistory(scope, call) {
  const box = (scope || document).querySelector?.("[data-mc-voice-history]");
  if (!box) return;
  try {
    box.innerHTML = renderVoiceHistory(await call("voice_command_history"));
  } catch { /* 保留旧内容 */ }
}

export function updateMissionControl(root, projection) {
  if (!root) return;
  if (root.dataset.selectedRun && root.dataset.selectedRun !== projection.runId) return;
  const rawTopology = readyTopology(projection);
  const topology = virtualizeTopology(rawTopology, { startLane: Number(root.dataset.startLane || 0), laneCount: Number(root.dataset.laneCount || 12) });
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
  let currentProjection = projection;
  const selector = document.createElement("select");
  selector.dataset.mcRunSelect = "";
  selector.setAttribute("aria-label", "选择 Agent 运行历史");
  root.querySelector(".mc-head")?.append(selector);
  call("agent_run_list", { limit: 50 }).then((runs) => {
    selector.innerHTML = renderRunHistoryOptions(runs, projection.runId);
  }).catch(() => {});
  selector.addEventListener("change", async () => {
    root.dataset.selectedRun = selector.value;
    const events = await call("agent_events", { runId: selector.value, afterSeq: 0 });
    currentProjection = createRunProjection(selector.value);
    for (const event of events || []) currentProjection = applyAgentEvent(currentProjection, event);
    updateMissionControl(root, currentProjection);
  });
  root.addEventListener("click", (event) => {
    const zoom = event.target.closest("[data-mc-zoom]");
    if (zoom) {
      const current = Number(root.dataset.zoom || 1);
      root.dataset.zoom = String(Math.min(1.5, Math.max(0.55, current + (zoom.dataset.mcZoom === "in" ? 0.1 : -0.1))));
      root.style.setProperty("--mc-zoom", root.dataset.zoom);
      return;
    }
    if (event.target.closest("[data-mc-focus-active]")) {
      root.querySelector(".mc-node.is-active")?.focus();
      root.querySelector(".mc-node.is-active")?.scrollIntoView({ block: "center", inline: "center" });
      return;
    }
    const control = event.target.closest("[data-agent-action]");
    if (control) {
      control.disabled = true;
      runAgentControl({ action: control.dataset.agentAction, runId: control.dataset.runId, nodeId: control.dataset.nodeId, planHash: control.dataset.planHash }, call, () => root.querySelector("[data-plan-hash]")?.dataset.planHash).finally(() => { control.disabled = false; });
      return;
    }
    const node = event.target.closest("[data-node-id]");
    if (!node) return;
    root.dataset.selectedNode = node.dataset.nodeId;
    updateMissionControl(root, currentProjection);
  });
  root.addEventListener("keydown", (event) => {
    if (!["ArrowLeft", "ArrowRight", "ArrowUp", "ArrowDown"].includes(event.key)) return;
    const nodes = [...root.querySelectorAll("[data-node-id]")];
    const index = nodes.indexOf(event.target.closest("[data-node-id]"));
    if (index < 0) return;
    event.preventDefault();
    nodes[Math.min(nodes.length - 1, Math.max(0, index + (event.key === "ArrowLeft" || event.key === "ArrowUp" ? -1 : 1)))]?.focus();
  });
  refreshVoiceHistory(root, call);
  updateMissionControl(root, currentProjection);
}
