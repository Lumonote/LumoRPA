import { $ } from "../dom.js";
import { state } from "../state.js";
import { parseYaml, findStepByPath, extractSteps } from "../yaml.js";
import { planGraphConnection } from "./graph-connections.js";
import { planGraphSequence } from "./graph-sequence.js";
import { resolveSelectionAfterEdit } from "./graph-editing.js";
import { snapshotSource } from "./history.js";
import { renderActiveView } from "./render.js";
import { renderInspector } from "./inspector.js";
import { graphLayoutKey, graphStatus } from "./graph.js";

let pending = null;
let focusBefore = null;
export function closeGraphConfirmation() {
  pending = null;
  const box = $("graphConnectConfirm"); if (box) box.hidden = true;
  if (focusBefore?.isConnected) focusBefore.focus({ preventScroll: true });
}

function commit(plan, source, flowKey) {
  if (source !== state.source || flowKey !== graphLayoutKey()) { graphStatus("流程已变化，请重新连接端口"); return; }
  if (!plan.changed) { graphStatus("这两个端口已经连接"); return; }
  snapshotSource("graph-connection");
  const ast = parseYaml(plan.source);
  state.selectedStepPath = resolveSelectionAfterEdit(state.ast, ast, state.selectedStepPath);
  state.selectedStepId = state.selectedStepPath ? findStepByPath(extractSteps(ast), state.selectedStepPath)?.id || null : null;
  state.source = plan.source; state.ast = ast;
  state.selectedStepPaths.clear();
  state.draftController?.capture();
  renderActiveView(); renderInspector();
  graphStatus(plan.nextOrder ? "执行顺序已更新 · 可撤销，保存后运行" : "数据绑定已更新 · 可撤销，保存后运行");
}

export function connectGraphPorts(options) {
  try {
    const source = state.source, flowKey = graphLayoutKey();
    const execution = options.kind === "execution";
    const plan = execution ? planGraphSequence(source, options) : planGraphConnection(source, options);
    if (!plan.requiresReplace && !options.disconnect) { commit(plan, source, flowKey); return; }
    pending = { plan, source, flowKey };
    const box = $("graphConnectConfirm");
    if (!box) return;
    focusBefore = document.activeElement;
    const previous = typeof plan.previousValue === "string" ? plan.previousValue : JSON.stringify(plan.previousValue);
    $("graphConnectMessage").textContent = execution
      ? `调整执行顺序为 ${plan.nextOrder.join(" → ")}？此操作会修改流程，现有数据依赖已检查。`
      : options.disconnect
      ? `断开 ${options.input} 的结果绑定？该参数将恢复为未设置，运行前请补充必填值。`
      : `将 ${options.input} 的现有值「${String(previous).slice(0, 150)}」替换为 ${plan.expression}？`;
    $("graphConnectApply").textContent = execution ? "调整执行顺序" : options.disconnect ? "断开绑定" : "替换并连接";
    $("graphConnectApply").onclick = () => { const saved = pending; closeGraphConfirmation(); if (saved) commit(saved.plan, saved.source, saved.flowKey); };
    $("graphConnectCancel").onclick = closeGraphConfirmation;
    box.onkeydown = (event) => {
      if (event.key === "Escape" && !event.isComposing) { event.preventDefault(); event.stopPropagation(); closeGraphConfirmation(); }
      if (event.key === "Tab") {
        event.preventDefault();
        (document.activeElement === $("graphConnectApply") ? $("graphConnectCancel") : $("graphConnectApply")).focus();
      }
    };
    box.hidden = false; $("graphConnectCancel").focus();
  } catch (error) { graphStatus(error.message || String(error)); }
}
