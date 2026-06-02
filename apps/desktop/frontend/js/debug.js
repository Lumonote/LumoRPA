// F-20 Breakpoint debugger. Drives the `debug_flow` Tauri command to pause the
// run *before* breakpointed steps (or single-step), surfacing the per-step
// variable watch (F-19) in the Time-Travel timeline, then step / continue.
//
// Model: there is no long-lived paused run process. Each step/continue resumes
// the paused run under the hood (F-13 resume replays the completed steps, then
// "steps off" the pause point and runs to the next one), so the timeline
// accumulates the full trace as you advance. `state.debugRunId` tracks the run
// to resume from; `state.breakpoints` is the set of step ids to pause before.

import { $, html, toast, setStatus } from "./dom.js";
import { call } from "./api.js";
import { state } from "./state.js";
import { applyRunResponse } from "./runs.js";
import { switchRightSection } from "./views.js";

export function isBreakpoint(stepId) {
  return !!stepId && state.breakpoints.has(stepId);
}

// Toggle a breakpoint on `stepId`; returns its new on/off state. The caller
// (inspector) re-renders itself; the debug bar is refreshed here if a session
// is live so its breakpoint list stays in sync.
export function toggleBreakpoint(stepId) {
  if (!stepId) return false;
  if (state.breakpoints.has(stepId)) state.breakpoints.delete(stepId);
  else state.breakpoints.add(stepId);
  if (state.debugRunId) renderDebugBar(state.debugPausedAt);
  return state.breakpoints.has(stepId);
}

function breakpointList() {
  return [...state.breakpoints];
}

async function runDebug({ step, resumeFrom }) {
  if (!state.flowPath) { toast("先选择一个流程", "", "warn"); return; }
  setStatus("调试中…", "warn");
  try {
    const response = await call("debug_flow", {
      path: state.flowPath,
      inputsJson: $("inputsJson").value || "{}",
      breakpoints: breakpointList(),
      step,
      resumeFrom: resumeFrom || null,
    });
    await onDebugResponse(response);
  } catch (error) {
    setStatus("调试失败", "bad");
    toast("调试失败", String(error), "bad");
  }
}

// Start a fresh debug session. With breakpoints set, runs to the first one;
// with none, single-steps from the first step so "Debug" is always useful.
export function startDebug() {
  state.debugRunId = null;
  return runDebug({ step: state.breakpoints.size === 0, resumeFrom: null });
}

export function debugStep() {
  return runDebug({ step: true, resumeFrom: state.debugRunId });
}

export function debugContinue() {
  return runDebug({ step: false, resumeFrom: state.debugRunId });
}

export function stopDebug() {
  state.debugRunId = null;
  state.debugPausedAt = null;
  const bar = $("debugBar");
  if (bar) bar.style.display = "none";
  setStatus("调试已停止", "");
}

async function onDebugResponse(response) {
  await applyRunResponse(response);
  switchRightSection("outputs");
  const pausedAt = response.report.pausedAt || null;
  state.debugPausedAt = pausedAt;
  if (pausedAt) {
    state.debugRunId = response.report.runId;
    setStatus(`已暂停 · ${pausedAt}`, "warn");
  } else {
    state.debugRunId = null;
    setStatus(response.report.success ? "调试完成" : "调试结束（失败）", response.report.success ? "ok" : "bad");
  }
  renderDebugBar(pausedAt);
}

function renderDebugBar(pausedAt) {
  const bar = $("debugBar");
  if (!bar) return;
  bar.style.display = "";
  const bps = breakpointList();
  const bpLabel = bps.length ? bps.map((b) => html(b)).join(", ") : "（无）";
  const statusHtml = pausedAt
    ? `<span class="debug-paused">⏸ 暂停于 <strong>${html(pausedAt)}</strong></span>`
    : `<span class="debug-done">调试会话结束</span>`;
  bar.innerHTML = `
    <div class="debug-bar-row">
      ${statusHtml}
      <span class="debug-bps">断点: ${bpLabel}</span>
      <span class="spacer"></span>
      <button id="dbgStepBtn"${pausedAt ? "" : " disabled"}>⏭ 单步</button>
      <button id="dbgContinueBtn"${pausedAt ? "" : " disabled"}>▶ 继续</button>
      <button id="dbgStopBtn">⏹ 停止</button>
    </div>`;
  $("dbgStepBtn")?.addEventListener("click", debugStep);
  $("dbgContinueBtn")?.addEventListener("click", debugContinue);
  $("dbgStopBtn")?.addEventListener("click", stopDebug);
}
