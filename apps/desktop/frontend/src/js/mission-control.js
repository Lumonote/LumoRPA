import { applyAgentEvent, createRunProjection, readyTopology } from "./agent-events.js";
import { renderConfirmation, renderRunControls, runAgentControl } from "./agent-confirmation.js";
import { renderAgentLog } from "./agent-feedback.js";

const escapeHtml = (value) => String(value ?? "").replaceAll("&", "&amp;").replaceAll("<", "&lt;").replaceAll(">", "&gt;").replaceAll('"', "&quot;").replaceAll("'", "&#39;");
const secretKey = (key) => /(token|secret|password|authorization|cookie|api.?key)/i.test(key);
const projections = new WeakMap();
const controlsInFlight = new WeakSet();
const missions = new WeakMap();
const terminal = (projection) => ["completed", "failed", "cancelled"].includes(projection?.status);

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
  return `<option value=""${selected === "" ? " selected" : ""}>跟随最新运行</option>${runs.map((run) => `<option value="${escapeHtml(run.id)}"${run.id === selected ? " selected" : ""}>${escapeHtml((run.utterance || run.id).slice(0, 48))} · ${escapeHtml(run.state || "unknown")}</option>`).join("")}`;
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

function clearNodeSelection(root) {
  delete root.dataset.selectedNode;
  delete root.dataset.startLane;
}

function renderProjection(root, projection) {
  if (!projection) return;
  if (projections.get(root)?.runId !== projection.runId) clearNodeSelection(root);
  projections.set(root, projection);
  const rawTopology = readyTopology(projection);
  const topology = virtualizeTopology(rawTopology, { startLane: Number(root.dataset.startLane || 0), laneCount: Number(root.dataset.laneCount || 12) });
  root.querySelector("[data-mc-topology]").innerHTML = renderTopology(topology);
  const selected = root.dataset.selectedNode || topology.activeNodeId;
  const pending = projection.pendingApprovals?.[0];
  root.querySelector("[data-mc-detail]").innerHTML = `${pending ? renderConfirmation({ runId: projection.runId, nodeId: pending.nodeId || pending.node_id, ...(pending.payload || {}) }) : renderNodeDetail(projection.nodes?.[selected])}${renderRunControls(projection)}<p data-mc-control-error role="status"></p>`;
  root.querySelector("[data-mc-metrics]").innerHTML = renderRunMetrics(projection);
  root.querySelector("[data-mc-log]").innerHTML = renderAgentLog(projection.events || []);
  root.classList.toggle("is-replanning", projection.status === "replanning");
}

function showHistoryError(state) {
  state.errorBox.textContent = [state.listError, state.selectionError].filter(Boolean).join("；");
}

function renderHistory(state) {
  const selected = state.root.dataset.selectedRun || "";
  const runs = [...state.history.values()];
  if (selected && !state.history.has(selected)) {
    runs.unshift({ id: selected, state: state.cache.get(selected)?.status || "unknown" });
  }
  state.selector.innerHTML = renderRunHistoryOptions(runs, selected);
  state.selector.value = selected;
}

async function refreshRunHistory(state) {
  if (state.refreshing) {
    state.refreshAgain = true;
    return;
  }
  state.refreshing = true;
  try {
    const runs = await state.call("agent_run_list", { limit: 50 });
    state.history = new Map((runs || []).map((run) => [run.id, run]));
    state.listError = "";
    renderHistory(state);
  } catch (error) {
    state.listError = `刷新运行历史失败：${error instanceof Error ? error.message : String(error)}`;
  } finally {
    showHistoryError(state);
    state.refreshing = false;
    if (state.refreshAgain) {
      state.refreshAgain = false;
      void refreshRunHistory(state);
    }
  }
}

export function updateMissionControl(root, projection) {
  if (!root || !projection) return;
  const state = missions.get(root);
  const previous = state?.cache.get(projection.runId) || projections.get(root);
  if (previous?.runId === projection.runId && previous.seq > projection.seq) projection = previous;
  if (!state) {
    if (!root.dataset.selectedRun || root.dataset.selectedRun === projection.runId) renderProjection(root, projection);
    return;
  }
  const latest = state.latest;
  state.cache.set(projection.runId, projection);
  state.latest = projection;
  if (!root.dataset.selectedRun || root.dataset.selectedRun === projection.runId) renderProjection(root, projection);
  if (latest?.runId !== projection.runId || (!terminal(latest) && terminal(projection))) void refreshRunHistory(state);
}

export function mountMissionControl(root, projection, call = async () => {}) {
  if (!root) return;
  if (missions.has(root)) { updateMissionControl(root, projection); return; }
  const selector = document.createElement("select");
  selector.dataset.mcRunSelect = "";
  selector.setAttribute("aria-label", "选择 Agent 运行历史");
  root.querySelector(".mc-head")?.append(selector);
  const errorBox = document.createElement("span");
  errorBox.dataset.mcHistoryError = "";
  errorBox.setAttribute("role", "status");
  root.querySelector(".mc-head")?.append(errorBox);
  const state = { root, call, selector, errorBox, latest: projection, cache: new Map(), history: new Map(), selectionRevision: 0, refreshing: false, refreshAgain: false, listError: "", selectionError: "" };
  missions.set(root, state);
  root.dataset.selectedRun = "";
  renderHistory(state);
  selector.addEventListener("change", async () => {
    const selected = selector.value;
    const revision = ++state.selectionRevision;
    root.dataset.selectedRun = selected;
    clearNodeSelection(root);
    state.selectionError = "";
    showHistoryError(state);
    selector.setAttribute("aria-busy", "false");
    renderHistory(state);
    if (!selected) { renderProjection(root, state.latest); return; }
    const cached = state.cache.get(selected);
    if (cached) renderProjection(root, cached);
    else {
      projections.delete(root);
      root.querySelector("[data-mc-topology]").innerHTML = '<div class="mc-empty">正在加载运行记录…</div>';
      root.querySelector("[data-mc-detail]").innerHTML = "";
      root.querySelector("[data-mc-metrics]").innerHTML = "";
      root.querySelector("[data-mc-log]").innerHTML = "";
      root.classList.toggle("is-replanning", false);
    }
    selector.setAttribute("aria-busy", "true");
    try {
      const events = await call("agent_events", { runId: selected, afterSeq: 0 });
      if (revision !== state.selectionRevision) return;
      let currentProjection = createRunProjection(selected);
      for (const event of events || []) currentProjection = applyAgentEvent(currentProjection, event);
      const fresh = state.cache.get(selected);
      if (fresh && fresh.seq >= currentProjection.seq) currentProjection = fresh;
      state.cache.set(selected, currentProjection);
      if (state.latest?.runId === selected) updateMissionControl(root, currentProjection);
      else renderProjection(root, currentProjection);
      renderHistory(state);
    } catch (error) {
      if (revision !== state.selectionRevision) return;
      state.selectionError = `加载运行记录失败：${error instanceof Error ? error.message : String(error)}`;
      showHistoryError(state);
    } finally {
      if (revision === state.selectionRevision) selector.setAttribute("aria-busy", "false");
    }
  });
  root.addEventListener("click", async (event) => {
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
      if (control.disabled || controlsInFlight.has(root)) return;
      controlsInFlight.add(root);
      control.disabled = true;
      const errorBox = root.querySelector("[data-mc-control-error]");
      if (errorBox) errorBox.textContent = "";
      try {
        await runAgentControl({ action: control.dataset.agentAction, runId: control.dataset.runId, nodeId: control.dataset.nodeId, planHash: control.dataset.planHash, risk: control.dataset.risk, strength: control.dataset.strength, capabilityId: control.dataset.capabilityId }, call, () => root.querySelector("[data-plan-hash]")?.dataset.planHash, (message) => window.confirm(message));
      } catch (error) {
        const currentErrorBox = projections.get(root)?.runId === control.dataset.runId
          ? root.querySelector("[data-mc-control-error]") : null;
        if (currentErrorBox) currentErrorBox.textContent = `操作失败：${error instanceof Error ? error.message : String(error)}`;
      } finally {
        control.disabled = false;
        controlsInFlight.delete(root);
      }
      return;
    }
    const node = event.target.closest("[data-node-id]");
    if (!node) return;
    root.dataset.selectedNode = node.dataset.nodeId;
    renderProjection(root, projections.get(root));
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
  updateMissionControl(root, projection);
  void refreshRunHistory(state);
}
