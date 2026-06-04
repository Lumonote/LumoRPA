// Flow library list + load / save / create / duplicate.

import { $, html, toast, setStatus, pretty, reportError } from "./dom.js";
import { call } from "./api.js";
import { state } from "./state.js";
import { BLANK_FLOW_TEMPLATE } from "./constants.js";
import { parseYaml } from "./yaml.js";
import { renderActiveView } from "./editor/render.js";
import { renderInspector } from "./editor/inspector.js";

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
    const path = await call("save_flow_as", { name, source });
    await refreshFlows();
    await loadFlow(path);
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

export async function loadFlow(path = state.flowPath) {
  if (!path) return;
  state.flowPath = path;
  renderFlowConfigCard();
  setStatus("载入中…", "warn");
  try {
    const [flow, source] = await Promise.all([
      call("inspect_flow", { path }),
      call("read_flow_source", { path }).catch((e) => `# ${e}`),
    ]);
    state.flow = flow;
    state.source = source;
    state.ast = parseYaml(source);
    state.selectedStepId = null;
    state.selectedStepPath = null;
    $("flowTitle").textContent = flow.name || flow.id || flow.fileName || "未选择流程";
    $("flowSubtitle").textContent = `${displayFlowSource(flow.source)} · ${flow.fileName} · ${flow.stepCount} 步 · ${flow.valid ? "校验通过" : "校验异常: " + flow.error}`;
    if (flow.valid) {
      $("inputsJson").value = pretty(defaultInputs(flow));
    }
    renderFlowList();
    renderFlowConfigCard();
    renderActiveView();
    renderInspector();
    setStatus(flow.valid ? "已载入" : "校验异常", flow.valid ? "ok" : "bad");
  } catch (error) {
    reportError(error);
  }
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
  if (!state.flowPath) return;
  await call("save_flow_source", { path: state.flowPath, source: state.source });
  state.ast = parseYaml(state.source);
  toast("已保存", state.flowPath, "ok");
  await loadFlow(state.flowPath);
}
