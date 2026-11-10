// Execution links reorder the existing YAML sequence. They do not create a
// second graph that disagrees with the VM's source-order execution semantics.
import { load as loadYaml, dump as dumpYaml } from "../vendor/js-yaml.mjs";
import { extractSteps, findStepByPath, parseYaml, walkSteps } from "../yaml.js";

const sameParent = (a, b) => a.length === b.length && a.slice(0, -1).every((part, index) => part === b[index]);
const validPath = (path) => Array.isArray(path) && path.length > 0 && Number.isSafeInteger(path.at(-1)) && path.at(-1) >= 0;
const listAt = (steps, path) => path.slice(0, -1).reduce((value, part) => value?.[part], steps);

function openDocument(source) {
  const opened = [];
  const spans = new WeakMap();
  let ast;
  try {
    ast = loadYaml(source, { listener(event, state) {
      if (event === "open") opened.push(state.position);
      else {
        const start = opened.pop();
        if (["mapping", "sequence"].includes(state.kind) && state.result) spans.set(state.result, { start, end: Math.min(state.position, source.length) });
      }
    } });
  } catch { throw new Error("流程 YAML 无效，请先在代码视图修复后再连接执行端口"); }
  const steps = extractSteps(ast);
  if (!Array.isArray(steps)) throw new Error("流程步骤结构无效，无法调整执行顺序");
  const ids = new Set();
  walkSteps(steps, (step) => {
    if (typeof step?.id !== "string" || !step.id.trim()) throw new Error("存在无效节点 ID，请先修复后再调整执行顺序");
    if (ids.has(step.id)) throw new Error("流程存在重复节点 ID，请先改为唯一 ID 后再调整执行顺序");
    ids.add(step.id);
  });
  return { steps, spans };
}

// Ignore printed string literals. A literal bracket access is statically known;
// dynamic result lookups cannot be proven safe across a changed execution order.
function referencesInExpression(expression) {
  const references = [];
  let quote = null;
  for (let index = 0; index < expression.length; index++) {
    const character = expression[index];
    if (quote && character === "\\") { index++; continue; }
    if (quote === character) { quote = null; continue; }
    if (!quote && (character === '"' || character === "'")) { quote = character; continue; }
    if (quote || !expression.startsWith("steps", index) || /[\w.]/.test(expression[index - 1] || "")) continue;
    const access = /^steps\s*(?:\.\s*([A-Za-z_][A-Za-z0-9_]*)|\[\s*["']([^"']+)["']\s*\])/.exec(expression.slice(index));
    if (access) { references.push(access[1] || access[2]); index += access[0].length - 1; }
    else if (/^steps\s*[.[]/.test(expression.slice(index))) throw new Error("存在动态节点结果引用，无法安全确认新执行顺序，请在代码视图中调整");
  }
  return references;
}

function referencesInValue(value, { bare = false } = {}, seen = new Set()) {
  if (typeof value === "string") {
    const templates = [...value.matchAll(/\{\{([\s\S]*?)\}\}/g)];
    return (templates.length ? templates.map((match) => match[1]) : bare ? [value] : []).flatMap(referencesInExpression);
  }
  if (!value || typeof value !== "object" || seen.has(value)) return [];
  seen.add(value);
  const references = Object.values(value).flatMap((item) => referencesInValue(item, {}, seen));
  seen.delete(value);
  return references;
}

function verifyDependencies(list, nextList) {
  const before = new Map();
  const after = new Map();
  const consumers = [];
  list.forEach((sibling, index) => walkSteps([sibling], (step) => {
    before.set(step.id, index);
    consumers.push(step);
  }));
  nextList.forEach((sibling, index) => walkSteps([sibling], (step) => after.set(step.id, index)));
  for (const consumer of consumers) {
    const references = [...referencesInValue(consumer.with), ...referencesInValue(consumer.when, { bare: true })];
    if (["control.if", "control.while"].includes(consumer.action)) references.push(...referencesInValue(consumer.with?.cond, { bare: true }));
    for (const provider of references) {
      if (!before.has(provider) || before.get(provider) === before.get(consumer.id)) continue;
      if (before.get(provider) < before.get(consumer.id) && after.get(provider) >= after.get(consumer.id)) {
        throw new Error(`不能调整执行顺序：${consumer.id} 依赖 ${provider} 的结果，移动后会引用尚未执行的节点`);
      }
    }
  }
}

function sequenceSpan(source, list, spans) {
  const span = spans.get(list);
  if (!span) throw new Error("无法定位步骤列表，请先在代码中展开 YAML 共享引用");
  let { start, end } = span;
  while (start < end && /\s/.test(source[start])) start++;
  while (end > start && /\s/.test(source[end - 1])) end--;
  const column = start - source.lastIndexOf("\n", start - 1) - 1;
  while (end > start) {
    const lineStart = source.lastIndexOf("\n", end - 1) + 1;
    if (!/^\s*#/.test(source.slice(lineStart, end))) break;
    end = lineStart;
    while (end > start && /\s/.test(source[end - 1])) end--;
  }
  return { start, end, column };
}

function moveTextBlocks(source, list, nextList, spans, end) {
  const starts = list.map((step) => {
    const span = spans.get(step);
    if (!span) return null;
    let start = span.start;
    while (/\s/.test(source[start] || "")) start++;
    const line = source.lastIndexOf("\n", start - 1) + 1;
    const prefix = source.slice(line, start);
    return /^ *- +$/.test(prefix) ? { line, indent: prefix.indexOf("-") } : null;
  });
  if (starts.some((start) => !start || start.indent !== starts[0].indent)) return null;
  const blocks = new Map(list.map((step, index) => [step, source.slice(starts[index].line, starts[index + 1]?.line ?? end)]));
  const newline = source.includes("\r\n") ? "\r\n" : "\n";
  const replacement = nextList.map((step, index) => {
    const block = blocks.get(step);
    return index < nextList.length - 1 && !block.endsWith("\n") ? block + newline : block;
  }).join("");
  return source.slice(0, starts[0].line) + replacement + source.slice(end);
}

function reorderSource(source, list, nextList, spans, path) {
  const { start, end, column } = sequenceSpan(source, list, spans);
  const matches = (candidate) => {
    try { return JSON.stringify(listAt(extractSteps(parseYaml(candidate)), path)) === JSON.stringify(nextList); }
    catch { return false; }
  };
  const blockMoved = moveTextBlocks(source, list, nextList, spans, end);
  if (blockMoved && matches(blockMoved)) return blockMoved;
  const inline = source[start] === "[";
  const serialized = dumpYaml(nextList, { noRefs: true, lineWidth: -1, indent: 2, ...(inline ? { flowLevel: 0 } : {}) }).replace(/\n$/, "");
  const replacement = inline ? serialized : serialized.replace(/\n/g, `\n${" ".repeat(column)}`);
  const result = source.slice(0, start) + replacement + source.slice(end);
  if (!matches(result)) throw new Error("该 YAML 布局无法安全重排，请先在代码视图中展开步骤列表");
  return result;
}

/** A -> C means moving the entire C subtree immediately after sibling A. */
export function planGraphSequence(source, { fromPath, toPath } = {}) {
  if (!validPath(fromPath) || !validPath(toPath)) throw new Error("执行连线节点路径无效，请重新选择端口");
  if (!sameParent(fromPath, toPath)) throw new Error("执行连线只能重排同一分支的步骤，不能跨分支或嵌套作用域连接");
  const { steps, spans } = openDocument(source);
  const from = findStepByPath(steps, fromPath);
  const to = findStepByPath(steps, toPath);
  if (!from || !to) throw new Error("执行连线节点已不存在，请重新选择端口");
  if (from === to) throw new Error("节点不能连接自身");
  const parent = fromPath.at(-2) === "do" ? findStepByPath(steps, fromPath.slice(0, -2)) : null;
  if (parent?.action === "control.parallel") throw new Error("并行节点的 do 项是独立分支，不能将其改为顺序执行连线");
  const list = listAt(steps, fromPath);
  const nextList = list.filter((step) => step !== to);
  nextList.splice(nextList.indexOf(from) + 1, 0, to);
  const previousOrder = list.map((step) => step.id);
  const nextOrder = nextList.map((step) => step.id);
  const changed = previousOrder.some((id, index) => id !== nextOrder[index]);
  if (!changed) return { source, changed, previousOrder, nextOrder, requiresReplace: false };
  verifyDependencies(list, nextList);
  const nextSource = reorderSource(source, list, nextList, spans, fromPath);
  return { source: nextSource, changed, previousOrder, nextOrder, requiresReplace: true };
}
