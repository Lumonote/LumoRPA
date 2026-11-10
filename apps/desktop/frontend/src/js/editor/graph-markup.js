import { html, pretty } from "../dom.js";
import { zhAction } from "../constants.js";
import { stepBadges } from "./node-badges.js";

export function renderWithSummary(value) {
  if (!value || typeof value !== "object") return "";
  return Object.entries(value).slice(0, 3).map(([key, item]) => {
    const text = typeof item === "string" ? item : pretty(item);
    return `<span><strong>${html(key)}</strong>: ${html(text.length > 38 ? `${text.slice(0, 36)}…` : text)}</span>`;
  }).join("<br />");
}

const port = (node, direction, kind, name = "") => `<button type="button" class="graph-port" data-port-direction="${direction}" data-port-kind="${kind}" data-node-key="${html(node.key)}"${name ? ` data-port-input="${html(name)}"` : ""} aria-label="${html(`${node.step.id} ${direction === "input" ? "输入" : "输出"} ${name || "result"}`)}" title="${html(direction === "input" ? `连接前序节点的结果到 ${name}` : "拖动或点击结果端口，再选择后序节点的输入端口")}"></button>`;
const execution = (node, output = false) => {
  const direction = output ? "output" : "input";
  const label = output ? "下一步" : "执行";
  const control = `<button type="button" class="graph-port" data-port-direction="${direction}" data-port-kind="execution" data-node-key="${html(node.key)}" aria-label="${html(`${node.step.id} 执行${output ? "输出" : "输入"}`)}" title="${output ? "连接到同一分支的节点，调整执行顺序" : "接收执行顺序连线"}"></button>`;
  return output ? `<span class="graph-port-name">${label}</span>${control}` : `${control}<span class="graph-port-name">${label}</span>`;
};
const displayValue = (value) => {
  if (value === undefined) return "未设置";
  if (typeof value === "string") return value;
  try { return JSON.stringify(value); } catch { return "共享或循环数据 · 在属性中查看"; }
};

export function graphNodeMarkup(node, index) {
  const family = String(node.step.action || "misc").split(".")[0];
  const label = zhAction(node.step.action).label || node.step.action || "未知动作";
  const inputs = node.inputs.length ? node.inputs.map((input) => {
    const value = displayValue(input.value);
    return `<div class="graph-port-row is-input">${port(node, "input", "data", input.name)}<span class="graph-port-name" title="${html(`${input.name}${input.type ? ` · ${input.type}` : ""}`)}">${html(input.name)}${input.required ? '<i aria-label="必填">*</i>' : ""}</span><span class="graph-port-value${value?.includes("{{") ? " is-expression" : ""}" title="${html(value)}">${html(value)}</span></div>`;
  }).join("") : '<div class="graph-port-row"><span class="graph-port-value">无输入参数</span></div>';
  const branches = node.branches.map((branch) => `<div class="graph-port-row is-output"><span class="graph-port-name">${html(branch.label)}</span><span class="graph-port" data-port-direction="output" data-port-kind="branch" aria-hidden="true"></span></div>`).join("");
  const badges = stepBadges(node.step).map((badge) => badge.text).join(" · ");
  return `<div xmlns="http://www.w3.org/1999/xhtml" class="graph-node family-${html(family)}" data-step-path="${html(node.key)}" tabindex="0" role="group" aria-label="${html(`${node.step.id} · ${label}`)}" style="height:${node.h}px">
    <div class="graph-node-head"><div class="graph-node-eyebrow"><span>${String(index + 1).padStart(2, "0")} / ${html(family)}</span><span class="graph-node-status" data-node-run-state>待执行</span></div><div class="graph-node-title" title="${html(node.step.id)}">${html(node.step.id || "未命名节点")}</div><div class="graph-node-action" title="${html(node.step.action)}">${html(label)} <span>${html(node.step.action)}</span></div><div class="graph-node-execution"><span>${execution(node)}</span><span>${execution(node, true)}</span></div></div>
    <div class="graph-node-ports">${inputs}${branches}</div><div class="graph-node-foot"><span class="graph-node-policy" title="${html(badges)}">${html(badges || (node.step.when ? "条件执行" : "顺序执行"))}</span><span class="graph-node-result"><span class="graph-port-name">result</span>${port(node, "output", "data")}</span></div></div>`;
}
