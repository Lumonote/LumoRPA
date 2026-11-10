// Source-aware node insertion and selection identity for canvas edits.
import { load, dump } from "../vendor/js-yaml.mjs";
import { collectStepIds, extractSteps, findStepByPath, parseYaml, pathKey, walkSteps } from "../yaml.js";
import { buildGraphModel } from "./graph-model.js";

export function nextGraphStepId(source, action) {
  if (!Array.isArray(extractSteps(parseYaml(source)))) throw new Error("流程 steps 必须是步骤列表，请先在代码视图修复");
  const ids = collectStepIds(source);
  const base = action.replace(/[^A-Za-z0-9_]/g, "_");
  let index = 1;
  while (ids.has(`${base}_${index}`)) index++;
  return `${base}_${index}`;
}

export function appendGraphStep(source, action) {
  const id = nextGraphStepId(source, action);
  const step = { id, action, with: {} };
  const spans = new WeakMap(), opened = [];
  const ast = load(source, { listener(event, item) {
    if (event === "open") opened.push(item.position);
    else {
      const start = opened.pop();
      if (item.result && typeof item.result === "object") spans.set(item.result, { start, end: item.position });
    }
  } }) || {};
  if (typeof ast !== "object" || Array.isArray(ast)) throw new Error("流程文档必须是 YAML 映射，请先在代码视图修复");
  if (ast.spec != null && (typeof ast.spec !== "object" || Array.isArray(ast.spec))) throw new Error("流程 spec 必须是映射，请先在代码视图修复");
  const steps = ast.spec?.steps;
  if (steps != null && !Array.isArray(steps)) throw new Error("流程 steps 必须是步骤列表，请先在代码视图修复");
  const span = Array.isArray(steps) && spans.get(steps);
  if (span) {
    let start = span.start; while (/\s/.test(source[start] || "")) start++;
    let end = span.end; while (end > start && /\s/.test(source[end - 1])) end--;
    if (source[start] === "[" && source[end - 1] === "]") {
      // Appending within an inline sequence also works when spec itself is a
      // flow mapping. No field outside the sequence is reserialized.
      const node = dump(step, { flowLevel: 0, lineWidth: -1 }).trim();
      const addition = `${steps.length ? ", " : ""}${node}`;
      return { id, source: source.slice(0, end - 1) + addition + source.slice(end - 1) };
    }
    const column = start - source.lastIndexOf("\n", start - 1) - 1;
    while (end > start) {
      const lineStart = source.lastIndexOf("\n", end - 1) + 1;
      const comment = /^( *)#/.exec(source.slice(lineStart, end));
      if (!comment || comment[1].length > column) break;
      end = lineStart;
      while (end > start && /\s/.test(source[end - 1])) end--;
    }
    const expected = [...steps, step];
    const rendered = dump([step], { lineWidth: -1, indent: 2 }).replace(/\n$/, "");
    const addition = `\n${" ".repeat(column)}${rendered.replace(/\n/g, `\n${" ".repeat(column)}`)}`;
    const appended = source.slice(0, end) + addition + source.slice(end);
    const equivalentSteps = (candidate) => {
      try { return JSON.stringify(extractSteps(parseYaml(candidate))) === JSON.stringify(expected); }
      catch { return false; }
    };
    if (equivalentSteps(appended)) return { id, source: appended };
    // A trailing block scalar may own the final blank lines. Serialize only
    // the list when raw appending would otherwise change that scalar's value.
    const replacement = dump(expected, { lineWidth: -1, indent: 2, noRefs: true }).replace(/\n$/, "")
      .replace(/\n/g, `\n${" ".repeat(column)}`);
    const replaced = source.slice(0, start) + replacement + source.slice(end);
    if (equivalentSteps(replaced)) {
      return { id, source: replaced };
    }
    throw new Error("无法安全添加节点，请先在代码视图中展开步骤列表");
  }
  // A new/empty document has no list span yet. Retain all configured fields.
  const next = { apiVersion: "lumorpa.io/v1", kind: "Flow", ...ast, metadata: { id: "untitled", version: "0.1.0", ...ast.metadata }, spec: { ...ast.spec, steps: [step] } };
  return { id, source: dump(next, { lineWidth: -1 }) };
}

/** Insert by the exact AST path: equal IDs in another branch are irrelevant. */
export function insertGraphStepAfter(source, path, action) {
  if (!Array.isArray(path) || !Number.isSafeInteger(path.at(-1)) || path.at(-1) < 0) throw new Error("节点路径无效，请重新选择后添加");
  const id = nextGraphStepId(source, action);
  const step = { id, action, with: {} };
  const spans = new WeakMap(), opened = [];
  const ast = load(source, { listener(event, item) {
    if (event === "open") opened.push(item.position);
    else {
      const start = opened.pop();
      if (item.result && typeof item.result === "object") spans.set(item.result, { start, end: item.position });
    }
  } });
  const steps = extractSteps(ast);
  const target = findStepByPath(steps, path);
  const list = path.slice(0, -1).reduce((value, part) => value?.[part], steps);
  if (!target || !Array.isArray(list)) throw new Error("选中的节点已不存在，请重新选择后添加");
  let references = 0;
  walkSteps(steps, (node) => { if (node === target) references++; });
  if (references !== 1) throw new Error("该节点使用 YAML 共享引用，请先在代码中展开后添加");
  const span = spans.get(list);
  if (!span) throw new Error("无法定位步骤列表，请先在代码视图中展开 YAML 引用");
  let start = span.start; while (/\s/.test(source[start] || "")) start++;
  let end = span.end; while (end > start && /\s/.test(source[end - 1])) end--;
  const column = start - source.lastIndexOf("\n", start - 1) - 1;
  const expected = [...list];
  expected.splice(path.at(-1) + 1, 0, step);
  const valid = (candidate) => {
    try {
      const nextList = path.slice(0, -1).reduce((value, part) => value?.[part], extractSteps(parseYaml(candidate)));
      return JSON.stringify(nextList) === JSON.stringify(expected);
    } catch { return false; }
  };
  const targetSpan = spans.get(target);
  if (source[start] === "[" && source[end - 1] === "]" && targetSpan) {
    let at = targetSpan.end;
    while (at > targetSpan.start && /\s/.test(source[at - 1])) at--;
    const candidate = source.slice(0, at) + ", " + dump(step, { flowLevel: 0, lineWidth: -1 }).trim() + source.slice(at);
    if (valid(candidate)) return { id, source: candidate };
  }
  // A normal block list can retain every existing node and its comments. The
  // parser span locates the next sibling even when IDs repeat elsewhere.
  if (targetSpan) {
    const next = list[path.at(-1) + 1];
    const nextSpan = next && spans.get(next);
    const insertionPoint = nextSpan
      ? source.lastIndexOf("\n", nextSpan.start - 1) + 1
      : end;
    const rendered = dump([step], { lineWidth: -1, indent: 2 }).replace(/\n$/, "")
      .replace(/\n/g, `\n${" ".repeat(column)}`);
    const addition = nextSpan ? `${" ".repeat(column)}${rendered}\n` : `\n${" ".repeat(column)}${rendered}`;
    const candidate = source.slice(0, insertionPoint) + addition + source.slice(insertionPoint);
    if (valid(candidate)) return { id, source: candidate };
  }
  while (end > start) {
    const lineStart = source.lastIndexOf("\n", end - 1) + 1;
    const comment = /^( *)#/.exec(source.slice(lineStart, end));
    if (!comment || comment[1].length > column) break;
    end = lineStart;
    while (end > start && /\s/.test(source[end - 1])) end--;
  }
  const inline = source[start] === "[";
  const serialized = dump(expected, { noRefs: true, lineWidth: -1, indent: 2, ...(inline ? { flowLevel: 0 } : {}) }).replace(/\n$/, "");
  const replacement = inline ? serialized : serialized.replace(/\n/g, `\n${" ".repeat(column)}`);
  const candidate = source.slice(0, start) + replacement + source.slice(end);
  if (!valid(candidate)) throw new Error("无法安全插入节点，请先在代码视图中展开步骤列表");
  return { id, source: candidate };
}

export function resolveSelectionAfterEdit(previousAst, nextAst, selectedPath) {
  if (!selectedPath) return null;
  const previous = buildGraphModel(extractSteps(previousAst));
  const identity = previous.nodes.get(pathKey(selectedPath))?.layoutKey;
  if (!identity) return null;
  const candidates = [...buildGraphModel(extractSteps(nextAst)).nodes.values()]
    .filter((node) => node.layoutKey === identity);
  return candidates.length === 1 ? candidates[0].path : null;
}
