// Flow / single-step execution, capability-denied recovery, the X-07 time-travel
// timeline and the runs-history detail view.

import { $, $$, html, toast, setStatus } from "./dom.js";
import { call } from "./api.js";
import { state, graph } from "./state.js";
import { previewResult } from "./result-preview.js";
import { getRunInstanceIndex, runInstancePage, runResultIdentity } from "./run-instances.js";
import { buildGraphModel } from "./editor/graph-model.js";
import { AI_HELPER_LABEL } from "./constants.js";
import {
  parseYaml, ensureCapability, ensureLlmCapability, ensureNetworkCapability, extractSteps, pathKey,
} from "./yaml.js";
import { renderActiveView } from "./editor/render.js";
import { snapshotSource } from "./editor/history.js";
import { switchRightSection } from "./views.js";
import { ensureCurrentFlowSaved } from "./flows.js";
import { reduceRunProgress } from "./app-events.js";
import { refreshGraphRunState } from "./editor/graph.js";
import { applyGraphRunResponse, createGraphRunContext, finishGraphRunContext, recordGraphRunProgress, graphRuntimePath } from "./editor/graph-run-state.js";

// Allocate a ULID before IPC so progress and cancellation always identify the
// exact editor request, including the interval before the first step starts.
function newRunId() {
  const alphabet = "0123456789ABCDEFGHJKMNPQRSTVWXYZ";
  let time = Date.now();
  let id = "";
  for (let i = 0; i < 10; i++) {
    id = alphabet[time % 32] + id;
    time = Math.floor(time / 32);
  }
  return id + Array.from(crypto.getRandomValues(new Uint8Array(16)), (n) => alphabet[n & 31]).join("");
}

// Keep execution controls locked until the response and timeline are ready.
export function setRunInFlight(on, { selectedRuntimePath = null } = {}) {
  state.runInFlight = !!on;
  state.inFlightRunId = on ? newRunId() : null;
  state.editorRunContext = on
    ? createGraphRunContext({ flowPath: state.flowPath, source: state.source, runId: state.inFlightRunId, selectedRuntimePath })
    : finishGraphRunContext(state.editorRunContext, state.editorRunContext?.terminal || "cancelled");
  refreshGraphRunState();
  acceptingProgress = !!on;
  preparingRun = !!on;
  cancelRequested = false;
  const btn = $("cancelRunBtn");
  if (btn) btn.hidden = !on;
  $$("[data-result-rerun]").forEach((control) => { control.disabled = !!on; });
  for (const id of ["runBtn", "runStepBtn", "debugBtn"]) {
    const control = $(id);
    if (control) control.disabled = on;
  }
}

export function updateRunProgress(event) {
  if (!acceptingProgress || !state.runInFlight || !event?.runId || event.runId !== state.inFlightRunId) return;
  const nextGraphState = recordGraphRunProgress(state.editorRunContext, event);
  if (nextGraphState !== state.editorRunContext) {
    state.editorRunContext = nextGraphState;
    refreshGraphRunState();
  }
  if (cancelRequested) {
    cancelRequested = false;
    call("cancel_run", { runId: event.runId }).catch(() => {});
  }
  const progress = state.liveRunProgress = reduceRunProgress(state.liveRunProgress, event);
  if (progress.currentStep) setStatus(`运行中 · ${progress.currentStep}`, "warn");
  if (progress.logs.length) {
    const dropped = progress.droppedLogs ? `\n… 已截断 ${progress.droppedLogs} 条较早日志` : "";
    $("outputBox").textContent = `${progress.logs.join("\n")}${dropped}`;
  }
}

// Always cancel the ID allocated for this editor request.
export async function cancelActiveRun() {
  if (!state.runInFlight) return;
  cancelRequested = true;
  if (preparingRun) {
    toast("已请求取消", "保存完成后停止执行", "ok");
    return;
  }
  try {
    await call("cancel_run", { runId: state.inFlightRunId });
    toast("已请求取消", "运行将在当前步中断", "ok");
  } catch (error) {
    toast("取消失败", String(error), "bad");
  }
}

export async function prepareEditorRun() {
  const path = await ensureCurrentFlowSaved();
  if (state.editorRunContext?.runId === state.inFlightRunId) {
    state.editorRunContext = { ...state.editorRunContext, flowPath: path };
  }
  preparingRun = false;
  if (!cancelRequested) return path;
  setStatus("运行已取消", "warn");
  $("outputBox").textContent = "运行已取消";
  state.editorRunContext = finishGraphRunContext(state.editorRunContext, "cancelled");
  refreshGraphRunState();
  return null;
}

export async function runSelectedFlow() {
  if (state.runInFlight) return;
  if (!state.flowPath) { toast("先选择一个流程", "", "warn"); return; }
  const inputsJson = $("inputsJson").value || "{}";
  setStatus("运行中…", "warn");
  state.liveRunProgress = null;
  $("outputBox").textContent = "准备运行…";
  setRunInFlight(true);
  try {
    const path = await prepareEditorRun();
    if (!path) return;
    const response = await call("run_flow", {
      path,
      inputsJson,
      noStore: false,
      runId: state.inFlightRunId,
    });
    await onRunComplete(response);
  } catch (error) {
    await showRunFailure(error);
    setStatus("运行失败", "bad");
    handleRunError(error);
  } finally {
    state.inFlightRunId = null;
    setRunInFlight(false);
  }
}

export async function runStep(stepId, selectedPath = state.selectedStepPath) {
  if (state.runInFlight) return;
  if (!state.flowPath || !stepId) return;
  const inputsJson = $("inputsJson").value || "{}";
  setStatus(`单步运行 ${stepId} …`, "warn");
  state.liveRunProgress = null;
  $("outputBox").textContent = "准备运行单步…";
  const selectedNode = nodeForSelection(selectedPath);
  const stepPath = selectedNode?.step?.id === stepId ? [...selectedNode.path] : null;
  setRunInFlight(true, { selectedRuntimePath: selectedNode?.step?.id === stepId ? selectedNode.runtimePath : null });
  try {
    const path = await prepareEditorRun();
    if (!path) return;
    const response = await call("run_step", {
      path,
      stepId,
      stepPath,
      inputsJson,
      noStore: false,
      runId: state.inFlightRunId,
    });
    await onRunComplete(response);
    toast("单步完成", `${stepId} · ${response.report.success ? "成功" : "失败"}`, response.report.success ? "ok" : "bad");
  } catch (error) {
    await showRunFailure(error);
    setStatus("单步失败", "bad");
    handleRunError(error);
  } finally {
    state.inFlightRunId = null;
    setRunInFlight(false);
  }
}

/** Detect `capability denied: <kind> \`<target>\`` and offer one-click whitelist. */
function handleRunError(error) {
  const failedPath = state.editorRunContext?.flowPath || state.flowPath;
  const msg = String(error);
  const m = msg.match(/capability denied:\s*(network|fs\.read|fs\.write|llm)\s*`([^`]+)`/i);
  if (!m) { toast("运行失败", msg, "bad"); return; }
  const kind = m[1].toLowerCase();
  const target = m[2];
  const stack = $("toastStack");
  const node = document.createElement("div");
  node.className = "toast bad";
  node.innerHTML = `
    <div class="toast-title">能力被拒绝: ${html(kind)} ${html(target)}</div>
    <div class="toast-body">流程的 spec.capabilities 没有声明这一项。点击下方按钮自动添加并重新校验。</div>
    <div class="toast-actions">
      <button class="primary" data-grant="${html(kind)}|${html(target)}">+ 加入白名单并保存</button>
      <button data-dismiss>忽略</button>
    </div>`;
  stack.appendChild(node);
  let timer = window.setTimeout(() => node.remove(), 12000);
  const dismiss = () => {
    window.clearTimeout(timer);
    node.remove();
  };
  node.querySelector("[data-grant]").addEventListener("click", async () => {
    window.clearTimeout(timer);
    try {
      if (state.flowPath !== failedPath) { toast("请先返回失败的流程", failedPath, "warn"); return; }
      snapshotSource("capability");
      if (kind === "network") state.source = ensureNetworkCapability(state.source, target);
      else if (kind === "llm") state.source = ensureLlmCapability(state.source);
      else state.source = ensureCapability(state.source, kind.replace(".", ""), target); // fs.read/fs.write
      state.ast = parseYaml(state.source);
      if (state.flowPath) {
        const savedSource = state.source;
        await call("save_flow_source", { path: failedPath, source: savedSource });
        if (state.flowPath === failedPath) state.savedSource = savedSource;
      }
      toast("已加入白名单", `${kind} ${target}`, "ok");
      dismiss();
      renderActiveView();
    } catch (e) {
      toast("写入失败", String(e), "bad");
      timer = window.setTimeout(() => node.remove(), 12000);
    }
  });
  node.querySelector("[data-dismiss]").addEventListener("click", dismiss);
}

async function onRunComplete(response) {
  await applyRunResponse(response);
  setStatus(response.report.success ? "运行完成" : "运行失败", response.report.success ? "ok" : "bad");
  switchRightSection("outputs");
}

// Failed VMs reject the IPC call after persisting their steps. Recover that
// trace so an error leaves useful output and a timeline instead of "准备运行".
export async function showRunFailure(error) {
  acceptingProgress = false;
  const runId = state.inFlightRunId;
  try {
    const detail = runId && await call("show_run", { runId, summary: true });
    if (detail?.run) {
      const steps = detail.steps || [];
      await applyRunResponse({ ...detail, report: {
        runId, success: false, outputs: detail.run.outputs,
        durationMs: detail.run.durationMs || 0, stepsTotal: steps.length,
        stepsOk: steps.filter((s) => s.state === "ok" || s.state === "ai_healed").length,
        stepsFailed: steps.filter((s) => ["failed", "timeout", "cancelled"].includes(s.state)).length,
      } });
    }
  } catch { /* A parse/save failure may not have created a stored run. */ }
  if (state.editorRunContext?.runId === runId) {
    state.editorRunContext = finishGraphRunContext(state.editorRunContext, state.liveRunProgress?.status === "cancelled" ? "cancelled" : "failed");
    refreshGraphRunState();
  }
  setOutputPreview({ runId, error: String(error), outputs: state.activeRun?.id === runId ? state.activeRun.outputs : null });
  switchRightSection("outputs");
}

/// Populate the Time-Travel timeline + output box from a `RunResponse` (the
/// shared shape returned by `run_flow` / `run_step` / `debug_flow`). Extracted
/// so the F-20 debugger can reuse the exact same timeline + F-19 vars surface
/// for a paused run. Returns the artifact-load promise (resolves after the
/// timeline first renders).
export function applyRunResponse(response) {
  const request = resultRequest = {};
  historyRequest = null;
  activeResultContext = runResultIdentity(state.editorRunContext, response.report.runId);
  timelinePage = 0;
  acceptingProgress = false;
  state.editorRunContext = applyGraphRunResponse(state.editorRunContext, response);
  refreshGraphRunState();
  state.activeRun = response.run;
  state.activeRunSteps = response.steps || [];
  // The run report's `outputs` is the full steps snapshot ({ "<id>": { result,
  // _ai? } }). Stash the per-step `_ai` traces so the timeline can show which
  // steps were rescued/decided by AI (purple "AI heal" badge).
  state.activeRunAi = collectAiTraces(response.report.outputs);
  setOutputPreview({ runId: response.report.runId, ...(response.report.outputsDeferred ? { message: "结果较大，已保存。选择节点按需查看输出，或在历史记录中加载流程输出。" } : { outputs: response.report.outputs }) });
  $("timelineLabel").textContent = `${response.report.runId.slice(0, 14)}… · ${response.report.durationMs}ms`;
  $("timelineCounts").textContent = `${response.report.stepsOk}/${response.report.stepsTotal} 成功 · ${response.report.stepsFailed} 失败`;
  return loadArtifacts(response.report.runId).finally(() => { if (resultRequest === request) renderTimeline(); });
}

// Walk a steps-snapshot value and collect `{ stepId: _ai }` for every step
// whose output carries an `_ai.used` trace. Tolerates `null`/non-object input.
function collectAiTraces(outputs) {
  const traces = {};
  if (!outputs || typeof outputs !== "object") return traces;
  for (const [id, entry] of Object.entries(outputs)) {
    if (entry && typeof entry === "object" && entry._ai && entry._ai.used) {
      traces[id] = entry._ai;
    }
  }
  return traces;
}

// Resolve the `_ai` trace for a timeline step. step_runs paths look like
// `loop/item.3/click`; the snapshot is keyed by bare step id, so match on the
// final path segment as well as state === "ai_healed".
function aiTraceForStep(step) {
  if (!step) return null;
  const traces = state.activeRunAi || {};
  const leaf = String(step.path || step.stepId || "").split("/").pop();
  return traces[step.stepId] || traces[leaf] || null;
}

// X-07 Time-Travel: pull the run's blob artifacts (screenshots / DOM / HAR) so
// the scrubber can show what the screen looked like at each step. Tolerates the
// IPC missing on older builds.
async function loadArtifacts(runId) {
  const request = {};
  artifactRequest = request;
  state.activeArtifacts = [];
  state.artifactBlobCache = new Map();
  try {
    const artifacts = (await call("list_artifacts", { runId })) || [];
    if (artifactRequest === request) state.activeArtifacts = artifacts;
  } catch (_) {
    if (artifactRequest === request) state.activeArtifacts = [];
  }
}

let artifactRequest;
let acceptingProgress = false;
let preparingRun = false;
let cancelRequested = false;

// Find the artifact (preferring screenshots) attributed to a given step path /
// id. step_runs.path and artifacts.step_id are matched loosely so loop-nested
// steps still line up.
function artifactForStep(step) {
  if (!state.activeArtifacts.length || !step) return null;
  const matches = state.activeArtifacts.filter(
    (a) => a.step_id && (a.step_id === step.stepId || a.step_id === step.path)
  );
  const pool = matches.length ? matches : [];
  return pool.find((a) => a.kind === "screenshot") || pool[0] || null;
}

async function blobDataUrl(artifactId) {
  if (state.artifactBlobCache.has(artifactId)) return state.artifactBlobCache.get(artifactId);
  const cache = state.artifactBlobCache;
  try {
    const dto = await call("read_artifact_blob", { artifactId });
    if (cache.size >= 12) cache.delete(cache.keys().next().value);
    cache.set(artifactId, dto.dataUrl);
    return dto.dataUrl;
  } catch (_) {
    cache.set(artifactId, null);
    return null;
  }
}

function renderRunsChart(runs) {
  const chart = $("runsChart");
  if (!chart) return;
  if (!runs.length) {
    chart.innerHTML = `<div class="prop-empty">暂无运行图表</div>`;
    return;
  }
  const total = runs.length;
  const ok = runs.filter((r) => r.state === "ok").length;
  const failed = runs.filter((r) => r.state === "failed").length;
  const other = Math.max(0, total - ok - failed);
  const durations = runs.map((r) => Number(r.durationMs || 0)).filter((n) => n >= 0);
  const avg = durations.length ? Math.round(durations.reduce((a, b) => a + b, 0) / durations.length) : 0;
  const recent = runs.slice(0, 14).reverse();
  const maxDuration = Math.max(1, ...recent.map((r) => Number(r.durationMs || 0)));
  chart.innerHTML = `
    <div class="runs-chart-stats">
      <div><span>最近运行</span><strong>${total}</strong></div>
      <div><span>成功</span><strong>${ok}</strong></div>
      <div><span>失败</span><strong>${failed}</strong></div>
      <div><span>其他</span><strong>${other}</strong></div>
      <div><span>平均耗时</span><strong>${avg}ms</strong></div>
    </div>
    <div class="runs-chart-bars" aria-label="最近运行耗时图">
      ${recent
        .map((r) => {
          const duration = Number(r.durationMs || 0);
          const height = Math.max(8, Math.round((duration / maxDuration) * 58));
          const klass = r.state === "ok" ? "ok" : r.state === "failed" ? "failed" : "other";
          return `<button class="run-bar ${klass}" data-run-id="${html(r.id)}" style="height: ${height}px" title="${html(r.flowId)} · ${html(r.state)} · ${duration}ms"></button>`;
        })
        .join("")}
    </div>
  `;
  chart.querySelectorAll("[data-run-id]").forEach((bar) =>
    bar.addEventListener("click", () => showHistoricalRun(bar.dataset.runId))
  );
}

function renderTimeline() {
  const track = $("timelineTrack");
  const steps = state.activeRunSteps;
  if (!steps.length) {
    track.innerHTML = "";
    $("timelineDetail").innerHTML = `<span style="color: var(--muted); font-size: 11px">运行或单步执行后，每个 step 会出现在这里。</span>`;
    return;
  }
  const start = timelinePage * STEP_PAGE_SIZE;
  track.innerHTML = steps.slice(start, start + STEP_PAGE_SIZE)
    .map((s, localIdx) => {
      const idx = start + localIdx;
      const left = ((localIdx / Math.max(1, Math.min(STEP_PAGE_SIZE, steps.length - start) - 1)) * (100 - 8));
      const aiHealed = s.state === "ai_healed" || !!aiTraceForStep(s);
      const klass = [
        s.state === "failed" ? "is-failed" : s.state === "skipped" ? "is-skipped" : "",
        aiHealed ? "is-ai" : "",
      ].filter(Boolean).join(" ");
      const titleAi = aiHealed ? " · ✨ AI" : "";
      return `<div class="timeline-step ${klass}" data-idx="${idx}" style="left: ${left}%; width: ${Math.min(8, 92 / Math.max(1, Math.min(STEP_PAGE_SIZE, steps.length - start)))}%" title="${html(s.path)} · ${s.state}${titleAi}"></div>`;
    })
    .join("");
  track.querySelectorAll(".timeline-step").forEach((el) => {
    el.addEventListener("click", () => showStepDetail(Number(el.dataset.idx)));
  });
  if (steps.length > STEP_PAGE_SIZE) {
    const pager = document.createElement("div");
    pager.className = "result-pager";
    pager.innerHTML = `<button data-prev ${start === 0 ? "disabled" : ""}>上一页</button> <span>${start + 1}–${Math.min(start + STEP_PAGE_SIZE, steps.length)} / ${steps.length}</span> <button data-next ${start + STEP_PAGE_SIZE >= steps.length ? "disabled" : ""}>下一页</button>`;
    track.appendChild(pager);
    pager.querySelector?.("[data-prev]")?.addEventListener("click", () => { timelinePage--; renderTimeline(); });
    pager.querySelector?.("[data-next]")?.addEventListener("click", () => { timelinePage++; renderTimeline(); });
  }
  showStepDetail(start, { focus: false });
}

// F-19 Variable Watch: render the `vars` (set_var) environment captured as of
// this step, so scrubbing the time-travel timeline shows how variables evolve.
// `varsJson` is `null` for steps recorded before the column existed (older
// runs predate the v3 migration) — those render nothing so the panel is
// unchanged for them.
function renderVarsWatch(varsJson) {
  if (!varsJson || typeof varsJson !== "object" || Array.isArray(varsJson)) return "";
  const entries = [];
  let truncated = false;
  for (const key in varsJson) {
    if (!Object.hasOwn(varsJson, key)) continue;
    if (entries.length >= 40) { truncated = true; break; }
    entries.push([key, varsJson[key]]);
  }
  const body = entries.length
    ? entries
        .map(([k, v]) => {
          const scalar = v === null || typeof v !== "object";
          const val = scalar
            ? `<strong>${html(previewResult(v, { maxString: 1200 }).text)}</strong>`
            : `<strong style="font-weight: 400; white-space: pre-wrap; font-family: 'SF Mono', Consolas, monospace">${html(previewResult(v).text)}</strong>`;
          return `<div class="kv"><span>${html(k)}</span>${val}</div>`;
        })
        .join("")
    : `<div class="kv"><span style="color: var(--muted)">（暂无变量）</span></div>`;
  return `<div class="section-title" style="margin-top: 10px">变量 Watch (F-19)</div>${body}${truncated ? `<div class="hint">仅预览前 40 个变量</div>` : ""}`;
}

async function showStepDetail(idx, { focus = true, drawer = false } = {}) {
  const summary = state.activeRunSteps[idx];
  if (!summary) return;
  const runId = state.activeRun?.id;
  const flowPath = state.flowPath;
  const source = state.source;
  const request = stepRequest = {};
  const step = summary.detailsDeferred ? await resolveStepDetails(summary, runId) : summary;
  if (stepRequest !== request || state.activeRun?.id !== runId || state.flowPath !== flowPath || state.source !== source) return;
  state.activeStepRun = step;
  if (focus) focusResultNode(step);
  $$(".timeline-step").forEach((el) => el.classList.toggle("is-active", Number(el.dataset.idx) === idx));
  const art = artifactForStep(step);
  const aiTrace = aiTraceForStep(step);
  const aiBadge = aiTrace
    ? `<div class="kv"><span>AI</span><strong class="ai-heal-badge">✨ ${html(AI_HELPER_LABEL[aiTrace.helper] || aiTrace.helper || "AI heal")}${
        typeof aiTrace.confidence === "number" ? ` · ${(aiTrace.confidence * 100).toFixed(0)}%` : ""
      }</strong></div>${
        aiTrace.healed_selector
          ? `<div class="kv"><span>新选择器</span><strong>${html(aiTrace.healed_selector)}</strong></div>`
          : ""
      }${
        aiTrace.reasoning
          ? `<div class="kv"><span>AI 理由</span><strong style="font-weight: 400">${html(aiTrace.reasoning)}</strong></div>`
          : ""
      }`
    : "";
  const detailHtml = `
    <div class="kv"><span>运行</span><strong>${html(runId || "—")}</strong></div>
    <div class="hint">${activeResultContext ? activeResultContext.source === state.source && activeResultContext.flowPath === state.flowPath ? "运行源码与当前编辑器一致" : "运行后源码已变更或切换流程" : "此运行的源码未关联当前编辑器"}</div>
    <div class="kv"><span>节点</span><strong>${html(step.path)}</strong></div>
    <div class="kv"><span>状态</span><strong style="color: ${step.state === "failed" ? "var(--bad)" : "var(--ok)"}">${html(step.state)}</strong></div>
    <div class="kv"><span>耗时</span><strong>${html(step.durationMs ?? 0)} ms</strong></div>
    ${aiBadge}
    ${step.error ? `<div class="kv"><span>错误</span><strong style="color: var(--bad)">${html(step.error)}</strong></div>` : ""}
    ${art ? `<div class="timeline-screenshot" data-timeline-shot><div class="shot-loading">加载快照…</div></div>` : ""}
    ${renderStepOutput(step)}
    ${renderInputConfiguration(step)}
    ${renderVarsWatch(step.varsJson)}
  `;
  const boxes = [$("timelineDetail"), $("graphResultDetail")].filter(Boolean);
  for (const box of boxes) {
    box.innerHTML = detailHtml;
    wireResultExpansion(box, step);
    renderInstancePicker(step, idx, box);
  }
  if (drawer) {
    const panel = $("graphResultDrawer");
    if (panel) panel.hidden = false;
    if (typeof document.dispatchEvent === "function" && typeof CustomEvent === "function") document.dispatchEvent(new CustomEvent("lumo:graph-result-shown"));
  }
  if (art) {
    blobDataUrl(art.id).then((url) => {
      if (state.activeStepRun !== step) return;
      for (const parent of boxes) {
        const box = parent.querySelector?.("[data-timeline-shot]");
        if (!box) continue;
        if (!url) {
          box.innerHTML = `<div class="shot-loading">快照不可用</div>`;
          continue;
        }
        if (art.mime && art.mime.startsWith("image/")) {
          box.innerHTML = `<img src="${url}" alt="step ${html(step.path)} screenshot" loading="lazy" />
            <div class="shot-caption">${html(art.kind)} · ${html(step.path)}</div>`;
        } else if (art.mime === "text/html") {
          box.innerHTML = `<iframe src="${url}" sandbox=""></iframe>
            <div class="shot-caption">${html(art.kind)} · ${html(step.path)}</div>`;
        } else {
          box.innerHTML = `<div class="shot-caption">${html(art.kind)} · ${html(art.mime)} (${html(art.size)} bytes)</div>`;
        }
      }
    });
  }
}

export async function refreshRuns({ offset = 0 } = {}) {
  const request = runsRequest = {};
  const rows = await call("list_runs", { limit: RUN_PAGE_SIZE, offset });
  if (runsRequest !== request) return;
  runsOffset = offset;
  state.runs = rows;
  renderRunsChart(state.runs);
  const list = $("runsList");
  if (!state.runs.length) {
    list.innerHTML = `<div class="prop-empty">尚无运行记录</div>`;
  } else list.innerHTML = state.runs
    .map(
      (r) => `<div class="run-row ${r.id === state.activeRun?.id ? "is-active" : ""}" data-run-id="${html(r.id)}">
      <div>
        <div class="title">${html(r.flowId)} · ${html(r.state)}</div>
        <div class="id">${html(r.id)} · ${html(r.durationMs ?? "-")}ms · ${html(r.triggerKind)}</div>
      </div>
      <span class="status-badge ${r.state === "ok" ? "ready" : r.state === "failed" ? "planned" : "partial"}">${html(r.state)}</span>
    </div>`
    )
    .join("");
  list.innerHTML += `<div class="result-pager"><button data-runs-prev ${offset === 0 ? "disabled" : ""}>上一页</button><span>第 ${Math.floor(offset / RUN_PAGE_SIZE) + 1} 页</span><button data-runs-next ${rows.length < RUN_PAGE_SIZE ? "disabled" : ""}>下一页</button></div>`;
  list.onclick = (e) => {
    if (e.target.closest("[data-runs-prev]")) { refreshRuns({ offset: Math.max(0, runsOffset - RUN_PAGE_SIZE) }); return; }
    if (e.target.closest("[data-runs-next]")) { refreshRuns({ offset: runsOffset + RUN_PAGE_SIZE }); return; }
    const row = e.target.closest("[data-run-id]");
    if (row) showHistoricalRun(row.dataset.runId);
  };
}

export async function showHistoricalRun(runId, { afterSeq = null, previous = [] } = {}) {
  const request = historyRequest = {};
  resultRequest = null;
  stepRequest = null;
  const isCurrent = () => historyRequest === request;
  const detail = afterSeq === null ? await call("show_run", { runId, summary: true }) : {
    run: state.activeRun,
    steps: await call("list_run_steps", { runId, afterSeq, limit: STEP_PAGE_SIZE }),
  };
  if (!isCurrent()) return;
  const pageSteps = detail.steps || [];
  activeResultContext = runResultIdentity(state.editorRunContext, runId);
  // X-10: fetch AI cost rows in parallel; tolerate the storage layer not having
  // any (older runs predate ai_calls). X-07: also pull blob artifacts so the
  // replay scrubber can show per-step screenshots.
  let aiCalls = [];
  try { aiCalls = (await call("run_cost", { runId, afterId: null, limit: STEP_PAGE_SIZE })).slice(0, STEP_PAGE_SIZE); } catch (_) {}
  if (!isCurrent()) return;
  await loadArtifacts(runId);
  if (!isCurrent()) return;
  state.activeRun = detail.run;
  state.activeRunSteps = pageSteps;
  $$("#runsList .run-row").forEach((el) => el.classList.toggle("is-active", el.dataset.runId === runId));
  const tokenTotal = (detail.run.costToken || 0).toLocaleString();
  const usd = ((detail.run.costUsdMicro || 0) / 1_000_000).toFixed(4);
  $("runDetail").innerHTML = `
    <div class="kv-list" style="padding: 0">
      <div class="kv"><span>流程</span><strong>${html(detail.run.flowId)} @ ${html(detail.run.flowVersion)}</strong></div>
      <div class="kv"><span>状态</span><strong>${html(detail.run.state)}</strong></div>
      <div class="kv"><span>开始时间</span><strong>${html(detail.run.startedAt || "-")}</strong></div>
      <div class="kv"><span>结束时间</span><strong>${html(detail.run.finishedAt || "-")}</strong></div>
      <div class="kv"><span>耗时</span><strong>${html(detail.run.durationMs ?? "-")} ms</strong></div>
      <div class="kv"><span>AI Token</span><strong>${tokenTotal}</strong></div>
      <div class="kv"><span>AI 费用</span><strong>$${usd}</strong></div>
    </div>
    <div class="section-title" style="margin-top: 10px">节点明细</div>
    <table style="width: 100%; font-size: 11.5px; border-collapse: collapse">
      <thead><tr><th style="text-align: left">#</th><th style="text-align: left">路径</th><th>状态</th><th>毫秒</th></tr></thead>
      <tbody>
        ${detail.steps
          .map(
            (s) => `<tr class="hist-step-row" data-seq="${s.seq}" style="border-top: 1px solid var(--line); cursor: pointer">
          <td>${s.seq}</td>
          <td style="font-family: 'SF Mono', Consolas, monospace">${"&nbsp;".repeat(Math.max(0, s.depth) * 2)}${html(s.path)}</td>
          <td style="text-align: center"><span class="status-badge ${s.state === "ok" ? "ready" : s.state === "failed" ? "planned" : "partial"}">${html(s.state)}</span></td>
          <td style="text-align: right">${s.durationMs ?? 0}</td>
        </tr>`
          )
          .join("")}
      </tbody>
    </table>
    <div class="section-title" style="margin-top: 10px">节点检视 · 点击上表任一节点查看变量与输出</div>
    <div id="histStepDetail" class="kv-list" style="padding: 0"></div>
    <div id="runCostDetail"></div>
    <div class="result-pager"><button data-steps-prev ${previous.length ? "" : "disabled"}>上一页</button><span>节点 ${pageSteps[0]?.seq ?? "—"}–${pageSteps.at(-1)?.seq ?? "—"}</span><button data-steps-next ${pageSteps.length < STEP_PAGE_SIZE ? "disabled" : ""}>下一页</button></div>
    <button data-load-run-output>按需查看流程输出</button><pre data-run-output></pre>
  `;
  let costRequest;
  const renderCosts = (rows, afterId = null, previous = []) => {
    const costPanel = $("runCostDetail");
    if (!costPanel) return;
    costPanel.innerHTML = runCostPageMarkup(rows, previous.length);
    const showCostPage = async (nextAfterId, nextPrevious) => {
      const pending = costRequest = {};
      try {
        const costs = await call("run_cost", { runId, afterId: nextAfterId, limit: STEP_PAGE_SIZE });
        if (isCurrent() && costRequest === pending) renderCosts(costs.slice(0, STEP_PAGE_SIZE), nextAfterId, nextPrevious);
      } catch (error) { if (isCurrent() && costRequest === pending) toast("费用加载失败", String(error), "bad"); }
    };
    costPanel.querySelector?.("[data-cost-prev]")?.addEventListener("click", () => showCostPage(previous.at(-1) ?? null, previous.slice(0, -1)));
    costPanel.querySelector?.("[data-cost-next]")?.addEventListener("click", () => showCostPage(rows.at(-1)?.id, [...previous, afterId]));
  };
  renderCosts(aiCalls);

  // F-19: drive a per-step inspector for the historical run so its persisted
  // `vars_json` snapshots are visible here too — not only for the live/just-ran
  // timeline. Clicking a node row scrubs to that step; the first step is
  // selected by default.
  const histRows = $$("#runDetail .hist-step-row");
  const selectHistStep = async (seq, { focus = true } = {}) => {
    const selection = stepRequest = {};
    const summary = pageSteps.find((s) => Number(s.seq) === Number(seq));
    const step = summary.detailsDeferred ? await resolveStepDetails(summary, runId) : summary;
    if (!isCurrent() || stepRequest !== selection) return;
    histRows.forEach((r) => {
      r.style.background = Number(r.dataset.seq) === Number(seq) ? "var(--surface-soft)" : "";
    });
    state.activeStepRun = step;
    renderHistoricalStepDetail(step);
    if (focus) focusResultNode(step);
  };
  histRows.forEach((tr) => tr.addEventListener("click", () => selectHistStep(tr.dataset.seq)));
  if (pageSteps.length) await selectHistStep(pageSteps[0].seq, { focus: false });
  else renderHistoricalStepDetail(null);
  if (!isCurrent()) return;
  const panel = $("runDetail");
  panel.querySelector?.("[data-steps-prev]")?.addEventListener("click", () => showHistoricalRun(runId, { afterSeq: previous.at(-1) ?? null, previous: previous.slice(0, -1) }));
  panel.querySelector?.("[data-steps-next]")?.addEventListener("click", () => showHistoricalRun(runId, { afterSeq: pageSteps.at(-1)?.seq, previous: [...previous, afterSeq] }));
  panel.querySelector?.("[data-load-run-output]")?.addEventListener("click", async (event) => {
    const button = event.currentTarget;
    button.disabled = true;
    try {
      const outputs = await call("read_run_outputs", { runId });
      if (!isCurrent()) return;
      const box = panel.querySelector("[data-run-output]");
      box.textContent = previewResult(outputs).text;
      button.textContent = "已加载输出预览";
    } catch (error) { if (isCurrent()) { button.disabled = false; toast("结果加载失败", String(error), "bad"); } }
  });
}

function runCostPageMarkup(rows, previousCount) {
  if (!rows.length && !previousCount) return "";
  const costRows = rows.map((c) => `<tr style="border-top: 1px solid var(--line)">
    <td style="font-family:'SF Mono',Consolas,monospace">${html(c.step_id || "-")}</td><td>${html(c.provider)}</td><td>${html(c.model)}</td>
    <td style="text-align:right">${c.input_tokens}</td><td style="text-align:right">${c.output_tokens}</td><td style="text-align:right">${c.latency_ms}ms</td><td style="text-align:right">$${(c.cost_usd_micro / 1_000_000).toFixed(4)}</td></tr>`).join("");
  return `<div class="section-title" style="margin-top: 10px">AI 用量 (X-10)</div>
    <table style="width:100%;font-size:11.5px;border-collapse:collapse"><thead><tr><th>step</th><th>provider</th><th>model</th><th>in</th><th>out</th><th>latency</th><th>USD</th></tr></thead><tbody>${costRows}</tbody></table>
    <div class="result-pager"><button data-cost-prev ${previousCount ? "" : "disabled"}>上一页</button><span>费用第 ${previousCount + 1} 页 · ${rows.length} 条</span><button data-cost-next ${rows.length < STEP_PAGE_SIZE ? "disabled" : ""}>下一页</button></div>`;
}

// F-19: render one historical step's variable snapshot + output into the runs
// detail pane. Reuses `renderVarsWatch` (the same surface the live time-travel
// timeline uses), so scrubbing a past run shows how variables evolved — closing
// the gap where vars were only visible for the run you just executed.
function renderHistoricalStepDetail(step) {
  const box = $("histStepDetail");
  if (!box) return;
  if (!step) {
    box.innerHTML = `<div class="kv"><span style="color: var(--muted)">点击上方节点查看变量与输出</span></div>`;
    return;
  }
  box.innerHTML = `
    <div class="kv"><span>节点</span><strong>${html(step.path)}</strong></div>
    <div class="kv"><span>状态</span><strong style="color: ${step.state === "failed" ? "var(--bad)" : "var(--ok)"}">${html(step.state)}</strong></div>
    <div class="kv"><span>耗时</span><strong>${html(step.durationMs ?? 0)} ms</strong></div>
    ${step.error ? `<div class="kv"><span>错误</span><strong style="color: var(--bad)">${html(step.error)}</strong></div>` : ""}
    ${renderStepOutput(step)}
    ${renderVarsWatch(step.varsJson)}
  `;
  wireResultExpansion(box, step);
}

const STEP_PAGE_SIZE = 200;
const RUN_PAGE_SIZE = 60;
let timelinePage = 0;
let runsOffset = 0;
let historyRequest;
let resultRequest;
let stepRequest;
let runsRequest;
let activeResultContext;
let selectionModel;
let focusingResult = false;

function nodeForSelection(selection) {
  if (!selection) return null;
  if (selection.runtimePath) return selection;
  if (!Array.isArray(selection)) return null;
  if (selectionModel?.source !== state.source || selectionModel?.ast !== state.ast) {
    selectionModel = { source: state.source, ast: state.ast, nodes: buildGraphModel(extractSteps(state.ast)).nodes };
  }
  return selectionModel.nodes.get(pathKey(selection)) || graph.allNodePositions?.get(pathKey(selection)) || null;
}

function setOutputPreview(value) {
  const preview = previewResult(value);
  $("outputBox").textContent = preview.text + (preview.truncated ? "\n… 结果较大，请选择节点查看对应输出" : "");
}

function renderStepOutput(step) {
  const hasOutput = step.hasOutput ?? (Object.hasOwn(step, "outputJson") && step.outputJson !== undefined);
  if (!hasOutput) return `<div class="hint">无结果记录</div>`;
  const preview = previewResult(step.outputJson);
  return `<div class="section-title">输出 · ${step.outputJson === null ? "null" : Array.isArray(step.outputJson) ? "array" : typeof step.outputJson}</div>${renderTablePreview(step.outputJson)}<pre data-result-preview>${html(preview.text)}</pre>${preview.truncated ? `<button data-result-expand>展开更多结果</button><button data-result-export>导出完整 JSON</button><span class="hint">预览已截断</span>` : ""}`;
}

function wireResultExpansion(box, step) {
  box?.querySelector?.("[data-result-expand]")?.addEventListener("click", (event) => {
    const preview = previewResult(step.outputJson, { maxItems: 2000, maxDepth: 12, maxString: 20000, maxChars: 200000 });
    box.querySelector("[data-result-preview]").textContent = preview.text;
    event.currentTarget.textContent = preview.truncated ? "已达到预览上限，请导出结果查看" : "已展开完整结果";
    event.currentTarget.disabled = true;
  });
  box?.querySelector?.("[data-result-export]")?.addEventListener("click", () => {
    const url = URL.createObjectURL(new Blob([JSON.stringify(step.outputJson, null, 2)], { type: "application/json" }));
    const link = document.createElement("a");
    link.href = url; link.download = `result-${step.seq ?? "node"}.json`; link.click();
    window.setTimeout(() => URL.revokeObjectURL(url), 1000);
  });
  box?.querySelector?.("[data-result-rerun]")?.addEventListener("click", () => {
    const node = nodeForRunStep(step);
    if (node) runStep(node.step.id, node.path);
  });
}

async function resolveStepDetails(summary, runId) {
  if (!summary?.detailsDeferred) return summary;
  try {
    const detail = await call("read_run_step", { runId, seq: summary.seq });
    return { ...summary, ...detail, detailsDeferred: false };
  } catch (error) {
    return { ...summary, error: [summary.error, `结果加载失败：${error}`].filter(Boolean).join("\n") };
  }
}

function focusResultNode(step) {
  if (!activeResultContext || activeResultContext.runId !== state.activeRun?.id || activeResultContext.flowPath !== state.flowPath || activeResultContext.source !== state.source) return;
  if (typeof document.dispatchEvent === "function" && typeof CustomEvent === "function") {
    // Graph focus selects the corresponding editor node synchronously. That
    // selection must not choose the latest loop instance over this result.
    focusingResult = true;
    try { document.dispatchEvent(new CustomEvent("lumo:focus-node", { detail: { path: graphRuntimePath(activeResultContext, step.path, state.ast) } })); }
    finally { focusingResult = false; }
  }
}

function nodeForRunStep(step) {
  if (!activeResultContext || activeResultContext.source !== state.source || activeResultContext.flowPath !== state.flowPath) return null;
  const path = graphRuntimePath(activeResultContext, step.path, state.ast);
  if (selectionModel?.source !== state.source || selectionModel?.ast !== state.ast) nodeForSelection([0]);
  return [...(selectionModel?.nodes.values() || [])].find((candidate) => candidate.runtimePath === path) || null;
}

function renderInputConfiguration(step) {
  const node = nodeForRunStep(step);
  if (!node) return "";
  return `${node.step?.with ? `<details><summary>输入配置 · 运行源码中的参数</summary><pre>${html(previewResult(node.step.with).text)}</pre></details>` : ""}<button data-result-rerun ${state.runInFlight ? "disabled" : ""}>重新运行此节点</button>`;
}

function renderTablePreview(value) {
  if (!Array.isArray(value) || !value.length || !value[0] || typeof value[0] !== "object" || Array.isArray(value[0])) return "";
  const columns = [];
  for (const key in value[0]) { if (Object.hasOwn(value[0], key)) columns.push(key); if (columns.length >= 8) break; }
  const rows = [];
  for (let index = 0; index < Math.min(20, value.length); index++) {
    rows.push(`<tr>${columns.map((key) => `<td>${html(previewResult(value[index]?.[key], { maxItems: 3, maxDepth: 1, maxString: 120, maxChars: 220 }).text)}</td>`).join("")}</tr>`);
  }
  return `<details open><summary>表格预览 · 前 ${rows.length} / ${value.length} 行 · 最多 8 列</summary><div style="overflow:auto"><table data-result-table><thead><tr>${columns.map((key) => `<th>${html(key)}</th>`).join("")}</tr></thead><tbody>${rows.join("")}</tbody></table></div></details>`;
}

function renderInstancePicker(step, index, box) {
  if (!activeResultContext) return;
  const path = graphRuntimePath(activeResultContext, step.path, state.ast);
  const instances = getRunInstanceIndex(state.activeRunSteps, activeResultContext, state.ast).get(path) || [];
  if (instances.length < 2) return;
  const { start, indices: page, previousIndex, nextIndex } = runInstancePage(instances, index, STEP_PAGE_SIZE);
  const picker = document.createElement("label");
  picker.innerHTML = `运行实例 ${start + 1}–${start + page.length} / ${instances.length}<select data-run-instance>${page.map((idx) => { const item = state.activeRunSteps[idx]; return `<option value="${idx}" ${idx === index ? "selected" : ""}>${html(item.path)} · #${item.seq ?? idx} · ${html(item.state)}</option>`; }).join("")}</select>${previousIndex !== undefined ? `<button data-instance-prev>前一组</button>` : ""}${nextIndex !== undefined ? `<button data-instance-next>后一组</button>` : ""}`;
  box.prepend?.(picker);
  picker.querySelector?.("select")?.addEventListener("change", (event) => showStepDetail(Number(event.target.value), { drawer: true }));
  picker.querySelector?.("[data-instance-prev]")?.addEventListener("click", () => showStepDetail(previousIndex, { drawer: true }));
  picker.querySelector?.("[data-instance-next]")?.addEventListener("click", () => showStepDetail(nextIndex, { drawer: true }));
}

// Public boundary for graph/steps selection. Only the exact editor snapshot
// may reveal a run result: browsing an unrelated historical run never rebinds
// node identity using a bare ID.
export function selectNodeRunResult(selection) {
  stepRequest = null;
  if (!selection || !activeResultContext || activeResultContext.runId !== state.activeRun?.id || activeResultContext.flowPath !== state.flowPath || activeResultContext.source !== state.source) return false;
  const node = nodeForSelection(selection);
  const path = typeof selection === "string" ? selection : node?.runtimePath;
  if (!path) return false;
  const idx = getRunInstanceIndex(state.activeRunSteps, activeResultContext, state.ast).get(path)?.at(-1) ?? -1;
  if (idx < 0) return false;
  timelinePage = Math.floor(idx / STEP_PAGE_SIZE);
  renderTimeline();
  showStepDetail(idx, { focus: false, drawer: true });
  return true;
}

if (typeof document !== "undefined" && typeof document.addEventListener === "function") {
  document.addEventListener("lumo:graph-result-closed", () => { stepRequest = null; });
  document.addEventListener("lumo:step-selected", (event) => {
    if (focusingResult) return;
    if (event.detail?.flowPath && event.detail.flowPath !== state.flowPath) return;
    stepRequest = null;
    if (!event.detail?.path && !event.detail?.runtimePath) {
      stepRequest = null;
      const panel = $("graphResultDrawer");
      if (panel) panel.hidden = true;
      const detail = $("graphResultDetail");
      if (detail) detail.innerHTML = "";
      return;
    }
    if (!selectNodeRunResult(event.detail?.runtimePath || event.detail?.path)) {
      state.activeStepRun = null;
      const detail = $("graphResultDetail");
      if (detail) detail.innerHTML = `<div class="prop-empty">此节点在当前源码版本没有运行结果</div>`;
    }
  });
}
