/* global window, document */
// Browser-only fixture: real editor modules/DOM/CSS, mocked Tauri and execution.
const schemas = {
  "file.read": { properties: { path: { type: "string" }, encoding: { type: "string" } }, required: ["path"], "x-lumo-output": { type: "string" } },
  "json.parse": { properties: { text: { type: "string" } }, required: ["text"] },
  "ai.chat": { properties: { prompt: { type: "string" }, model: { type: "string" } }, required: ["prompt"], "x-lumo-output": { type: "string" } },
  "file.write": { properties: { path: { type: "string" }, content: { type: "string" } }, required: ["path", "content"] },
  "control.log": { properties: { message: { type: "string" } } },
  "control.if": { properties: { condition: { type: "string" } } },
  "control.for": { properties: { from: { type: "integer" }, to: { type: "integer" } } },
  "http.get": { properties: { url: { type: "string" } }, required: ["url"], "x-lumo-output": { type: "object", required: ["status", "text", "json"], properties: { status: { type: "integer" }, text: { type: "string" }, json: {} } } },
};
window.__TAURI__ = { core: { invoke: async (command, args) => {
  if (command === "action_schema") return schemas[args.id] || { properties: {} };
  if (command === "list_artifacts") return [];
  throw new Error(`界面验收模拟接口不支持 ${command}`);
} } };
const { state } = await import("../src/js/state.js");
const { parseYaml } = await import("../src/js/yaml.js");
const { renderGraph, bindGraphPan, refreshGraphRunState } = await import("../src/js/editor/graph.js");
const { applyRunResponse } = await import("../src/js/runs.js");
const { createGraphRunContext } = await import("../src/js/editor/graph-run-state.js");
const { switchEditorMode } = await import("../src/js/views.js");
const { undoEdit, redoEdit } = await import("../src/js/editor/history.js");
const { renderActions } = await import("../src/js/actions.js");
const sample = `apiVersion: lumorpa.io/v1
kind: Flow
metadata: {id: content_pipeline, version: 0.1.0}
spec:
  steps:
    - id: read_brief
      action: file.read
      with: {path: briefs/today.md, encoding: utf-8}
    - id: generate_script
      action: ai.chat
      with: {prompt: "", model: local-chat}
    - id: save_script
      action: file.write
      with: {path: output/script.md, content: "{{ steps.generate_script.result }}"}
      retry: {times: 2}
  outputs: {status: ready}
`;
Object.assign(state, { flowPath: "/probe/content_pipeline.yaml", flow: { id: "content_pipeline" }, source: sample, savedSource: sample, ast: parseYaml(sample), viewMode: "graph", currentView: "design", schemaCache: new Map(Object.entries(schemas)), graphLayouts: {}, actions: Object.keys(schemas).map((id) => ({ id, family: id.split(".")[0], summary: `界面验收 · ${id}` })) });
document.documentElement.className = "theme-dark";
document.title = "Lumo 节点画布 · 模拟接口验收";
document.getElementById("appMeta").textContent = "界面验收 · 模拟接口 · 不执行业务任务";
document.getElementById("flowTitle").textContent = "内容生产流水线";
document.getElementById("flowSubtitle").textContent = "读取创作简报 → 生成脚本 → 保存结果";
document.getElementById("elementLibrary").hidden = true;
const toolbar = document.querySelector(".editor-toolbar");
for (const element of [...toolbar.children]) element.hidden = true;
const control = (text, action) => { const button = document.createElement("button"); button.textContent = text; button.addEventListener("click", action); toolbar.appendChild(button); return button; };
const reset = (source, name) => { state.source = source; state.savedSource = source; state.ast = parseYaml(source); state.flowPath = `/probe/${name}.yaml`; state.selectedStepPath = null; state.selectedStepPaths.clear(); state.editorRunContext = null; renderGraph(); };
control("重置示例", () => reset(sample, "content_pipeline"));
control("分支示例", () => reset(`spec:\n  steps:\n    - id: gate\n      action: control.if\n      with: {condition: true}\n      do:\n        - {id: yes, action: control.log, with: {message: accepted}}\n      else:\n        - {id: no, action: control.log, with: {message: rejected}}\n    - {id: done, action: control.log, with: {message: finished}}\n`, "branches"));
control("空画布", () => reset("spec: {steps: []}\n", "empty"));
control("80 节点", () => reset(`spec:\n  steps:\n${Array.from({ length: 80 }, (_, i) => `    - {id: step_${i}, action: control.log, with: {message: 'node ${i}'}}`).join("\n")}\n`, "scale"));
for (const count of [200, 300, 1000]) control(`${count} 节点`, () => reset(`spec:\n  steps:\n${Array.from({ length: count }, (_, i) => `    - {id: step_${i}, action: control.log, with: {message: 'node ${i}'}}`).join("\n")}\n`, `scale_${count}`));
control("类型端口", () => reset('spec:\n  steps:\n    - {id: fetch, action: http.get, with: {url: "https://example.test/brief"}}\n    - {id: save, action: file.write, with: {path: output/brief.txt, content: ""}}\n', "typed"));
control("循环结果", async () => {
  reset('spec:\n  steps:\n    - id: batch\n      action: control.for\n      with: {from: 0, to: 3}\n      do:\n        - {id: row, action: control.log, with: {message: "{{ loop.index }}"}}\n    - {id: done, action: control.log, with: {message: finished}}\n', "loop_results");
  state.editorRunContext = createGraphRunContext({ source: state.source, flowPath: state.flowPath, runId: "probe-loop-run" });
  await applyRunResponse({ run: { id: "probe-loop-run", state: "ok" }, report: { runId: "probe-loop-run", success: true, stepsTotal: 5, stepsOk: 5, stepsFailed: 0, durationMs: 124, outputs: {} }, steps: [
    { stepId: "row", path: "batch[0]/row", seq: 1, state: "ok", durationMs: 12, outputJson: false },
    { stepId: "row", path: "batch[1]/row", seq: 2, state: "ok", durationMs: 14, outputJson: 0 },
    { stepId: "row", path: "batch[2]/row", seq: 3, state: "ok", durationMs: 16, outputJson: [{ name: "镜头一", duration: 4 }, { name: "镜头二", duration: 6 }] },
    { stepId: "batch", path: "batch", seq: 4, state: "ok", durationMs: 100, outputJson: null },
    { stepId: "done", path: "done", seq: 5, state: "ok", durationMs: 24, outputJson: "" },
  ] });
});
control("撤销", undoEdit); control("重做", redoEdit);
control("模拟执行状态", () => { state.editorRunContext = { source: state.source, flowPath: state.flowPath, runId: "probe", stepStates: Object.fromEntries([...document.querySelectorAll(".graph-node")].map((element, index) => [element.querySelector(".graph-node-title").textContent, { state: index === 0 ? "ok" : index === 1 ? "running" : "idle" }])) }; refreshGraphRunState(); });
control("切换主题", () => document.documentElement.classList.toggle("theme-dark"));
const sourceButton = control("查看 YAML", () => { const box = document.getElementById("probeSource"); box.hidden = !box.hidden; box.value = state.source; });
sourceButton.id = "probeSourceToggle";
const sourceBox = document.createElement("textarea"); sourceBox.id = "probeSource"; sourceBox.readOnly = true; sourceBox.hidden = true; sourceBox.setAttribute("aria-label", "当前流程 YAML"); sourceBox.style.cssText = "position:fixed;z-index:50;inset:100px 12% 70px;width:76%;height:65%;padding:20px;background:#17212a;color:#d9e5e9;border:1px solid #668c83;font:13px monospace";
document.body.appendChild(sourceBox);
document.getElementById("viewSwitch").onclick = (event) => { if (event.target.dataset.mode) switchEditorMode(event.target.dataset.mode); };
document.addEventListener("keydown", (event) => { if (event.key === "Escape") sourceBox.hidden = true; if (event.defaultPrevented || ["INPUT", "TEXTAREA"].includes(event.target.tagName)) return; if ((event.ctrlKey || event.metaKey) && event.key === "z") { event.preventDefault(); (event.shiftKey ? redoEdit : undoEdit)(); } });
renderActions(); switchEditorMode("graph"); bindGraphPan();
