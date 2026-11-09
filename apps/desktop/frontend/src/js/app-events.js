import { applyAgentEvent, createRunProjection } from "./agent-events.js";

const LOG_LIMIT = 1000;

export function normalizeToastEvent(payload = {}) {
  const rawKind = String(payload.kind || "ok").toLowerCase();
  const kind = rawKind === "warning" ? "warn" : rawKind === "error" ? "bad" : rawKind;
  return {
    title: String(payload.title || "提示"),
    body: String(payload.message || payload.body || ""),
    kind: ["ok", "warn", "bad"].includes(kind) ? kind : "ok",
  };
}

export function reduceRunProgress(previous, event = {}) {
  const sameRun = previous?.runId === event.runId;
  const next = sameRun
    ? { ...previous, logs: [...(previous.logs || [])] }
    : { runId: event.runId || null, status: "running", currentStep: null, lastStep: null, logs: [], droppedLogs: 0 };

  if (event.type === "step_started") {
    next.status = "running";
    next.currentStep = event.path || event.stepId || null;
  } else if (event.type === "step_finished") {
    next.lastStep = event;
    if (next.currentStep === (event.path || event.stepId)) next.currentStep = null;
    if (["failed", "cancelled", "timeout"].includes(event.state)) next.status = event.state;
  } else if (event.type === "log") {
    next.logs.push(String(event.line || ""));
    if (next.logs.length > LOG_LIMIT) {
      next.logs.splice(0, next.logs.length - LOG_LIMIT);
      next.droppedLogs += 1;
    }
  }
  next.refreshRequested = false;
  return next;
}

export async function restoreAndSubscribeAgentEvents({ runId, call, listen, onProjection, schedule = (fn) => requestAnimationFrame(fn) }) {
  const followLatest = runId === "latest";
  let projection = createRunProjection(runId);
  let renderQueued = false;
  let disposed = false;
  let ready = false;
  let pending = Promise.resolve();
  const buffered = [];
  const seenRuns = new Set();
  const eventRunId = (event) => event?.runId || event?.run_id;
  const publish = () => {
    if (renderQueued || disposed) return;
    renderQueued = true;
    schedule(() => {
      renderQueued = false;
      if (!disposed) onProjection(projection);
    });
  };
  const selectRun = (id) => {
    if (!id || id === projection.runId) return;
    if (projection.runId !== "latest") seenRuns.add(projection.runId);
    projection = createRunProjection(id);
  };
  const loadAfter = async (id, afterSeq) => {
    const events = await call("agent_events", { runId: id, afterSeq });
    if (disposed) return;
    for (const event of events || []) {
      const actualId = eventRunId(event);
      if (followLatest && projection.runId === "latest" && actualId) selectRun(actualId);
      if (actualId && actualId !== projection.runId) continue;
      projection = applyAgentEvent(projection, event);
    }
    publish();
  };
  const receive = async (payload) => {
    if (!payload || disposed) return;
    const actualId = eventRunId(payload);
    if (actualId && actualId !== projection.runId) {
      if (!followLatest || seenRuns.has(actualId)) return;
      // A buffered event may belong to a run older than the restored snapshot.
      // Resolve the alias again before switching so it cannot rewind the view.
      const latest = await call("agent_events", { runId: "latest", afterSeq: 0 });
      const latestId = eventRunId(latest?.[0]);
      if (latestId && latestId !== actualId) return;
      selectRun(actualId);
      for (const event of latest || []) projection = applyAgentEvent(projection, event);
    } else if (payload.seq > projection.seq + 1) {
      await loadAfter(projection.runId, projection.seq);
    }
    if (disposed) return;
    projection = applyAgentEvent(projection, payload);
    publish();
  };
  const enqueue = (payload) => {
    // IPC handlers can overlap while filling a sequence gap. Preserve arrival
    // order so two runs or out-of-order replies cannot overwrite each other.
    const next = pending.then(() => receive(payload));
    pending = next.catch(() => {});
    return next;
  };
  // Listen first and buffer while restoring, closing the snapshot/subscribe
  // gap where a fast run could otherwise finish without ever being displayed.
  const stopListening = await listen("lumo://agent-event", ({ payload }) => {
    if (!ready) { buffered.push(payload); return; }
    return enqueue(payload);
  });
  try {
    await loadAfter(runId, 0);
    ready = true;
    for (const payload of buffered) await enqueue(payload);
  } catch (error) {
    disposed = true;
    stopListening();
    throw error;
  }
  return { getProjection: () => projection, unlisten: () => { disposed = true; stopListening(); } };
}
