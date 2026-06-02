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
}

export async function createNewFlow() {
  const proposed = `flow-${new Date().toISOString().slice(0, 10)}`;
  const name = prompt("流程文件名（不带扩展名）：", proposed);
  if (!name) return;
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
}

export async function loadFlow(path = $("flowPath").value.trim()) {
  if (!path) return;
  state.flowPath = path;
  $("flowPath").value = path;
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
    $("flowSubtitle").textContent = `${flow.path}  ·  ${flow.stepCount} 步  ·  ${flow.valid ? "校验通过" : "校验异常: " + flow.error}`;
    if (flow.valid) {
      $("inputsJson").value = pretty(defaultInputs(flow));
    }
    renderFlowList();
    renderActiveView();
    renderInspector();
    setStatus(flow.valid ? "已载入" : "校验异常", flow.valid ? "ok" : "bad");
  } catch (error) {
    reportError(error);
  }
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
