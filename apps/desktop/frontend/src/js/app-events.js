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
  let projection = createRunProjection(runId);
  let renderQueued = false;
  const publish = () => {
    if (renderQueued) return;
    renderQueued = true;
    schedule(() => {
      renderQueued = false;
      onProjection(projection);
    });
  };
  const loadAfter = async (afterSeq) => {
    const events = await call("agent_events", { runId, afterSeq });
    for (const event of events || []) projection = applyAgentEvent(projection, event);
    publish();
  };

  await loadAfter(0);
  const unlisten = await listen("lumo://agent-event", async ({ payload }) => {
    const eventRunId = payload?.runId || payload?.run_id;
    if (!payload || (eventRunId && eventRunId !== runId)) return;
    if (payload.seq > projection.seq + 1) await loadAfter(projection.seq);
    projection = applyAgentEvent(projection, payload);
    publish();
  });
  return { getProjection: () => projection, unlisten };
}
