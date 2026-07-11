import { applyAgentEvent, createRunProjection } from "./agent-events.js";

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

