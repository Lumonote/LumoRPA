// Flow / single-step execution, capability-denied recovery, the X-07 time-travel
// timeline and the runs-history detail view.

import { $, $$, html, pretty, toast, setStatus } from "./dom.js";
import { call } from "./api.js";
import { state } from "./state.js";
import { AI_HELPER_LABEL } from "./constants.js";
import {
  parseYaml, ensureCapability, ensureLlmCapability, ensureNetworkCapability,
} from "./yaml.js";
import { renderActiveView } from "./editor/render.js";
import { switchRightSection } from "./views.js";

export async function runSelectedFlow() {
  if (!state.flowPath) { toast("先选择一个流程", "", "warn"); return; }
  setStatus("运行中…", "warn");
  $("runBtn").disabled = true;
  try {
    const response = await call("run_flow", {
      path: state.flowPath,
      inputsJson: $("inputsJson").value || "{}",
      noStore: false,
    });
    onRunComplete(response);
  } catch (error) {
    setStatus("运行失败", "bad");
    handleRunError(error);
  } finally {
    $("runBtn").disabled = false;
  }
}

export async function runStep(stepId) {
  if (!state.flowPath || !stepId) return;
  setStatus(`单步运行 ${stepId} …`, "warn");
  try {
    const response = await call("run_step", {
      path: state.flowPath,
      stepId,
      inputsJson: $("inputsJson").value || "{}",
      noStore: false,
    });
    onRunComplete(response);
    toast("单步完成", `${stepId} · ${response.report.success ? "成功" : "失败"}`, response.report.success ? "ok" : "bad");
  } catch (error) {
    setStatus("单步失败", "bad");
    handleRunError(error);
  }
}

/** Detect `capability denied: <kind> \`<target>\`` and offer one-click whitelist. */
function handleRunError(error) {
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
  node.querySelector("[data-grant]").addEventListener("click", async () => {
    try {
      if (kind === "network") state.source = ensureNetworkCapability(state.source, target);
      else if (kind === "llm") state.source = ensureLlmCapability(state.source);
      else state.source = ensureCapability(state.source, kind.replace(".", ""), target); // fs.read/fs.write
      state.ast = parseYaml(state.source);
      if (state.flowPath) await call("save_flow_source", { path: state.flowPath, source: state.source });
      toast("已加入白名单", `${kind} ${target}`, "ok");
      node.remove();
      renderActiveView();
    } catch (e) {
      toast("写入失败", String(e), "bad");
    }
  });
  node.querySelector("[data-dismiss]").addEventListener("click", () => node.remove());
}

function onRunComplete(response) {
  state.activeRun = response.run;
  state.activeRunSteps = response.steps || [];
  // The run report's `outputs` is the full steps snapshot ({ "<id>": { result,
  // _ai? } }). Stash the per-step `_ai` traces so the timeline can show which
  // steps were rescued/decided by AI (purple "AI heal" badge).
  state.activeRunAi = collectAiTraces(response.report.outputs);
  $("outputBox").textContent = pretty({ runId: response.report.runId, outputs: response.report.outputs });
  $("timelineLabel").textContent = `${response.report.runId.slice(0, 14)}… · ${response.report.durationMs}ms`;
  $("timelineCounts").textContent = `${response.report.stepsOk}/${response.report.stepsTotal} 成功 · ${response.report.stepsFailed} 失败`;
  loadArtifacts(response.report.runId).finally(renderTimeline);
  setStatus(response.report.success ? "运行完成" : "运行失败", response.report.success ? "ok" : "bad");
  switchRightSection("outputs");
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
  state.activeArtifacts = [];
  state.artifactBlobCache = new Map();
  try {
    state.activeArtifacts = (await call("list_artifacts", { runId })) || [];
  } catch (_) {
    state.activeArtifacts = [];
  }
}

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
  try {
    const dto = await call("read_artifact_blob", { artifactId });
    state.artifactBlobCache.set(artifactId, dto.dataUrl);
    return dto.dataUrl;
  } catch (_) {
    state.artifactBlobCache.set(artifactId, null);
    return null;
  }
}

function renderTimeline() {
  const track = $("timelineTrack");
  const steps = state.activeRunSteps;
  if (!steps.length) {
    track.innerHTML = "";
    $("timelineDetail").innerHTML = `<span style="color: var(--muted); font-size: 11px">运行或单步执行后，每个 step 会出现在这里。</span>`;
    return;
  }
  track.innerHTML = steps
    .map((s, idx) => {
      const left = ((idx / Math.max(1, steps.length - 1)) * (100 - 8));
      const aiHealed = s.state === "ai_healed" || !!aiTraceForStep(s);
      const klass = [
        s.state === "failed" ? "is-failed" : s.state === "skipped" ? "is-skipped" : "",
        aiHealed ? "is-ai" : "",
      ].filter(Boolean).join(" ");
      const titleAi = aiHealed ? " · ✨ AI" : "";
      return `<div class="timeline-step ${klass}" data-idx="${idx}" style="left: ${left}%; width: ${Math.min(8, 100 - left)}%" title="${html(s.path)} · ${s.state}${titleAi}"></div>`;
    })
    .join("");
  track.querySelectorAll(".timeline-step").forEach((el) => {
    el.addEventListener("click", () => showStepDetail(Number(el.dataset.idx)));
  });
  showStepDetail(0);
}

// F-19 Variable Watch: render the `vars` (set_var) environment captured as of
// this step, so scrubbing the time-travel timeline shows how variables evolve.
// `varsJson` is `null` for steps recorded before the column existed (older
// runs predate the v3 migration) — those render nothing so the panel is
// unchanged for them.
function renderVarsWatch(varsJson) {
  if (!varsJson || typeof varsJson !== "object" || Array.isArray(varsJson)) return "";
  const entries = Object.entries(varsJson);
  const body = entries.length
    ? entries
        .map(([k, v]) => {
          const scalar = v === null || typeof v !== "object";
          const val = scalar
            ? `<strong>${html(String(v))}</strong>`
            : `<strong style="font-weight: 400; white-space: pre-wrap; font-family: 'SF Mono', Consolas, monospace">${html(pretty(v))}</strong>`;
          return `<div class="kv"><span>${html(k)}</span>${val}</div>`;
        })
        .join("")
    : `<div class="kv"><span style="color: var(--muted)">（暂无变量）</span></div>`;
  return `<div class="section-title" style="margin-top: 10px">变量 Watch (F-19)</div>${body}`;
}

function showStepDetail(idx) {
  const step = state.activeRunSteps[idx];
  if (!step) return;
  state.activeStepRun = step;
  $$(".timeline-step").forEach((el, i) => el.classList.toggle("is-active", i === idx));
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
  $("timelineDetail").innerHTML = `
    <div class="kv"><span>节点</span><strong>${html(step.path)}</strong></div>
    <div class="kv"><span>状态</span><strong style="color: ${step.state === "failed" ? "var(--bad)" : "var(--ok)"}">${html(step.state)}</strong></div>
    <div class="kv"><span>耗时</span><strong>${html(step.durationMs ?? 0)} ms</strong></div>
    ${aiBadge}
    ${step.error ? `<div class="kv"><span>错误</span><strong style="color: var(--bad)">${html(step.error)}</strong></div>` : ""}
    ${art ? `<div class="timeline-screenshot" id="timelineShot"><div class="shot-loading">加载快照…</div></div>` : ""}
    ${step.outputJson ? `<pre>${html(pretty(step.outputJson))}</pre>` : ""}
    ${renderVarsWatch(step.varsJson)}
  `;
  if (art) {
    blobDataUrl(art.id).then((url) => {
      const box = $("timelineShot");
      if (!box) return;
      if (!url) {
        box.innerHTML = `<div class="shot-loading">快照不可用</div>`;
        return;
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
    });
  }
}

export async function refreshRuns() {
  state.runs = await call("list_runs", { limit: 60 });
  const list = $("runsList");
  if (!state.runs.length) {
    list.innerHTML = `<div class="prop-empty">尚无运行记录</div>`;
    return;
  }
  list.innerHTML = state.runs
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
  list.onclick = (e) => {
    const row = e.target.closest("[data-run-id]");
    if (row) showHistoricalRun(row.dataset.runId);
  };
}

async function showHistoricalRun(runId) {
  const detail = await call("show_run", { runId });
  state.activeRun = detail.run;
  state.activeRunSteps = detail.steps;
  // X-10: fetch AI cost rows in parallel; tolerate the storage layer not having
  // any (older runs predate ai_calls). X-07: also pull blob artifacts so the
  // replay scrubber can show per-step screenshots.
  let aiCalls = [];
  try { aiCalls = await call("run_cost", { runId }); } catch (_) {}
  await loadArtifacts(runId);
  $$("#runsList .run-row").forEach((el) => el.classList.toggle("is-active", el.dataset.runId === runId));
  const tokenTotal = (detail.run.costToken || 0).toLocaleString();
  const usd = ((detail.run.costUsdMicro || 0) / 1_000_000).toFixed(4);
  const costRows = aiCalls
    .map(
      (c) => `<tr style="border-top: 1px solid var(--line)">
        <td style="font-family:'SF Mono',Consolas,monospace">${html(c.step_id || "-")}</td>
        <td>${html(c.provider)}</td>
        <td>${html(c.model)}</td>
        <td style="text-align:right">${c.input_tokens}</td>
        <td style="text-align:right">${c.output_tokens}</td>
        <td style="text-align:right">${c.latency_ms}ms</td>
        <td style="text-align:right">$${(c.cost_usd_micro / 1_000_000).toFixed(4)}</td>
      </tr>`
    )
    .join("");
  const costBlock = aiCalls.length
    ? `
    <div class="section-title" style="margin-top: 10px">AI 用量 (X-10)</div>
    <table style="width: 100%; font-size: 11.5px; border-collapse: collapse">
      <thead><tr><th style="text-align:left">step</th><th style="text-align:left">provider</th><th style="text-align:left">model</th><th style="text-align:right">in</th><th style="text-align:right">out</th><th style="text-align:right">latency</th><th style="text-align:right">USD</th></tr></thead>
      <tbody>${costRows}</tbody>
    </table>`
    : "";
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
            (s) => `<tr style="border-top: 1px solid var(--line)">
          <td>${s.seq}</td>
          <td style="font-family: 'SF Mono', Consolas, monospace">${"&nbsp;".repeat(Math.max(0, s.depth) * 2)}${html(s.path)}</td>
          <td style="text-align: center"><span class="status-badge ${s.state === "ok" ? "ready" : s.state === "failed" ? "planned" : "partial"}">${html(s.state)}</span></td>
          <td style="text-align: right">${s.durationMs ?? 0}</td>
        </tr>`
          )
          .join("")}
      </tbody>
    </table>
    ${costBlock}
    <pre style="margin-top: 10px; background: var(--surface-soft); padding: 10px; border-radius: 8px; font-size: 11px; max-height: 220px; overflow: auto">${html(pretty(detail.run.outputs))}</pre>
  `;
}
