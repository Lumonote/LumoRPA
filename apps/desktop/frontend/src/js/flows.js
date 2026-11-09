// Flow library list + load / save / create / duplicate.

import { $, html, toast, setStatus, pretty, reportError } from "./dom.js";
import { call } from "./api.js";
import { state } from "./state.js";
import { BLANK_FLOW_TEMPLATE } from "./constants.js";
import { parseYaml } from "./yaml.js";
import { renderActiveView } from "./editor/render.js";
import { renderInspector } from "./editor/inspector.js";
import { resetHistory } from "./editor/history.js";
import { chooseEditorAction, draftRecoveryState } from "./editor-drafts.js";

export async function refreshFlows() {
  state.examples = await call("list_flow_library");
  renderFlowList();
  renderFlowConfigCard();
}

export async function createNewFlow() {
  const stamp = new Date().toISOString().replace(/[-:]/g, "").replace(/\.\d+Z$/, "");
  const name = `flow-${stamp}`;
  const yaml = BLANK_FLOW_TEMPLATE.replace("id: NAME", `id: ${name.replace(/[^a-zA-Z0-9_-]/g, "-")}`);
  try {
    const path = await call("save_flow_as", { name, source: yaml });
    await refreshFlows();
    await loadFlow(path);
    toast("已创建", path, "ok");
  } catch (e) {
    toast("新建失败", String(e), "bad");
  }
}

export async function saveCurrentFlowAs() {
  const source = (state.source || "").trim();
  if (!source) {
    toast("当前编辑器为空", "先输入流程内容再另存为", "bad");
    return;
  }
  const seed = (state.flowPath || "").split("/").pop()?.replace(/\.lumoflow\.ya?ml$/, "") || "flow-copy";
  const name = prompt("另存为流程名（不带扩展名）：", seed);
  if (!name) return;
  try {
    const previousPath = state.flowPath;
    const path = await call("save_flow_as", { name, source });
    await refreshFlows();
    if (state.flowPath === previousPath) {
      state.flowPath = path;
      state.savedSource = source;
      state.flow = { ...state.flow, path, source: "user", fileName: path.split(/[\\/]/).pop() };
      await state.draftController?.move(previousPath);
      if (state.orphanDraftPath) { await state.draftController?.discard(state.orphanDraftPath); state.orphanDraftPath = null; }
      renderActiveView(); renderInspector(); renderFlowList();
    }
    toast("已另存", path, "ok");
  } catch (e) {
    toast("另存失败", String(e), "bad");
  }
}

export async function importFlowFile() {
  const input = document.createElement("input");
  input.type = "file";
  input.accept = ".yaml,.yml,.lumoflow.yaml,.lumoflow.yml";
  input.hidden = true;
  document.body.appendChild(input);
  input.addEventListener("change", async () => {
    const file = input.files?.[0];
    input.remove();
    if (!file) return;
    try {
      const source = await file.text();
      if (!source.trim()) {
        toast("导入失败", "文件内容为空", "bad");
        return;
      }
      const seed = flowNameFromSource(source, pathStem(file.name));
      const name = prompt("导入为流程名（不带扩展名）：", seed);
      if (!name) return;
      const path = await call("save_flow_as", { name, source });
      await refreshFlows();
      await loadFlow(path);
      toast("已导入流程", path, "ok");
    } catch (e) {
      toast("导入失败", String(e), "bad");
    }
  }, { once: true });
  input.click();
}

export async function exportCurrentFlow() {
  let source = state.source || "";
  if (!source && state.flowPath) {
    source = await call("read_flow_source", { path: state.flowPath });
  }
  if (!source.trim()) {
    toast("导出失败", "当前没有可导出的流程", "bad");
    return;
  }
  const seed = pathStem(state.flow?.fileName || state.flowPath || "flow-export");
  await exportFlowSource(seed, source);
}

export async function exportFlowAtPath(path) {
  const source = await call("read_flow_source", { path });
  const summary = state.examples.find((f) => f.path === path);
  const seed = pathStem(summary?.fileName || path);
  await exportFlowSource(seed, source);
}

export async function revealCurrentFlowFile() {
  if (!state.flowPath) {
    toast("未选择流程", "先从流程库选择一个流程", "warn");
    return;
  }
  await call("reveal_flow_file", { path: state.flowPath });
  toast("已打开文件位置", displayFlowName(state.flow), "ok");
}

async function exportFlowSource(seed, source) {
  const name = prompt("导出为文件名（不带扩展名）：", seed || "flow-export");
  if (!name) return;
  const path = await call("export_flow_as", { name, source });
  toast("已导出流程", path, "ok");
}

function flowNameFromSource(source, fallback) {
  try {
    const ast = parseYaml(source);
    return sanitizeLocalName(ast?.metadata?.id || ast?.metadata?.name || fallback);
  } catch {
    return sanitizeLocalName(fallback);
  }
}

function pathStem(path) {
  const name = String(path || "").split(/[\\/]/).pop() || "flow";
  return sanitizeLocalName(
    name
      .replace(/\.lumoflow\.ya?ml$/i, "")
      .replace(/\.ya?ml$/i, "")
  );
}

function sanitizeLocalName(name) {
  return String(name || "flow")
    .trim()
    .replace(/[^a-zA-Z0-9_.-]+/g, "-")
    .replace(/^-+|-+$/g, "") || "flow";
}

export function renderFlowList() {
  const box = $("flowList");
  if (!state.examples.length) {
    box.innerHTML = `<div class="flow-item"><div class="title">空流程库</div><div class="meta">点击「新建流程」开始，或通过录制保存流程</div></div>`;
    return;
  }
  // Group by source: user-saved → recordings → examples.
  const groups = { user: [], recording: [], example: [] };
  for (const f of state.examples) {
    (groups[f.source] || groups.example).push(f);
  }
  const SECTION_LABELS = {
    user:      { label: "我的流程",   collapsed: false },
    recording: { label: "录制产物",   collapsed: false },
    example:   { label: "内置示例",   collapsed: true  },
  };
  const renderSection = (kind, items) => {
    if (!items.length) return "";
    const cfg = SECTION_LABELS[kind];
    const folded = state.flowSectionFolded?.[kind] ?? cfg.collapsed;
    const rows = items
      .map((f) => `
        <div class="flow-row" data-source="${kind}">
          <button class="flow-item ${f.path === state.flowPath ? "is-active" : ""}" data-path="${html(f.path)}">
            <div class="title">${html(f.name || f.id || f.fileName)}</div>
            <div class="meta">${html(f.valid ? `${f.stepCount} 步 · ${f.fileName}` : (f.error || "解析失败"))}</div>
          </button>
          <div class="flow-row-actions">
            <button class="icon-btn" data-act="export" data-path="${html(f.path)}" title="导出">⇩</button>
            <button class="icon-btn" data-act="dup"  data-path="${html(f.path)}" title="复制到我的流程">⎘</button>
            ${kind !== "example" ? `<button class="icon-btn danger" data-act="del" data-path="${html(f.path)}" title="删除">✕</button>` : ""}
          </div>
        </div>`)
      .join("");
    return `
      <div class="flow-section ${folded ? "is-folded" : ""}" data-section="${kind}">
        <div class="flow-section-head" data-toggle="${kind}">
          <span>${cfg.label} · ${items.length}</span>
          <span class="chev">${folded ? "▸" : "▾"}</span>
        </div>
        <div class="flow-section-body">${rows}</div>
      </div>`;
  };
  box.innerHTML =
    renderSection("user", groups.user)
    + renderSection("recording", groups.recording)
    + renderSection("example", groups.example);
  renderFlowConfigCard();
}

let loadSequence = 0;

export async function loadFlow(path = state.flowPath) {
  if (!path) return;
  const request = ++loadSequence;
  if (state.draftController) await state.draftController.flush();
  if (path !== state.flowPath && state.source !== state.savedSource) {
    const keep = confirm("当前流程有未保存修改。保存后再切换吗？\n\n确定 = 保存并切换\n取消 = 放弃修改并切换");
    if (keep) {
      try { await saveFlowSource(); }
      catch (error) {
        if (request === loadSequence) reportError(error);
        return;
      }
      if (request !== loadSequence) return;
      if (state.source !== state.savedSource) {
        setStatus("载入已停止：当前流程有新编辑", "warn");
        return;
      }
    } else {
      await state.draftController?.discard(state.flowPath);
    }
  }
  if (request !== loadSequence) return;
  const previousPath = state.flowPath;
  const previousSource = state.source;
  setStatus("载入中…", "warn");
  try {
    const [flow, diskSource] = await Promise.all([
      call("inspect_flow", { path }),
      call("read_flow_source", { path }),
    ]);
    if (request !== loadSequence) return;
    if (state.flowPath !== previousPath || state.source !== previousSource) {
      setStatus("载入已停止：当前流程有新编辑", "warn");
      return;
    }
    let source = diskSource;
    const draft = state.draftRecovery?.find((entry) => entry.path === path);
    if (draft) {
      const recovery = draftRecoveryState(draft, diskSource);
      if (recovery !== "redundant") {
        const choice = await chooseEditorAction({ title: recovery === "conflict" ? "磁盘文件与草稿均有修改" : "恢复未保存草稿", detail: "恢复只替换编辑缓冲区，磁盘文件保持不变。可另存为保留双方版本。", actions: [["recover", "恢复到编辑器"], ["discard", "丢弃草稿"], ["cancel", "取消打开"]] });
        if (request !== loadSequence || choice.action === "cancel") return;
        if (choice.action === "recover") source = draft.source;
        else await state.draftController.discard(path);
      } else await state.draftController.discard(path);
      state.draftRecovery = state.draftRecovery.filter((entry) => entry !== draft);
    }
    let ast = null;
    try { ast = parseYaml(source); }
    catch (error) { flow.valid = false; flow.error = String(error); }
    if (path !== previousPath) {
      state.breakpoints.clear();
      state.debugRunId = null;
      state.debugPausedAt = null;
      const bar = $("debugBar");
      if (bar) bar.style.display = "none";
    }
    state.flowPath = path;
    state.flow = { ...flow, source: state.examples.find((f) => f.path === path)?.source || flow.source };
    state.source = source;
    state.savedSource = diskSource;
    state.ast = ast;
    state.selectedStepId = null;
    state.selectedStepPath = null;
    state.selectedStepPaths?.clear();
    resetHistory();
    $("flowTitle").textContent = flow.name || flow.id || flow.fileName || "未选择流程";
    $("flowSubtitle").textContent = `${displayFlowSource(state.flow.source)} · ${flow.fileName} · ${flow.stepCount} 步 · ${flow.valid ? "校验通过" : "校验异常: " + flow.error}`;
    $("inputsJson").value = pretty(flow.valid ? defaultInputs(flow) : {});
    renderFlowList();
    renderFlowConfigCard();
    renderActiveView();
    renderInspector();
    setStatus(flow.valid ? "已载入" : "校验异常", flow.valid ? "ok" : "bad");
  } catch (error) {
    if (request === loadSequence) reportError(error);
  }
}

export async function restoreOrphanDraft(draft) {
  const choice = await chooseEditorAction({ title: "原流程文件不可用", detail: "草稿仍可恢复为未命名流程，保存时将创建新文件。", actions: [["recover", "恢复草稿"], ["discard", "丢弃草稿"], ["cancel", "取消"]] });
  if (choice.action === "cancel") return;
  if (choice.action === "discard") { await state.draftController.discard(draft.path); return; }
  state.flowPath = "";
  state.source = draft.source;
  state.savedSource = "";
  state.flow = null;
  try { state.ast = parseYaml(draft.source); } catch { state.ast = null; }
  state.orphanDraftPath = draft.path;
  state.viewMode = "code";
  resetHistory();
  renderActiveView();
  renderInspector();
  $("flowTitle").textContent = "已恢复草稿 · 请另存为";
  $("flowSubtitle").textContent = draft.path || "未命名流程";
}

export function renderFlowConfigCard() {
  const name = $("flowConfigName");
  const meta = $("flowConfigMeta");
  const reveal = $("revealFlowBtn");
  if (!name || !meta || !reveal) return;
  if (!state.flowPath) {
    name.textContent = "未选择流程";
    meta.textContent = "从下方流程库选择，或导入本地流程";
    reveal.disabled = true;
    return;
  }
  const flow = state.flow || state.examples.find((f) => f.path === state.flowPath);
  name.textContent = displayFlowName(flow);
  meta.textContent = flow
    ? `${displayFlowSource(flow.source)} · ${flow.stepCount ?? 0} 步 · ${flow.valid ? "校验通过" : "校验异常"}`
    : "当前流程文件";
  reveal.disabled = false;
}

function displayFlowName(flow) {
  if (!flow) return "当前流程";
  return flow.name || flow.id || flow.fileName || "当前流程";
}

function displayFlowSource(source) {
  return ({
    user: "我的流程",
    recording: "录制产物",
    example: "内置示例",
  })[source] || "流程文件";
}

export function defaultInputs(flow) {
  const out = {};
  for (const input of flow.inputs || []) {
    if (input.default !== undefined && input.default !== null) out[input.name] = input.default;
  }
  return out;
}

export async function saveFlowSource() {
  if (!state.flowPath) {
    if (state.source) await saveCurrentFlowAs();
    return;
  }
  const path = state.flowPath;
  const source = state.source;
  parseYaml(source);
  const savedPath = await persistFlowSource(path, source);
  if (state.flowPath !== savedPath) return;
  toast("已保存", savedPath, "ok");
  // Keep the current selection, history and execution timeline intact. A full
  // reload here made save feel like a navigation and broke rapid edit→run.
  renderActiveView();
  renderInspector();
  renderFlowConfigCard();
}

/** Persist the editor buffer before an execution command reads the flow file. */
export async function ensureCurrentFlowSaved() {
  if (!state.flowPath || state.source === state.savedSource) return state.flowPath;
  // Parse before writing so a malformed edit cannot replace the last runnable
  // version on disk with content that the backend will reject.
  const path = state.flowPath;
  const source = state.source;
  state.ast = parseYaml(source);
  return persistFlowSource(path, source);
}

const pendingSaves = new Map();

async function persistFlowSource(path, source) {
  let pending = pendingSaves.get(path);
  if (!pending) {
    pending = {
      path, tail: null, savedPath: null,
      example: state.flow?.source === "example"
        || state.examples.some((flow) => flow.path === path && flow.source === "example"),
    };
    pendingSaves.set(path, pending);
  }
  // An older disk write must finish before a newer snapshot is saved. Example
  // copies share the same queue while their original path is still in flight.
  const persist = () => writeFlowSource(pending, source);
  const saving = pending.tail ? pending.tail.catch(() => {}).then(persist) : persist();
  pending.tail = saving;
  try { return await saving; }
  finally {
    if (pending.tail === saving) {
      if (pendingSaves.get(pending.path) === pending) pendingSaves.delete(pending.path);
      if (pendingSaves.get(pending.savedPath) === pending) pendingSaves.delete(pending.savedPath);
    }
  }
}

async function writeFlowSource(pending, source) {
  // Bundled examples are read-only. Keep the operator's edits in a user copy
  // and return its path so validate/run reads exactly the document just saved.
  const { path, example } = pending;
  const savedPath = pending.savedPath || (example ? await call("duplicate_flow", { path }) : path);
  pending.savedPath = savedPath;
  pendingSaves.set(savedPath, pending);
  await call("save_flow_source", { path: savedPath, source });
  if (state.flowPath === path || state.flowPath === savedPath) {
    state.flowPath = savedPath;
    state.savedSource = source;
    if (example) state.flow = { ...state.flow, path: savedPath, source: "user", fileName: savedPath.split(/[\\/]/).pop() };
    renderFlowConfigCard();
    await state.draftController?.move(path);
  }
  if (example) {
    await refreshFlows().catch(() => {});
    toast("已保存到我的流程", savedPath, "ok");
  }
  return savedPath;
}
