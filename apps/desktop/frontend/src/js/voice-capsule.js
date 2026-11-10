import { recallHint } from "./voice-shortcut.js";

const escapeHtml = (value) => String(value ?? "").replaceAll("&", "&amp;").replaceAll("<", "&lt;").replaceAll(">", "&gt;").replaceAll('"', "&quot;").replaceAll("'", "&#39;");
const clamp = (value) => Number.isFinite(Number(value)) ? Math.max(0, Math.min(1, Number(value))) : 0;
const text = (value, limit = 6000) => String(value ?? "").slice(0, limit);
const mounts = new WeakMap();
const summaryText = (projection) => projection.partial || (["listening", "transcribing", "routing", "planning"].includes(projection.voiceState) && projection.transcript) || projection.message || projection.transcript;
const labels = { idle: "READY", disabled: "READY", listening: "LISTENING", transcribing: "TRANSCRIBING", routing: "ROUTING", planning: "PLANNING", confirming: "CONFIRM", executing: "EXECUTING", reporting: "REPORTING", success: "DONE", error: "ERROR", muted: "MUTED", paused: "PAUSED", cancelled: "STOPPED" };
const taskLabels = { idle: "等待指令", planning: "规划中", planned: "准备执行", running: "执行中", executing: "执行中", waiting: "等待审批", waiting_for_approval: "等待审批", waitingapproval: "等待审批", paused: "已暂停", completed: "已完成", failed: "执行失败", cancelled: "已停止", budget_exceeded: "预算待核对", budgetexceeded: "预算待核对" };
const activeTasks = new Set(["planning", "planned", "running", "executing", "waiting", "waiting_for_approval", "waitingapproval", "paused", "replanning"]);
const activeVoice = new Set(["wake_detected", "listening", "transcribing", "routing", "planning", "confirming", "executing", "reporting"]);
const terminalTasks = new Set(["completed", "failed", "cancelled", "budget_exceeded", "budgetexceeded"]);
const inputVoice = new Set(["wake_detected", "listening", "transcribing", "routing"]);
const voiceBusy = (projection) => projection.listening || (terminalTasks.has(projection.runState) ? inputVoice : activeVoice).has(projection.voiceState);

export function createVoiceProjection() {
  return { status: "idle", voiceState: "idle", partial: "", transcript: "", source: "LOCAL", progress: 0, level: 0, message: "随时可以唤醒 Lumo", muted: false, blocked: false, listening: false, recallHint: recallHint({}), activeRunId: null, runState: "idle", revision: -1, seq: 0, taskLabel: "", result: "", error: "", connected: false };
}

export function applyCapsuleSnapshot(projection, snapshot) {
  if (!snapshot || !Number.isSafeInteger(snapshot.revision) || snapshot.revision <= projection.revision) return projection;
  const voiceState = String(snapshot.state || "idle").toLowerCase();
  const runState = String(snapshot.runState || "idle").toLowerCase();
  let status = labels[voiceState] ? voiceState : "idle";
  if (activeTasks.has(runState)) status = ["waiting", "waiting_for_approval", "waitingapproval"].includes(runState) ? "confirming" : runState === "paused" ? "paused" : "executing";
  else if (!voiceBusy({ voiceState, runState, listening: snapshot.listening })) {
    if (runState === "completed") status = "success";
    else if (["failed", "budget_exceeded", "budgetexceeded"].includes(runState)) status = "error";
    else if (runState === "cancelled") status = "cancelled";
    else if (snapshot.muted) status = "muted";
  }
  return { ...projection, status, voiceState, runState, connected: true,
    revision: snapshot.revision, seq: Number.isSafeInteger(snapshot.seq) ? snapshot.seq : 0,
    activeRunId: typeof snapshot.activeRunId === "string" && snapshot.activeRunId ? snapshot.activeRunId : null,
    message: text(snapshot.message || "随时可以唤醒 Lumo"), partial: text(snapshot.partial), transcript: text(snapshot.transcript),
    source: text(snapshot.source || "local", 32).toUpperCase(), progress: clamp(snapshot.progress),
    muted: snapshot.muted === true, blocked: snapshot.blocked === true, listening: snapshot.listening === true,
    recallHint: recallHint(snapshot), taskLabel: text(snapshot.taskLabel, 500), result: text(snapshot.result), error: text(snapshot.error),
    level: snapshot.listening ? projection.level : 0,
  };
}

// Kept for callers projecting individual voice events. The mounted companion
// follows host-owned snapshots rather than global Agent events from other runs.
export function applyVoiceEvent(projection, event) {
  if (!event) return projection;
  if (event.kind === "voice.state") return { ...projection, status: labels[event.state] ? event.state : projection.status, message: event.message || projection.message };
  if (event.kind === "transcript.partial") return { ...projection, status: "transcribing", partial: event.text || "" };
  if (event.kind === "transcript.final") return { ...projection, status: "routing", partial: "", transcript: event.text || "", source: String(event.source || "local").toUpperCase() };
  if (event.kind === "agent.state") {
    if (event.runId && event.runId !== projection.activeRunId || Number.isFinite(event.seq) && event.seq <= projection.seq) return projection;
    return { ...projection, status: labels[event.state] ? event.state : projection.status, progress: clamp(event.progress ?? projection.progress), message: event.message || projection.message, seq: event.seq ?? projection.seq };
  }
  if (event.kind === "voice.level") return { ...projection, level: clamp(event.level) };
  if (event.kind === "voice.follow-up") return { ...projection, status: "listening", message: event.message || `继续说，我会等待 ${Number(event.timeoutSeconds) || 8} 秒` };
  if (event.kind === "voice.feedback") return { ...projection, status: event.ok === false ? "error" : "success", partial: "", transcript: "", message: event.message || projection.message, progress: event.ok === false ? projection.progress : 1 };
  if (event.kind === "voice.muted") return { ...projection, status: event.muted ? "muted" : "idle", muted: Boolean(event.muted) };
  return projection;
}

export function renderVoiceCapsule(projection, { expanded = false } = {}) {
  const status = labels[projection.status] ? projection.status : "idle";
  const summary = summaryText(projection);
  return `<div class="voice-companion" data-companion><section class="voice-capsule is-${status}" data-tauri-drag-region title="按住拖动可移到任意位置" aria-live="polite" aria-label="Lumo 任务助手"><div class="vc-orb" aria-hidden="true"><div class="vc-aura"></div><div class="vc-spectrum"><i></i><i></i><i></i><i></i></div><div class="vc-core"></div><div class="vc-particles"><i></i><i></i><i></i><i></i><i></i><i></i></div><b class="vc-ring ring-a"></b><b class="vc-ring ring-b"></b></div><div class="vc-copy"><div><span data-capsule-label>${labels[status]}</span><em data-capsule-source>${escapeHtml(projection.source)}</em></div><strong data-capsule-summary>${escapeHtml(summary)}</strong><div class="vc-wave" data-voice-meter style="--voice-level:${clamp(projection.level)}" aria-hidden="true">${Array.from({ length: 12 }, (_, index) => `<i style="--wave:${index};--wave-scale:${3 + index % 5}"></i>`).join("")}</div><div class="vc-progress" data-capsule-progress role="progressbar" aria-label="任务执行进度" aria-valuemin="0" aria-valuemax="100" aria-valuenow="${Math.round(clamp(projection.progress) * 100)}"><i data-capsule-progress-fill style="width:${clamp(projection.progress) * 100}%"></i></div></div><div class="vc-actions"><button type="button" data-voice-action="panel" aria-expanded="${expanded}" aria-controls="companionPanel" title="展开任务助手" aria-label="展开任务助手">⌨</button><button type="button" data-voice-action="cancel" title="停止并收起 ${escapeHtml(projection.recallHint || "")}" aria-label="停止当前任务并收起悬浮窗">×</button></div></section><section class="vc-panel" id="companionPanel" data-capsule-panel${expanded ? "" : " hidden"} aria-label="任务助手面板"><div class="vc-panel-heading"><strong>Lumo 任务助手</strong><span data-capsule-task-state>${escapeHtml(taskLabels[projection.runState] || "等待指令")}</span></div><div class="vc-task-content" data-capsule-content role="region" aria-label="任务进度与结果" tabindex="0"><p class="vc-task-label" data-capsule-task-label>${escapeHtml(projection.taskLabel)}</p><p class="vc-task-message" data-capsule-message aria-live="polite"${projection.result || projection.error ? " hidden" : ""}>${escapeHtml(projection.message)}</p><pre class="vc-result" data-capsule-result${projection.result || projection.error ? "" : " hidden"}>${escapeHtml(projection.error || projection.result)}</pre></div><div class="vc-task-actions"><button type="button" data-voice-action="listen">语音输入</button><button type="button" data-voice-action="mute" aria-pressed="${Boolean(projection.muted)}">${projection.muted ? "取消静音" : "麦克风静音"}</button><button type="button" data-voice-action="stop-task" disabled>停止任务</button><button type="button" data-voice-action="expand" disabled>查看详情</button></div><form data-capsule-form><label for="companionPrompt">文字指令<span>无需开启麦克风</span></label><textarea id="companionPrompt" data-capsule-input rows="2" maxlength="4000" placeholder="例如：运行日报流程并告诉我结果"></textarea><div class="vc-compose-footer"><small>Ctrl / ⌘ + Enter 发送 · 敏感操作仍需审批</small><button type="submit" data-capsule-submit disabled>发送</button></div></form><p class="vc-error" data-capsule-error role="alert" hidden></p></section></div>`;
}

export async function mountVoiceCapsule(root, {
  listen = globalThis.__TAURI__?.event?.listen,
  call = globalThis.__TAURI__?.core?.invoke,
  requestFrame = globalThis.requestAnimationFrame || ((callback) => setTimeout(callback, 16)),
  cancelFrame = globalThis.cancelAnimationFrame || clearTimeout,
  hostWindow = globalThis.window,
  visibilityTarget = globalThis.document,
} = {}) {
  mounts.get(root)?.dispose();
  let projection = createVoiceProjection();
  let disposed = false;
  let expanded = false;
  let frame = null;
  let draft = "";
  let localError = "";
  let submitting = false;
  const pending = new Set();
  const unlisteners = [];
  root.innerHTML = renderVoiceCapsule(projection);
  const find = (selector) => root.querySelector(selector);
  const input = find("[data-capsule-input]");
  const assign = (selector, value) => { const node = find(selector); if (node) node.textContent = value; };
  const button = (action) => find(`[data-voice-action="${action}"]`);
  const disable = (node, disabled) => { if (node) node.disabled = disabled; };
  const taskBusy = () => activeTasks.has(projection.runState);
  const busy = () => taskBusy() || voiceBusy(projection) || submitting;
  const updateLevel = () => { if (!disposed) find("[data-voice-meter]")?.style.setProperty("--voice-level", String(projection.level)); };
  const paintLevel = () => { frame = null; updateLevel(); };
  const paint = () => {
    if (disposed) return;
    const status = localError ? "error" : labels[projection.status] ? projection.status : "idle";
    const shell = find(".voice-capsule");
    if (shell) shell.className = `voice-capsule is-${status}`;
    assign("[data-capsule-label]", labels[status]);
    assign("[data-capsule-source]", projection.source);
    assign("[data-capsule-summary]", localError || summaryText(projection));
    assign("[data-capsule-task-state]", taskLabels[projection.runState] || (activeVoice.has(projection.voiceState) ? "正在处理指令" : "等待指令"));
    assign("[data-capsule-task-label]", projection.taskLabel);
    assign("[data-capsule-message]", projection.message);
    const message = find("[data-capsule-message]");
    if (message) message.hidden = Boolean(projection.result || projection.error);
    assign("[data-capsule-result]", projection.error || projection.result);
    const result = find("[data-capsule-result]");
    if (result) result.hidden = !(projection.error || projection.result);
    assign("[data-capsule-error]", localError);
    const error = find("[data-capsule-error]"); if (error) error.hidden = !localError;
    const percent = Math.round(projection.progress * 100);
    const progress = find("[data-capsule-progress]");
    progress?.setAttribute("aria-valuenow", String(percent));
    progress?.setAttribute("aria-valuetext", `${taskLabels[projection.runState] || "任务进度"} ${percent}%`);
    const fill = find("[data-capsule-progress-fill]"); if (fill) fill.style.width = `${percent}%`;
    const panel = find("[data-capsule-panel]"); if (panel) panel.hidden = !expanded;
    const toggle = button("panel");
    toggle?.setAttribute("aria-expanded", String(expanded));
    if (toggle) { toggle.title = expanded ? "收起任务面板" : "展开任务助手"; toggle.setAttribute("aria-label", toggle.title); }
    const close = button("cancel"); if (close) close.title = `停止并收起 ${projection.recallHint}`;
    const mute = button("mute");
    if (mute) { mute.textContent = projection.muted ? "取消静音" : "麦克风静音"; mute.setAttribute("aria-pressed", String(projection.muted)); }
    disable(button("listen"), !projection.connected || busy() || projection.muted || projection.blocked || pending.has("listen"));
    disable(mute, !projection.connected || pending.has("mute"));
    disable(button("stop-task"), !projection.activeRunId || !taskBusy() || pending.has("stop-task"));
    disable(button("expand"), !projection.activeRunId || pending.has("expand"));
    disable(button("cancel"), pending.has("cancel"));
    disable(toggle, pending.has("panel"));
    const send = find("[data-capsule-submit]");
    disable(send, !projection.connected || !draft.trim() || busy());
    if (send) send.textContent = submitting ? "发送中…" : "发送";
    find("[data-companion]")?.classList.toggle("is-background", visibilityTarget?.hidden === true);
    find("[data-companion]")?.classList.toggle("is-expanded", expanded);
    updateLevel();
  };
  const accept = (snapshot) => {
    const next = applyCapsuleSnapshot(projection, snapshot);
    if (next !== projection) { projection = next; if (typeof snapshot.expanded === "boolean" && !pending.has("panel")) expanded = snapshot.expanded; paint(); }
  };
  const invoke = async (command, args) => {
    if (!call) throw new Error("桌面服务暂不可用，请在应用内重试");
    return call(command, args);
  };
  const refresh = async () => {
    if (disposed) return;
    try { const snapshot = await invoke("capsule_status"); if (!disposed) accept(snapshot); }
    catch (error) { if (!disposed) { localError = `无法读取任务状态：${error instanceof Error ? error.message : String(error)}`; paint(); } }
  };
  const submit = async () => {
    const value = draft.trim();
    if (!value || busy() || !projection.connected || disposed) return;
    submitting = true; localError = ""; paint();
    try {
      const snapshot = await invoke("capsule_submit_text", { text: value });
      if (disposed) return;
      accept(snapshot);
      if (draft.trim() === value) { draft = ""; if (input) input.value = ""; }
    } catch (error) {
      if (!disposed) localError = `发送失败：${error instanceof Error ? error.message : String(error)}`;
    } finally { submitting = false; paint(); }
  };
  const onInput = (event) => {
    if (event.target !== input) return;
    draft = input.value; localError = ""; paint();
  };
  const onSubmit = (event) => {
    if (!event.target.closest?.("[data-capsule-form]")) return;
    event.preventDefault(); return submit();
  };
  const onKey = (event) => {
    if (event.target === input && event.key === "Enter" && (event.ctrlKey || event.metaKey) && !event.isComposing) {
      event.preventDefault(); return submit();
    }
    if (event.key === "Escape" && !event.isComposing && expanded && !pending.has("panel")) {
      event.preventDefault(); return act("panel");
    }
  };
  const act = async (action) => {
    if (pending.has(action) || disposed) return;
    const control = button(action); if (control?.disabled) return;
    const runId = projection.activeRunId;
    if (["stop-task", "expand"].includes(action) && !runId) return;
    pending.add(action); localError = ""; paint();
    let focusAfterAction = null;
    try {
      if (action === "panel") {
        const target = !expanded;
        await invoke("capsule_resize", { expanded: target });
        if (!disposed) { expanded = target; focusAfterAction = expanded ? input : control; }
      } else if (action === "cancel") await invoke("voice_stop");
      else if (action === "listen") { await invoke("voice_start_listening"); await refresh(); }
      else if (action === "mute") { await invoke("voice_daemon_set_muted", { muted: !projection.muted }); await refresh(); }
      else if (["stop-task", "expand"].includes(action)) {
        const snapshot = await invoke("capsule_control", { action: action === "stop-task" ? "cancel" : "open", runId });
        if (!disposed) accept(snapshot);
        await refresh();
      }
    } catch (error) { if (!disposed) localError = `操作失败：${error instanceof Error ? error.message : String(error)}`; }
    finally {
      pending.delete(action); paint();
      if (!disposed) focusAfterAction?.focus();
    }
  };
  const onClick = (event) => {
    const action = event.target.closest?.("[data-voice-action]")?.dataset.voiceAction;
    if (action) return act(action);
  };
  const onVisibility = () => {
    if (visibilityTarget?.hidden && frame !== null) { cancelFrame(frame); frame = null; }
    paint();
    if (!visibilityTarget?.hidden) refresh();
  };
  const dispose = () => {
    if (disposed) return;
    disposed = true;
    if (frame !== null) cancelFrame(frame);
    for (const unlisten of unlisteners.splice(0)) { try { unlisten(); } catch {} }
    root.removeEventListener("click", onClick); root.removeEventListener("submit", onSubmit);
    root.removeEventListener("input", onInput); root.removeEventListener("keydown", onKey);
    hostWindow?.removeEventListener("focus", refresh); hostWindow?.removeEventListener("pagehide", dispose);
    visibilityTarget?.removeEventListener("visibilitychange", onVisibility);
    if (mounts.get(root) === controller) mounts.delete(root);
  };
  const controller = { refresh, dispose, getProjection: () => projection, isExpanded: () => expanded };
  mounts.set(root, controller);
  root.addEventListener("click", onClick); root.addEventListener("submit", onSubmit);
  root.addEventListener("input", onInput); root.addEventListener("keydown", onKey);
  hostWindow?.addEventListener("focus", refresh); hostWindow?.addEventListener("pagehide", dispose);
  visibilityTarget?.addEventListener("visibilitychange", onVisibility);
  paint();
  if (listen) {
    try {
      for (const [name, handler] of [
        ["lumo://capsule-state", ({ payload }) => { if (!disposed) accept(payload); }],
        ["lumo://voice-level", ({ payload }) => {
          if (disposed || visibilityTarget?.hidden || !projection.listening) return;
          projection = { ...projection, level: clamp(payload?.level) };
          if (frame === null) frame = requestFrame(paintLevel);
        }],
      ]) {
        if (disposed) break;
        const unlisten = await listen(name, handler);
        if (typeof unlisten === "function") { if (disposed) unlisten(); else unlisteners.push(unlisten); }
      }
    } catch (error) { if (!disposed) { localError = `任务状态连接失败：${error instanceof Error ? error.message : String(error)}`; paint(); } }
  }
  if (!disposed) await refresh();
  return controller;
}

if (typeof document !== "undefined") {
  const root = document.querySelector("[data-voice-capsule]");
  if (root) mountVoiceCapsule(root).catch(() => {});
}
