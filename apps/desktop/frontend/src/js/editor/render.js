// Editor render dispatch — picks the active view renderer based on state.viewMode.

import { state } from "../state.js";
import { renderStepList } from "./steps.js";
import { renderGraph } from "./graph.js";
import { renderTree } from "./tree.js";
import { renderCode } from "./code.js";

export function renderActiveView() {
  if (state.viewMode === "steps") renderStepList();
  else if (state.viewMode === "graph") renderGraph();
  else if (state.viewMode === "tree") renderTree();
  else renderCode();
}
