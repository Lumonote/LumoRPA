const escapeHtml = (value) => String(value ?? "").replaceAll("&", "&amp;").replaceAll("<", "&lt;").replaceAll(">", "&gt;").replaceAll('"', "&quot;").replaceAll("'", "&#39;");

export function createVoiceProjection() {
  return { status: "idle", partial: "", transcript: "", source: "LOCAL", progress: 0, level: 0, message: "随时可以唤醒 Lumo", muted: false };
}

export function applyVoiceEvent(projection, event) {
  if (!event) return projection;
  if (event.kind === "voice.state") return { ...projection, status: event.state, message: event.message || projection.message };
  if (event.kind === "transcript.partial") return { ...projection, status: "transcribing", partial: event.text || "" };
  if (event.kind === "transcript.final") return { ...projection, status: "routing", partial: "", transcript: event.text || "", source: String(event.source || "local").toUpperCase() };
  if (event.kind === "agent.state") return { ...projection, status: event.state || projection.status, progress: event.progress ?? projection.progress, message: event.message || projection.message };
  if (event.kind === "voice.level") return { ...projection, level: Math.max(0, Math.min(1, Number(event.level) || 0)) };
  if (event.kind === "voice.follow-up") return { ...projection, status: "listening", message: event.message || `继续说，我会等待 ${Number(event.timeoutSeconds) || 8} 秒` };
  if (event.kind === "voice.feedback") return { ...projection, status: event.ok === false ? "error" : "success", partial: "", transcript: "", message: event.message || projection.message, progress: event.ok === false ? projection.progress : 1 };
  if (event.kind === "voice.muted") return { ...projection, status: event.muted ? "muted" : "idle", muted: Boolean(event.muted) };
  return projection;
}

const labels = { idle: "READY", listening: "LISTENING", transcribing: "TRANSCRIBING", routing: "ROUTING", confirming: "CONFIRM", executing: "EXECUTING", reporting: "REPORTING", success: "DONE", error: "ERROR", muted: "MUTED" };

export function renderVoiceCapsule(projection) {
  const text = projection.partial || projection.transcript || projection.message;
  const level = Math.max(0, Math.min(1, Number(projection.level) || 0));
  return `<section class="voice-capsule is-${escapeHtml(projection.status)}" aria-live="polite" aria-label="Lumo voice agent ${escapeHtml(projection.status)}"><div class="vc-orb" aria-hidden="true"><div class="vc-aura"></div><div class="vc-spectrum"><i></i><i></i><i></i><i></i></div><div class="vc-core"></div><div class="vc-particles"><i></i><i></i><i></i><i></i><i></i><i></i></div><b class="vc-ring ring-a"></b><b class="vc-ring ring-b"></b></div><div class="vc-copy"><div><span>${escapeHtml(labels[projection.status] || projection.status.toUpperCase())}</span><em>${escapeHtml(projection.source)}</em></div><strong>${escapeHtml(text)}</strong><div class="vc-wave" aria-hidden="true">${Array.from({ length: 12 }, (_, index) => `<i style="--wave:${index};--amplitude:${(level * (3 + (index % 5))).toFixed(2)}px"></i>`).join("")}</div><div class="vc-progress"><i style="width:${Math.max(0, Math.min(1, projection.progress || 0)) * 100}%"></i></div></div><div class="vc-actions"><button data-voice-action="expand" aria-label="展开详情">⌁</button><button data-voice-action="cancel" aria-label="取消语音任务">×</button></div></section>`;
}

export async function mountVoiceCapsule(root, { listen = globalThis.__TAURI__?.event?.listen, call = globalThis.__TAURI__?.core?.invoke } = {}) {
  let projection = createVoiceProjection();
  const render = () => { root.innerHTML = renderVoiceCapsule(projection); };
  root.addEventListener("click", async (event) => {
    const action = event.target.closest("[data-voice-action]")?.dataset.voiceAction;
    if (action === "cancel") await call?.("voice_stop");
    if (action === "expand") await call?.("capsule_expand", { expanded: true });
  });
  if (listen) {
    await listen("lumo://voice-state", ({ payload }) => { projection = applyVoiceEvent(projection, { kind: "voice.state", ...(payload || {}) }); render(); });
    await listen("lumo://transcript", ({ payload }) => { projection = applyVoiceEvent(projection, { kind: payload?.final ? "transcript.final" : "transcript.partial", ...(payload || {}) }); render(); });
    await listen("lumo://voice-level", ({ payload }) => { projection = applyVoiceEvent(projection, { kind: "voice.level", ...(payload || {}) }); render(); });
    await listen("lumo://voice-follow-up", ({ payload }) => { projection = applyVoiceEvent(projection, { kind: "voice.follow-up", ...(payload || {}) }); render(); });
    await listen("lumo://voice-feedback", ({ payload }) => { projection = applyVoiceEvent(projection, { kind: "voice.feedback", ...(payload || {}) }); render(); });
    await listen("lumo://voice-muted", ({ payload }) => { projection = applyVoiceEvent(projection, { kind: "voice.muted", ...(payload || {}) }); render(); });
    await listen("lumo://agent-event", ({ payload }) => { projection = applyVoiceEvent(projection, { kind: "agent.state", state: payload?.capsuleState || payload?.state, progress: payload?.payload?.progress }); render(); });
  }
  render();
}

if (typeof document !== "undefined") {
  const root = document.querySelector("[data-voice-capsule]");
  if (root) mountVoiceCapsule(root).catch(() => {});
}
