// Tree view: collapsible nested step outline.

import { $, html } from "../dom.js";
import { state } from "../state.js";
import { extractSteps, pathKey } from "../yaml.js";
import { selectStep } from "./inspector.js";

export function renderTree() {
  const root = $("treeRoot");
  root.innerHTML = "";
  if (!state.ast) return;
  const steps = extractSteps(state.ast);
  root.appendChild(renderTreeList(steps, null));
}

function renderTreeList(list, parentPath) {
  const wrap = document.createElement("div");
  list.forEach((step, idx) => {
    const path = parentPath ? [...parentPath, idx] : [idx];
    const key = pathKey(path);
    const childKinds = ["do", "else", "catch", "finally"].filter((k) => Array.isArray(step[k]) && step[k].length);
    const hasChildren = childKinds.length > 0;
    const selected = pathKey(state.selectedStepPath || []) === key;
    const node = document.createElement("div");
    node.className = `tree-node ${hasChildren ? "" : "is-leaf"} ${selected ? "is-selected" : ""}`;
    node.dataset.stepPath = key;
    node.innerHTML = `
      <span class="caret">${hasChildren ? "▾" : "·"}</span>
      <span class="label"><span class="id">${html(step.id || "(no id)")}</span><span class="action">${html(step.action || "?")}</span></span>
      <span class="badge">${html(step.action ? step.action.split(".")[0] : "?")}</span>
    `;
    node.addEventListener("click", (e) => {
      e.stopPropagation();
      const caret = e.target.closest(".caret");
      if (caret && hasChildren) {
        const childrenEl = node.nextElementSibling;
        if (childrenEl?.classList.contains("tree-children")) {
          childrenEl.classList.toggle("is-collapsed");
          node.querySelector(".caret").textContent = childrenEl.classList.contains("is-collapsed") ? "▸" : "▾";
        }
        return;
      }
      selectStep(path);
    });
    wrap.appendChild(node);
    if (hasChildren) {
      const childWrap = document.createElement("div");
      childWrap.className = "tree-children";
      childKinds.forEach((kind) => {
        const label = document.createElement("div");
        label.style.fontSize = "10.5px";
        label.style.color = "var(--accent-2)";
        label.style.padding = "4px 4px 2px";
        label.textContent = `▿ ${kind}`;
        childWrap.appendChild(label);
        childWrap.appendChild(renderTreeList(step[kind], [...path, kind]));
      });
      wrap.appendChild(childWrap);
    }
  });
  return wrap;
}
