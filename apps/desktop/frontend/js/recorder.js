// Recorder view: live CDP event stream, elapsed timer, and the captured-YAML
// patch that can be inserted into the active flow or saved as a new flow.

import { $, html, toast } from "./dom.js";
import { call } from "./api.js";
import { state } from "./state.js";
import { parseYaml } from "./yaml.js";
import { renderActiveView } from "./editor/render.js";
import { refreshFlows, loadFlow } from "./flows.js";

let recorderTick = null;
let recorderStartedAt = 0;
let recorderEventCount = 0;
let recorderEventUnlisten = null;

export async function refreshRecorder() {
  const status = await call("recorder_status");
  applyRecorderStatus(status);
}

function applyRecorderStatus(status) {
  state.recorder = status;
  $("recorderPill").classList.toggle("is-on", !!status.recording);
  $("recorderPillText").textContent = status.recording
    ? `录制中 · ${status.target || "browser"}`
    : "未录制";
  $("recorderStartBtn").disabled = !!status.recording;
  $("recorderStopBtn").disabled = !status.recording;
  $("recorderNote").innerHTML = html(status.note || "");
  const backendEl = $("recorderBackend");
  if (backendEl) backendEl.textContent = status.backend || "—";
  const elapsedEl = $("recorderStatElapsed");
  const eventsEl = $("recorderStatEvents");
  if (elapsedEl && eventsEl) {
    elapsedEl.classList.toggle("is-pulsing", !!status.recording);
    eventsEl.classList.toggle("is-pulsing", !!status.recording);
  }
  if (status.recording) {
    if (!recorderTick) {
      recorderStartedAt = status.started_at ? Date.parse(status.started_at) : Date.now();
      recorderEventCount = 0;
      $("recorderEvents").textContent = "0";
      recorderStreamReset(status.target || "browser", status.backend || "");
      recorderTick = setInterval(updateRecorderElapsed, 1000);
    }
  } else if (recorderTick) {
    clearInterval(recorderTick);
    recorderTick = null;
    recorderStreamAppend("muted", "[idle] 录制已停止");
  }
  // Always refresh elapsed display so static "00:00" is shown when idle.
  updateRecorderElapsed();
}

function updateRecorderElapsed() {
  const el = $("recorderElapsed");
  if (!el) return;
  if (!state.recorder?.recording) { el.textContent = "00:00"; return; }
  const sec = Math.max(0, Math.floor((Date.now() - recorderStartedAt) / 1000));
  const mm = String(Math.floor(sec / 60)).padStart(2, "0");
  const ss = String(sec % 60).padStart(2, "0");
  el.textContent = `${mm}:${ss}`;
}

function pad2(n) { return String(n).padStart(2, "0"); }

function recorderStreamReset(target, backend) {
  const box = $("recorderStream");
  if (!box) return;
  box.innerHTML = `<div class="head">▶ live event stream · target=${html(target)} · backend=${html(backend || "—")}</div>`;
  recorderStreamAppend("muted", `[init] session 已启动 · 等待事件…`);
  recorderStreamAppend("muted", `[note] CDP Runtime.addBinding 已注入 · click/input/change/keydown 实时透传 · ActionBuffer 在 stop 时合并 · Alt+点击 抓取同款`);
}

function recorderStreamAppend(cls, text) {
  const box = $("recorderStream");
  if (!box) return;
  const ts = new Date().toLocaleTimeString("zh-CN", { hour12: false });
  const line = document.createElement("div");
  line.className = `line ${cls}`;
  line.innerHTML = `<span class="ts">${html(ts)}</span>${html(text)}`;
  box.appendChild(line);
  box.scrollTop = box.scrollHeight;
}

function summarizeSelector(s) {
  if (!s) return "?";
  return s.length > 60 ? s.slice(0, 57) + "…" : s;
}

function onLiveRecorderEvent(evt) {
  // evt = { source, kind, atMs|at_ms, payload }
  const kind = evt?.kind || "event";
  const source = evt?.source || "?";
  const payload = evt?.payload || {};
  const summary = (() => {
    if (kind === "navigate" && payload.url) return `→ ${payload.url}`;
    if (kind === "launched") return payload.msg || "browser launched";
    if (kind === "heartbeat") return `tick #${payload.n ?? "?"}`;
    if (kind === "binding_ready") return `binding ready @ ${payload.url || ""}`;
    if (kind === "click") return `🖱  ${summarizeSelector(payload.selector)}${payload.label ? "  ·  " + payload.label : ""}`;
    if (kind === "input") return `⌨️  ${summarizeSelector(payload.selector)} = ${JSON.stringify(payload.value ?? "")}`;
    if (kind === "change") return `🔁  ${summarizeSelector(payload.selector)} = ${JSON.stringify(payload.value ?? "")}`;
    if (kind === "keydown") return `⌨️  key=${payload.key} on ${summarizeSelector(payload.selector)}`;
    if (kind === "similar_grab") {
      const cnt = payload.sibling_count ?? "?";
      const sample = Array.isArray(payload.sample_values) ? payload.sample_values.slice(0, 3).join(" / ") : "";
      return `📋 alt-click → ${cnt} 个同款 · ${summarizeSelector(payload.generalized_selector)}${sample ? "  ·  " + sample : ""}`;
    }
    if (kind === "bind_error") return `bind error: ${payload.error || ""}`;
    try { return JSON.stringify(payload); } catch { return String(payload); }
  })();
  const cls = (() => {
    if (kind === "heartbeat") return "warn";
    if (kind === "similar_grab") return "ok";
    if (kind === "click" || kind === "input" || kind === "change") return "ok";
    if (kind === "bind_error") return "bad";
    return "tick";
  })();
  if (kind !== "heartbeat" && kind !== "binding_ready") {
    recorderEventCount += 1;
    const el = $("recorderEvents"); if (el) el.textContent = String(recorderEventCount);
  }
  const sec = Math.max(0, Math.floor((Date.now() - (recorderStartedAt || Date.now())) / 1000));
  recorderStreamAppend(cls, `[${pad2(sec)}s] ${source}.${kind.padEnd(12)} · ${summary}`);
}

export async function ensureRecorderListener() {
  if (recorderEventUnlisten) return;
  try {
    const listenFn = window.__TAURI__?.event?.listen;
    if (!listenFn) return;
    recorderEventUnlisten = await listenFn("lumo://recorder-event", (e) => {
      onLiveRecorderEvent(e?.payload);
    });
  } catch (err) {
    console.warn("recorder listen failed", err);
  }
}

export async function startRecording() {
  try {
    await ensureRecorderListener();
    const status = await call("recorder_start", { target: $("recorderTarget").value });
    applyRecorderStatus(status);
    toast("已开始录制", `${state.recorder.target || "browser"} · ${state.recorder.backend || ""}`, "ok");
  } catch (e) {
    toast("启动失败", String(e), "bad");
  }
}

export async function stopRecording() {
  try {
    const result = await call("recorder_stop");
    recorderStreamAppend("tick", `[done] ${result.events} 事件 · ${result.note}`);
    renderRecorderPatch(result.yamlHint || "");
    toast("录制结束", `${result.events} 事件 · 看下方 YAML 草稿`, "ok");
    await refreshRecorder();
  } catch (e) {
    toast("停止失败", String(e), "bad");
  }
}

// Captured YAML patch waiting to be merged into the active flow. Lives as
// long as the recorder view is open; cleared when the user merges or
// dismisses it.
let pendingRecorderPatch = "";

function renderRecorderPatch(yaml) {
  pendingRecorderPatch = yaml;
  const box = $("recorderPatch");
  if (!box) return;
  const stripped = (yaml || "").trim();
  const hasSteps = stripped && !stripped.includes("no actionable events were captured");
  if (!hasSteps) {
    box.innerHTML = `<div class="muted" style="padding:8px 10px">本次录制没有产生可合并的步骤。继续操作浏览器，或检查 Chromium 是否已正确启动。</div>`;
    return;
  }
  box.innerHTML = `
    <div class="recorder-patch-head">
      <span>▶ Recorder YAML patch · 可粘贴到 spec.steps</span>
      <div style="display:flex;gap:6px">
        <button id="recorderPatchCopyBtn" title="复制到剪贴板">📋 复制</button>
        <button id="recorderPatchSaveBtn" title="另存为一个新流程文件，进入「录制产物」">💾 另存为新流程</button>
        <button class="primary" id="recorderPatchInsertBtn" title="追加到当前打开的流程末尾">⤓ 插入到当前流程</button>
      </div>
    </div>
    <pre class="recorder-patch-body"><code>${html(yaml)}</code></pre>
  `;
  const copy = $("recorderPatchCopyBtn");
  if (copy) copy.onclick = () => {
    navigator.clipboard.writeText(pendingRecorderPatch).then(
      () => toast("已复制", "Recorder YAML patch", "ok"),
      (e) => toast("复制失败", String(e), "bad"),
    );
  };
  const insert = $("recorderPatchInsertBtn");
  if (insert) insert.onclick = () => insertRecorderPatchIntoFlow();
  const save = $("recorderPatchSaveBtn");
  if (save) save.onclick = () => saveRecorderPatchAsFlow();
}

async function saveRecorderPatchAsFlow() {
  if (!pendingRecorderPatch.trim()) {
    toast("没有可保存内容", "先录制一段操作", "bad");
    return;
  }
  const proposed = `rec-${new Date().toISOString().slice(0, 19).replace(/[T:]/g, "-")}`;
  const name = prompt("保存为流程名（不带扩展名）：", proposed);
  if (!name) return;
  try {
    const path = await call("save_recording_as_flow", {
      name,
      yamlHint: pendingRecorderPatch,
    });
    await refreshFlows();
    await loadFlow(path);
    pendingRecorderPatch = "";
    const box = $("recorderPatch");
    if (box) box.innerHTML = `<div class="muted" style="padding:8px 10px">已保存到 ${html(path)}。可继续录制或返回设计页编辑。</div>`;
    toast("已保存到流程库", path, "ok");
  } catch (e) {
    toast("保存失败", String(e), "bad");
  }
}

function insertRecorderPatchIntoFlow() {
  if (!pendingRecorderPatch.trim()) {
    toast("没有可插入内容", "先录制一段操作", "bad");
    return;
  }
  if (!state.flowPath) {
    toast("请先打开流程", "录制结果会追加到当前编辑的流程末尾", "bad");
    return;
  }
  // Strip the YAML header comments (everything before the first list dash) so
  // the patch slots cleanly under `spec.steps`. Indent every line two spaces
  // to match the existing spec.steps indentation.
  const lines = pendingRecorderPatch.split("\n");
  const firstStepIdx = lines.findIndex((l) => /^\s*-\s+/.test(l));
  const stepsBlock = firstStepIdx >= 0 ? lines.slice(firstStepIdx) : lines;
  const indented = stepsBlock
    .filter((l) => l.length > 0)
    .map((l) => "  " + l)
    .join("\n");
  const banner = "\n  # === recorder patch (review before keeping) ===\n";
  const newSource = (state.source || "").replace(/\n*$/, "") + banner + indented + "\n";
  state.source = newSource;
  try {
    state.ast = parseYaml(state.source);
  } catch (e) {
    toast("YAML 解析失败", String(e), "bad");
    return;
  }
  // Mirror the source back into the open code editor if any.
  const codeEl = $("codeArea");
  if (codeEl) codeEl.value = newSource;
  renderActiveView();
  toast("已合并", `Recorder patch 已追加到 ${state.flowPath}`, "ok");
  pendingRecorderPatch = "";
  const box = $("recorderPatch");
  if (box) box.innerHTML = `<div class="muted" style="padding:8px 10px">已合并到 ${html(state.flowPath)}。可继续录制以追加更多步骤；记得 💾 保存。</div>`;
}
