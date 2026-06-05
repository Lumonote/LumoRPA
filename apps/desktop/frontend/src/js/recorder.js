// Recorder view: live CDP event stream, elapsed timer, and the captured-YAML
// patch that can be inserted into the active flow or saved as a new flow.

import { $, html, toast } from "./dom.js";
import { call } from "./api.js";
import { state } from "./state.js";
import { emitStep, extractSteps, findStepRange, parseYaml } from "./yaml.js";
import { renderActiveView } from "./editor/render.js";
import { refreshFlows, loadFlow } from "./flows.js";
import { switchTopView } from "./views.js";
import { refreshElementLibrary } from "./elements.js";

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
    // R-02 desktop lane (DesktopRecorder AX backend).
    if (kind === "app_changed") return `🪟 切换应用 → ${payload.app || "?"}${payload.window_title ? "  ·  " + payload.window_title : ""}`;
    if (kind === "focus_changed") return `🪟 切换窗口 → ${payload.window_title || payload.app || "?"}`;
    if (kind === "focus_field") {
      const ctrl = payload.focused_name || payload.focused_role || "?";
      return `🎯 焦点控件 → ${ctrl}${payload.focused_role && payload.focused_name ? "  ·  " + payload.focused_role : ""}`;
    }
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
    if (kind === "focus_field" || kind === "app_changed" || kind === "focus_changed") return "ok";
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
    await refreshElementLibrary().catch(() => {});
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
    switchTopView("design");
    pendingRecorderPatch = "";
    const box = $("recorderPatch");
    if (box) box.innerHTML = `<div class="muted" style="padding:8px 10px">已保存并打开 ${html(path)}。</div>`;
    toast("已保存并打开", path, "ok");
  } catch (e) {
    toast("保存失败", String(e), "bad");
  }
}

async function insertRecorderPatchIntoFlow() {
  if (!pendingRecorderPatch.trim()) {
    toast("没有可插入内容", "先录制一段操作", "bad");
    return;
  }
  if (!state.flowPath) {
    toast("请先打开流程", "录制结果会追加到当前编辑的流程末尾", "bad");
    return;
  }
  const nextSource = mergeRecorderPatchIntoSource(state.source || "", pendingRecorderPatch);
  if (!nextSource) {
    toast("没有可插入步骤", "本次录制没有产生可合并的 browser/desktop 动作", "bad");
    return;
  }
  state.source = nextSource;
  try {
    state.ast = parseYaml(state.source);
  } catch (e) {
    toast("YAML 解析失败", String(e), "bad");
    return;
  }
  const codeEl = $("codeEditor");
  if (codeEl) codeEl.value = state.source;
  renderActiveView();
  try {
    await call("save_flow_source", { path: state.flowPath, source: state.source });
    await refreshFlows();
    await loadFlow(state.flowPath);
    switchTopView("design");
    toast("已合并并保存", `Recorder patch 已追加到 ${state.flowPath}`, "ok");
    pendingRecorderPatch = "";
    const box = $("recorderPatch");
    if (box) box.innerHTML = `<div class="muted" style="padding:8px 10px">已合并并保存到 ${html(state.flowPath)}。</div>`;
  } catch (e) {
    toast("保存失败", `已临时合并到编辑器，但未写入文件：${String(e)}`, "bad");
  }
}

export function mergeRecorderPatchIntoSource(source, patch) {
  const patchSteps = recorderPatchSteps(patch);
  if (!patchSteps.length) return "";
  const sourceLines = source.split(/\r?\n/);
  const ast = parseYaml(source || "spec:\n  steps:\n");
  const steps = extractSteps(ast);
  let baseIndent = 4;
  let insertIdx = sourceLines.length;

  const lastStep = Array.isArray(steps) ? steps[steps.length - 1] : null;
  if (lastStep?.id) {
    const range = findStepRange(sourceLines, lastStep.id);
    if (range) {
      baseIndent = range.baseIndent;
      insertIdx = range.endIdx;
    }
  } else {
    const stepsIdx = findStepsLine(sourceLines);
    if (stepsIdx >= 0) {
      const indent = sourceLines[stepsIdx].match(/^( *)/)?.[1]?.length || 0;
      baseIndent = indent + 2;
      insertIdx = stepsIdx + 1;
    }
  }

  const indent = " ".repeat(baseIndent);
  const inserted = [
    `${indent}# === recorder patch (auto-merged) ===`,
    ...patchSteps.flatMap((step) => emitStep(step, baseIndent).split("\n")),
  ];
  return [
    ...sourceLines.slice(0, insertIdx),
    ...inserted,
    ...sourceLines.slice(insertIdx),
  ].join("\n").replace(/\s*$/, "\n");
}

function recorderPatchSteps(patch) {
  const patchLines = recorderPatchStepLines(patch);
  if (!patchLines.length) return [];
  const normalizedLines = normalizeRecorderPatchStepLines(patchLines);
  const wrapped = [
    "spec:",
    "  steps:",
    ...normalizedLines.map((line) => `    ${line}`),
  ].join("\n");
  try {
    return sanitizeRecorderSteps(extractSteps(parseYaml(wrapped)));
  } catch (e) {
    console.warn("recorder patch parse failed", e);
    return [];
  }
}

function recorderPatchStepLines(patch) {
  const lines = (patch || "").split(/\r?\n/);
  const firstStepIdx = lines.findIndex((line) => /^\s*-\s+/.test(line));
  if (firstStepIdx < 0) return [];
  return lines
    .slice(firstStepIdx)
    .filter((line) => line.trim())
    .map((line) => line.replace(/\s+$/, ""));
}

function normalizeRecorderPatchStepLines(lines) {
  const first = lines.find((line) => /^\s*-\s+/.test(line));
  const baseIndent = first?.match(/^( *)/)?.[1]?.length || 0;
  return lines.map((line) => line.startsWith(" ".repeat(baseIndent)) ? line.slice(baseIndent) : line.trimStart());
}

function sanitizeRecorderSteps(steps) {
  const seen = new Set();
  return (Array.isArray(steps) ? steps : [])
    .map((step, idx) => sanitizeRecorderStep(step, idx, seen))
    .filter(Boolean);
}

function sanitizeRecorderStep(step, idx, seen) {
  if (!isPlainObject(step)) return null;
  const action = cleanString(step.action);
  if (!action) return null;
  const withBlock = sanitizeRecorderWith(action, step.with);
  if (withBlock === null) return null;
  const id = uniqueStepId(cleanString(step.id) || `${action.replace(/\W+/g, "_")}_${idx + 1}`, seen);
  const out = { id, action };
  if (step.when !== undefined && step.when !== null && step.when !== "") out.when = step.when;
  if (cleanString(step.bind)) out.bind = cleanString(step.bind);
  if (Object.keys(withBlock).length) out.with = withBlock;
  const retry = pruneValue(step.retry);
  if (isPlainObject(retry) && Object.keys(retry).length) out.retry = retry;
  const ai = pruneValue(step.ai);
  if (isPlainObject(ai) && Object.keys(ai).length) out.ai = ai;
  return out;
}

function sanitizeRecorderWith(action, rawWith) {
  const withBlock = isPlainObject(rawWith) ? rawWith : {};
  if (action === "browser.open") {
    const url = cleanString(withBlock.url);
    if (!url) return null;
    const out = { url };
    copyBool(out, withBlock, "headless");
    copyString(out, withBlock, "wait_for");
    copyNumber(out, withBlock, "timeout_ms");
    return out;
  }
  if (action === "browser.click") {
    const out = selectorWith(withBlock);
    if (!hasUsableSelector(out)) return null;
    copyString(out, withBlock, "prompt");
    copyString(out, withBlock, "model");
    copyNumber(out, withBlock, "timeout_ms");
    return out;
  }
  if (action === "browser.type") {
    const out = selectorWith(withBlock);
    if (!hasUsableSelector(out)) return null;
    const text = typeof withBlock.text === "string" ? withBlock.text : cleanString(withBlock.text);
    if (!text) return null;
    out.text = text;
    copyBool(out, withBlock, "clear");
    copyString(out, withBlock, "prompt");
    copyString(out, withBlock, "model");
    copyNumber(out, withBlock, "timeout_ms");
    return out;
  }
  if (action === "browser.extract") {
    const selector = cleanString(withBlock.selector);
    if (!selector) return null;
    const out = { selector };
    copyBool(out, withBlock, "all");
    copyString(out, withBlock, "attr");
    copyNumber(out, withBlock, "timeout_ms");
    const map = pruneValue(withBlock.map);
    if (isPlainObject(map) && Object.keys(map).length) out.map = map;
    const frame = pruneValue(withBlock.frame);
    if (isPlainObject(frame) && Object.keys(frame).length) out.frame = frame;
    return out;
  }
  return pruneValue(withBlock) || {};
}

function selectorWith(withBlock) {
  const out = {};
  copyString(out, withBlock, "selector");
  const selectors = sanitizeSelectors(withBlock.selectors);
  if (Object.keys(selectors).length) out.selectors = selectors;
  return out;
}

function sanitizeSelectors(selectors) {
  if (!isPlainObject(selectors)) return {};
  return ["id", "data_testid", "css", "aria_label", "text_includes", "xpath"].reduce((acc, key) => {
    const value = cleanString(selectors[key]);
    if (value) acc[key] = value;
    return acc;
  }, {});
}

function hasUsableSelector(withBlock) {
  return !!cleanString(withBlock.selector)
    || (isPlainObject(withBlock.selectors) && Object.keys(withBlock.selectors).some((key) => cleanString(withBlock.selectors[key])));
}

function pruneValue(value) {
  if (value === undefined || value === null) return undefined;
  if (typeof value === "string") return value.trim() ? value : undefined;
  if (typeof value === "number" || typeof value === "boolean") return value;
  if (Array.isArray(value)) {
    const arr = value.map(pruneValue).filter((item) => item !== undefined);
    return arr.length ? arr : undefined;
  }
  if (isPlainObject(value)) {
    return Object.entries(value).reduce((acc, [key, item]) => {
      const pruned = pruneValue(item);
      if (pruned !== undefined) acc[key] = pruned;
      return acc;
    }, {});
  }
  return undefined;
}

function copyString(out, src, key) {
  const value = cleanString(src?.[key]);
  if (value) out[key] = value;
}

function copyNumber(out, src, key) {
  const value = Number(src?.[key]);
  if (Number.isFinite(value)) out[key] = value;
}

function copyBool(out, src, key) {
  if (typeof src?.[key] === "boolean") out[key] = src[key];
}

function cleanString(value) {
  return typeof value === "string" ? value.trim() : "";
}

function uniqueStepId(base, seen) {
  const clean = base.replace(/[^\w.-]+/g, "_").replace(/^_+|_+$/g, "") || "recorded_step";
  let id = clean;
  let n = 2;
  while (seen.has(id)) {
    id = `${clean}_${n}`;
    n += 1;
  }
  seen.add(id);
  return id;
}

function isPlainObject(value) {
  return value && typeof value === "object" && !Array.isArray(value);
}

function findStepsLine(lines) {
  let inSpec = false;
  for (let i = 0; i < lines.length; i++) {
    const line = lines[i];
    if (/^spec:\s*$/.test(line)) {
      inSpec = true;
      continue;
    }
    if (inSpec && /^\S/.test(line) && !/^spec:\s*$/.test(line)) inSpec = false;
    if (inSpec && /^\s+steps:\s*$/.test(line)) return i;
  }
  return lines.findIndex((line) => /^\s*steps:\s*$/.test(line));
}
