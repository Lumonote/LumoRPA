// Editor render dispatch — picks the active view renderer based on state.viewMode.

import { state } from "../state.js";
import { renderStepList } from "./steps.js";
import { renderGraph } from "./graph.js";
import { renderTree } from "./tree.js";
import { renderCode } from "./code.js";

export function renderActiveView() {
  const save = document.getElementById("saveFlowBtn");
  if (save) {
    const dirty = Boolean(state.flowPath) && state.source !== state.savedSource;
    save.textContent = dirty ? "💾 保存 *" : "💾 保存";
    save.classList.toggle("is-dirty", dirty);
    save.title = dirty ? "保存当前未保存修改" : "保存当前流程";
  }
  if (state.viewMode === "steps") renderStepList();
  else if (state.viewMode === "graph") renderGraph();
  else if (state.viewMode === "tree") renderTree();
  else renderCode();
}
