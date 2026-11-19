// YAML parser + textual mutation helpers. The Code view remains the source of
// truth; js-yaml gives the editor the same YAML semantics as the Rust runtime
// (quoted strings, block scalars, nested branches and flow-style collections).
import { load as loadYaml, dump as dumpYaml } from "./vendor/js-yaml.mjs";

export function parseYaml(text) {
  const value = loadYaml(String(text ?? ""));
  return value && typeof value === "object" ? value : {};
}

export function parseScalar(s) {
  s = s.trim();
  if (s === "") return null;
  // Flow-style inline list
  if (s.startsWith("[") && s.endsWith("]")) {
    const inner = s.slice(1, -1);
    if (!inner.trim()) return [];
    return splitFlow(inner).map((p) => parseScalar(p));
  }
  if (s.startsWith("{") && s.endsWith("}")) {
    const inner = s.slice(1, -1);
    const obj = {};
    for (const part of splitFlow(inner)) {
      const idx = part.indexOf(":");
      if (idx > 0) {
        const k = part.slice(0, idx).trim();
        const v = part.slice(idx + 1).trim();
        obj[k] = parseScalar(v);
      }
    }
    return obj;
  }
  // Strings
  if ((s.startsWith('"') && s.endsWith('"')) || (s.startsWith("'") && s.endsWith("'"))) {
    return s.slice(1, -1);
  }
  if (s === "null" || s === "~") return null;
  if (s === "true") return true;
  if (s === "false") return false;
  if (/^-?\d+$/.test(s)) return Number(s);
  if (/^-?\d+\.\d+$/.test(s)) return Number(s);
  return s;
}

export function splitFlow(s) {
  const out = [];
  let buf = "";
  let depth = 0;
  let inStr = null;
  for (const ch of s) {
    if (inStr) {
      buf += ch;
      if (ch === inStr) inStr = null;
      continue;
    }
    if (ch === '"' || ch === "'") { inStr = ch; buf += ch; continue; }
    if (ch === "[" || ch === "{") { depth++; buf += ch; continue; }
    if (ch === "]" || ch === "}") { depth--; buf += ch; continue; }
    if (ch === "," && depth === 0) { out.push(buf.trim()); buf = ""; continue; }
    buf += ch;
  }
  if (buf.trim()) out.push(buf.trim());
  return out;
}

export function extractSteps(ast) {
  return ast?.spec?.steps || [];
}

export const STEP_CHILD_KINDS = ["do", "else", "catch", "finally"];

export function childStepBlocks(step) {
  const blocks = STEP_CHILD_KINDS
    .filter((kind) => Array.isArray(step?.[kind]) && step[kind].length)
    .map((kind) => ({
      kind,
      label: kind,
      path: [kind],
      steps: step[kind],
    }));
  if (Array.isArray(step?.branches)) {
    step.branches.forEach((branch, index) => {
      if (!Array.isArray(branch) || !branch.length) return;
      blocks.push({
        kind: "branches",
        label: `branch[${index}]`,
        path: ["branches", index],
        steps: branch,
      });
    });
  }
  return blocks;
}

export function walkSteps(steps, cb, parent = null, depth = 0, ancestors = new Set()) {
  if (!Array.isArray(steps)) throw new Error("流程 steps 必须是步骤列表");
  steps.forEach((step, idx) => {
    if (ancestors.has(step)) throw new Error("流程包含循环 YAML 引用，请先展开后编辑");
    const path = parent ? [...parent, idx] : [idx];
    cb(step, { parent, idx, depth, path });
    const nextAncestors = new Set([...ancestors, step]);
    childStepBlocks(step).forEach((block) => {
      walkSteps(block.steps, cb, [...path, ...block.path], depth + 1, nextAncestors);
    });
  });
}

// Retain scalar positions as well as object positions. This lets identity edits
// change only actual YAML values, leaving comments and unrelated text intact.
function yamlDocument(source) {
  const stack = [], spans = new WeakMap();
  let root;
  const ast = loadYaml(source, { listener(event, item) {
    if (event === "open") stack.push({ start: item.position, children: [] });
    else {
      const node = stack.pop();
      Object.assign(node, { end: Math.min(item.position, source.length), kind: item.kind, value: item.result });
      if (node.value && typeof node.value === "object") spans.set(node.value, node);
      if (stack.length) stack.at(-1).children.push(node); else root = node;
    }
  } });
  return { ast, root, spans };
}

/** Rewrite static steps.foo / steps['foo'] lookups, excluding quoted literals. */
export function rewriteStepExpression(expression, renames) {
  let result = "", quote = null;
  for (let index = 0; index < expression.length;) {
    const character = expression[index];
    if (quote) {
      result += character; index++;
      if (character === "\\" && index < expression.length) result += expression[index++];
      else if (character === quote) quote = null;
      continue;
    }
    if (character === '"' || character === "'") { quote = character; result += character; index++; continue; }
    if (expression.startsWith("steps", index) && !/[\w.$]/.test(expression[index - 1] || "")) {
      const access = /^steps\s*(?:\.\s*([A-Za-z_][A-Za-z0-9_]*)|\[\s*("(?:\\.|[^"\\])*"|'(?:\\.|[^'\\])*')\s*\])/.exec(expression.slice(index));
      if (access) {
        let id = access[1];
        if (id === undefined) {
          try { id = access[2][0] === '"' ? JSON.parse(access[2]) : access[2].slice(1, -1).replace(/\\(['\\])/g, "$1"); }
          catch { id = null; }
        }
        if (renames.has(id)) {
          const next = renames.get(id);
          result += access[1] && /^[A-Za-z_][A-Za-z0-9_]*$/.test(next) ? `steps.${next}` : `steps[${JSON.stringify(next)}]`;
          index += access[0].length; continue;
        }
      }
    }
    result += character; index++;
  }
  return result;
}

function rewriteYamlStepValues(source, renames, { renameIds = false, stepsRoot = false } = {}) {
  if (!renames.size) return source;
  const { ast, root } = yamlDocument(source);
  const steps = stepsRoot ? ast : extractSteps(ast);
  const stepObjects = new Set(), conditions = new Set();
  walkSteps(steps, (step) => {
    stepObjects.add(step);
    if (["control.if", "control.while"].includes(step?.action) && step.with && typeof step.with === "object") conditions.add(step.with);
  });
  const edits = [];
  const visit = (node, parent, key) => {
    if (node.children.length === 1 && node.children[0].value === node.value && ["mapping", "sequence"].includes(node.kind)) visit(node.children[0], parent, key);
    else if (node.kind === "mapping") {
      for (let index = 0; index + 1 < node.children.length; index += 2) visit(node.children[index + 1], node.value, node.children[index].value);
    } else if (node.kind === "sequence") node.children.forEach((child, index) => visit(child, node.value, index));
    else if (typeof node.value === "string") {
      const bare = stepObjects.has(parent) && key === "when" || conditions.has(parent) && key === "cond";
      const next = renameIds && stepObjects.has(parent) && key === "id" && renames.has(node.value)
        ? renames.get(node.value)
        : bare && !node.value.includes("{{") ? rewriteStepExpression(node.value, renames)
          : node.value.replace(/\{\{([\s\S]*?)\}\}/g, (_all, expression) => `{{${rewriteStepExpression(expression, renames)}}}`);
      if (next === node.value) return;
      let start = node.start, end = node.end;
      while (start < end && /\s/.test(source[start])) start++;
      while (end > start && /\s/.test(source[end - 1])) end--;
      edits.push({ start, end, text: renameIds && stepObjects.has(parent) && key === "id" ? yamlScalar(next) : JSON.stringify(next) });
    }
  };
  visit(root, null, null);
  let result = source;
  for (const edit of edits.sort((a, b) => b.start - a.start)) result = result.slice(0, edit.start) + edit.text + result.slice(edit.end);
  loadYaml(result);
  return result;
}

/** Apply the node form and migrate its static references as one source edit. */
export function mutateStepWithReferences(source, path, patch) {
  const steps = extractSteps(parseYaml(source));
  const original = findStepByPath(steps, path);
  if (!original) throw new Error("选中的节点已不存在，请重新选择");
  const renamed = original.id !== patch.id;
  if (renamed) {
    let oldCount = 0, newCount = 0;
    walkSteps(steps, (step) => { if (step.id === original.id) oldCount++; if (step.id === patch.id) newCount++; });
    if (oldCount !== 1 || newCount) throw new Error("重命名需要唯一的节点 ID，请先修复重复 ID");
  }
  const updated = mutateStepAtPath(source, path, patch);
  return renamed ? rewriteYamlStepValues(updated, new Map([[original.id, patch.id]])) : updated;
}

export function findStepByPath(steps, path) {
  let cur = steps;
  let parent = null;
  for (let i = 0; i < path.length; i++) {
    const seg = path[i];
    if (typeof seg === "number") {
      if (!Array.isArray(cur)) return null;
      parent = cur[seg];
      if (!parent || Array.isArray(parent)) return null;
      if (i === path.length - 1) return parent;
      cur = parent;
    } else if (seg === "branches") {
      const branchIndex = path[++i];
      if (typeof branchIndex !== "number") return null;
      cur = parent?.branches?.[branchIndex] || [];
    } else {
      cur = parent?.[seg] || [];
    }
  }
  return parent;
}

// F-20: the VM identifies a step by a slash-joined chain of *step ids* down
// the nesting tree (`parentId/childId/...`, see lumo-core vm.rs run_block_at).
// `path` is the index-based selection path (e.g. [0, "do", 1]); we walk it,
// collecting each step node's id, so breakpoints on NESTED nodes match the
// path the VM actually checks instead of just the leaf id.
export function stepIdChain(steps, path) {
  const ids = [];
  let cur = steps;
  let node = null;
  for (let i = 0; i < path.length; i++) {
    const seg = path[i];
    if (typeof seg === "number") {
      if (!Array.isArray(cur)) return null;
      node = cur ? cur[seg] : null;
      if (!node || Array.isArray(node)) return null;
      ids.push(node.id);
      cur = node;
    } else if (seg === "branches") {
      const branchIndex = path[++i];
      if (typeof branchIndex !== "number") return null;
      // vm.rs run_parallel names each branch scope `branch[i]` in the path,
      // so the chain must include it or branch-nested breakpoints never match.
      ids.push(`branch[${branchIndex}]`);
      cur = node?.branches?.[branchIndex] || null;
      node = null;
    } else {
      cur = node ? node[seg] : null;
    }
  }
  if (ids.some((id) => id == null || id === "")) return null;
  return ids.join("/");
}

export function pathKey(path) {
  return (path || []).map((p) => (typeof p === "number" ? String(p) : `:${p}`)).join("/");
}

export function parsePathKey(key) {
  if (!key) return [];
  return key.split("/").map((s) => (s.startsWith(":") ? s.slice(1) : Number(s)));
}

export function cssEscape(s) {
  return s.replace(/(["\\:])/g, "\\$1");
}

// ─── YAML mutation helpers (insert / delete / move / ai / capability) ─────

function stepIdLineRegex(originalId) {
  const escaped = String(originalId).replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
  const value = `(?:"${escaped}"|'${escaped}'|${escaped})`;
  return new RegExp(
    `^(\\s*)-\\s*(?:id:\\s*${value}(?=\\s*(?:#|$))|\\{[^}]*\\bid\\s*:\\s*${value}(?=\\s*(?:[,}]))[^}]*\\})`
  );
}

export function findStepRange(lines, originalId) {
  const re = stepIdLineRegex(originalId);
  let startIdx = -1;
  let baseIndent = 0;
  for (let i = 0; i < lines.length; i++) {
    const m = lines[i].match(re);
    if (m) { startIdx = i; baseIndent = m[1].length; break; }
  }
  if (startIdx < 0) return null;
  let endIdx = lines.length;
  for (let j = startIdx + 1; j < lines.length; j++) {
    const t = lines[j];
    if (t.trim() === "") continue;
    const ind = t.match(/^( *)/)[1].length;
    if (ind <= baseIndent && !/^\s*$/.test(t)) { endIdx = j; break; }
  }
  return { startIdx, endIdx, baseIndent };
}

export function ensureLlmCapability(source) {
  const ast = parseYaml(source);
  const llm = ast?.spec?.capabilities?.llm;
  if (Array.isArray(llm) && (llm.includes("*") || llm.length > 0)) return source;
  return ensureCapability(source, "llm", "*");
}

export function ensureNetworkCapability(source, host) {
  const ast = parseYaml(source);
  const net = ast?.spec?.capabilities?.network || [];
  if (Array.isArray(net) && (net.includes(host) || net.includes("*"))) return source;
  return ensureCapability(source, "network", host);
}

/** Add `<key>: [<value>]` to the spec.capabilities block. Idempotent. */
export function ensureCapability(source, key, value) {
  const lines = source.split(/\r?\n/);
  const specIdx = lines.findIndex((l) => /^spec\s*:/.test(l));
  if (specIdx < 0) {
    // No spec block — append at end.
    return source + `\nspec:\n  capabilities:\n    ${key}:\n      - ${yamlScalar(value)}\n`;
  }
  // Find capabilities: within spec.
  let capIdx = -1;
  let capIndent = 2;
  for (let i = specIdx + 1; i < lines.length; i++) {
    const t = lines[i];
    if (/^\S/.test(t)) break; // left spec
    if (/^\s*capabilities\s*:/.test(t)) { capIdx = i; capIndent = (t.match(/^( *)/)[1].length); break; }
  }
  if (capIdx < 0) {
    // Insert capabilities block right after spec:
    const indent = "  ";
    const insertAt = specIdx + 1;
    const block = [
      `${indent}capabilities:`,
      `${indent}  ${key}:`,
      `${indent}    - ${yamlScalar(value)}`,
    ];
    return lines.slice(0, insertAt).concat(block).concat(lines.slice(insertAt)).join("\n");
  }
  // Find <key>: within capabilities.
  let keyIdx = -1;
  let keyIndent = capIndent + 2;
  for (let j = capIdx + 1; j < lines.length; j++) {
    const t = lines[j];
    if (t.trim() === "") continue;
    const ind = t.match(/^( *)/)[1].length;
    if (ind <= capIndent && !/^\s*$/.test(t)) break;
    const re = new RegExp(`^\\s{${keyIndent}}${key}\\s*:`);
    if (re.test(t)) { keyIdx = j; break; }
  }
  if (keyIdx < 0) {
    // Append `<key>:\n  - <value>` after capabilities:
    const ind = " ".repeat(keyIndent);
    const block = [`${ind}${key}:`, `${ind}  - ${yamlScalar(value)}`];
    return lines.slice(0, capIdx + 1).concat(block).concat(lines.slice(capIdx + 1)).join("\n");
  }
  // Append `- value` under existing key block.
  const itemIndent = " ".repeat(keyIndent + 2);
  return lines.slice(0, keyIdx + 1).concat([`${itemIndent}- ${yamlScalar(value)}`]).concat(lines.slice(keyIdx + 1)).join("\n");
}

export function mutateStepInSource(source, originalId, patch) {
  const lines = source.split(/\r?\n/);
  const re = stepIdLineRegex(originalId);
  let startIdx = -1;
  let baseIndent = 0;
  for (let i = 0; i < lines.length; i++) {
    const m = lines[i].match(re);
    if (m) { startIdx = i; baseIndent = m[1].length; break; }
  }
  if (startIdx < 0) {
    // Fallback: append as new step at end of `steps:` section.
    return source + `\n${emitStep(patch, 4)}`;
  }
  // Determine block end: next sibling line with same indent OR less.
  let endIdx = lines.length;
  for (let j = startIdx + 1; j < lines.length; j++) {
    const t = lines[j];
    if (t.trim() === "") continue;
    const ind = t.match(/^( *)/)[1].length;
    if (ind <= baseIndent && !/^\s*$/.test(t)) { endIdx = j; break; }
  }
  // Re-emit block.
  const replacement = emitStep(patch, baseIndent);
  return lines.slice(0, startIdx).concat(replacement.split("\n")).concat(lines.slice(endIdx)).join("\n");
}

/** Replace exactly the selected node, including compact/flow-style branches.
 * Parser spans avoid matching a similar id inside another branch or a string.
 * Regions outside the mapping remain byte-for-byte intact.
 */
export function mutateStepAtPath(source, path, patch) {
  const opened = [];
  const spans = new WeakMap();
  const ast = loadYaml(source, { listener(event, state) {
    if (event === "open") opened.push(state.position);
    else {
      const start = opened.pop();
      if (state.kind === "mapping" && state.result) spans.set(state.result, { start, end: Math.min(state.position, source.length) });
    }
  } });
  const steps = extractSteps(ast);
  const step = findStepByPath(steps, path);
  if (!step) throw new Error("选中的节点已不存在，请重新选择");
  let references = 0;
  walkSteps(steps, (candidate) => { if (candidate === step) references++; });
  if (references !== 1) throw new Error("该节点使用 YAML 共享引用，请先在代码中展开后编辑");
  const span = spans.get(step);
  if (!span) throw new Error("无法定位节点，请先在代码中展开 YAML 引用");
  let start = span.start;
  while (start < span.end && /\s/.test(source[start])) start++;
  if (source[start] === "&" || source[start] === "*") throw new Error("该节点使用 YAML 共享引用，请先在代码中展开后编辑");
  const column = start - source.lastIndexOf("\n", start - 1) - 1;
  let end = span.end;
  while (end > start && /\s/.test(source[end - 1])) end--;
  // The parser consumes separator comments while looking for a sibling.
  // Keep these outside the replacement, even when they were attached to the
  // preceding mapping span by the loader.
  while (end > start) {
    const lineStart = source.lastIndexOf("\n", end - 1) + 1;
    const comment = /^( *)#/.exec(source.slice(lineStart, end));
    if (!comment || comment[1].length >= column) break;
    end = lineStart;
    while (end > start && /\s/.test(source[end - 1])) end--;
  }
  const inline = source[start] === "{";
  const serialized = dumpYaml(patch, { noRefs: true, lineWidth: -1, indent: 2, ...(inline ? { flowLevel: 0 } : {}) }).replace(/\n$/, "");
  const replacement = inline ? serialized : serialized.replace(/\n/g, `\n${" ".repeat(column)}`);
  const result = source.slice(0, start) + replacement + source.slice(end);
  // Unsupported YAML layouts must fail before changing the editor buffer.
  loadYaml(result);
  return result;
}

export function emitStep(step, baseIndent) {
  const pad = " ".repeat(baseIndent);
  const cIndent = " ".repeat(baseIndent + 2);
  let out = `${pad}- id: ${yamlScalar(step.id)}\n${cIndent}action: ${yamlScalar(step.action)}\n`;
  if (step.when !== undefined && step.when !== null && step.when !== "") {
    out += emitYamlField("when", step.when, baseIndent + 2);
  }
  if (step.bind) out += emitYamlField("bind", step.bind, baseIndent + 2);
  if (step.resource != null) out += emitYamlField("resource", step.resource, baseIndent + 2);
  if (step.with && Object.keys(step.with).length) {
    out += `${cIndent}with:\n`;
    out += emitYamlMapEntries(step.with, baseIndent + 4);
  }
  if (step.retry && Object.keys(step.retry).length) {
    out += `${cIndent}retry:\n`;
    out += emitYamlMapEntries(step.retry, baseIndent + 4);
  }
  // Failure handling. Emitted only when it actually differs from the default,
  // so re-saving an untouched step does not grow the file with a no-op block.
  // `output` is the reason this has to round-trip at all: drop it and a
  // downstream `{{ steps.x.result.* }}` silently becomes undefined.
  const oe = step.on_error;
  if (oe && Object.keys(oe).length && (oe.mode !== "interrupt" || Object.keys(oe).some((key) => key !== "mode"))) {
    out += `${cIndent}on_error:\n`;
    out += emitYamlMapEntries(oe, baseIndent + 4);
  }
  if (step.ai && Object.keys(step.ai).length) {
    out += `${cIndent}ai:\n`;
    out += emitYamlMapEntries(step.ai, baseIndent + 4);
  }
  for (const kind of ["do", "else", "catch", "finally"]) {
    const arr = step[kind];
    if (Array.isArray(arr) && arr.length) {
      out += `${cIndent}${kind}:\n`;
      for (const child of arr) {
        out += emitStep(child, baseIndent + 4) + "\n";
      }
    }
  }
  if (Array.isArray(step.branches) && step.branches.length) {
    const branchIndent = baseIndent + 4;
    const branchPad = " ".repeat(branchIndent);
    out += `${cIndent}branches:\n`;
    for (const branch of step.branches) {
      if (!Array.isArray(branch) || !branch.length) {
        out += `${branchPad}- []\n`;
        continue;
      }
      out += `${branchPad}-\n`;
      for (const child of branch) {
        out += emitStep(child, branchIndent + 2) + "\n";
      }
    }
  }
  const known = new Set(["id", "action", "when", "bind", "resource", "with", "retry", "on_error", "ai", "do", "else", "catch", "finally", "branches"]);
  for (const [key, value] of Object.entries(step)) {
    if (!known.has(key) && value !== undefined) out += emitYamlField(key, value, baseIndent + 2);
  }
  return out.replace(/\n$/, "");
}

function emitYamlMapEntries(obj, indent) {
  return Object.entries(obj)
    .filter(([, v]) => v !== undefined)
    .map(([k, v]) => emitYamlField(k, v, indent))
    .join("");
}

function emitYamlField(key, value, indent) {
  const pad = " ".repeat(indent);
  const name = yamlKey(key);
  if (value === null || value === undefined) return `${pad}${name}: ~\n`;
  if (typeof value === "string" && value.includes("\n")) {
    const blockIndent = " ".repeat(indent + 2);
    return `${pad}${name}: |\n${value.split("\n").map((line) => `${blockIndent}${line}`).join("\n")}\n`;
  }
  if (Array.isArray(value)) {
    if (!value.length) return `${pad}${name}: []\n`;
    return `${pad}${name}:\n${value.map((item) => emitYamlListItem(item, indent + 2)).join("")}`;
  }
  if (isPlainObject(value)) {
    const entries = Object.entries(value).filter(([, v]) => v !== undefined);
    if (!entries.length) return `${pad}${name}: {}\n`;
    return `${pad}${name}:\n${emitYamlMapEntries(Object.fromEntries(entries), indent + 2)}`;
  }
  return `${pad}${name}: ${yamlInline(value)}\n`;
}

function emitYamlListItem(value, indent) {
  const pad = " ".repeat(indent);
  if (Array.isArray(value)) {
    if (!value.length) return `${pad}- []\n`;
    return `${pad}-\n${value.map((item) => emitYamlListItem(item, indent + 2)).join("")}`;
  }
  if (isPlainObject(value)) {
    const entries = Object.entries(value).filter(([, v]) => v !== undefined);
    if (!entries.length) return `${pad}- {}\n`;
    return `${pad}-\n${emitYamlMapEntries(Object.fromEntries(entries), indent + 2)}`;
  }
  if (typeof value === "string" && value.includes("\n")) {
    const blockIndent = " ".repeat(indent + 2);
    return `${pad}- |\n${value.split("\n").map((line) => `${blockIndent}${line}`).join("\n")}\n`;
  }
  return `${pad}- ${yamlInline(value)}\n`;
}

function isPlainObject(value) {
  return value && typeof value === "object" && !Array.isArray(value);
}

function yamlKey(key) {
  return /^[A-Za-z_][\w.-]*$/.test(String(key)) ? String(key) : JSON.stringify(String(key));
}

export function yamlScalar(s) {
  if (typeof s !== "string") return JSON.stringify(s);
  if (/^[\w.\-/]+$/.test(s) && loadYaml(s) === s) return s;
  return JSON.stringify(s);
}

export function yamlInline(v) {
  if (v === null || v === undefined) return "~";
  if (typeof v === "string") {
    if (v.includes("\n")) return `|\n      ${v.split("\n").join("\n      ")}`;
    if (/[:#{}[\]&*!|<>='"%@`,]/.test(v) || /^\s|\s$/.test(v)) return JSON.stringify(v);
    try { if (loadYaml(v) === v) return v; } catch {}
    return JSON.stringify(v);
  }
  if (typeof v === "boolean" || typeof v === "number") return String(v);
  return JSON.stringify(v);
}

// ─── Step-block clipboard helpers (copy / duplicate / paste / batch delete) ──
// All operate on the YAML source text so comments inside untouched regions
// survive, mirroring the mutation helpers above.

/** Every step id in the flow, including nested do/else/catch/finally/branches. */
export function collectStepIds(source) {
  const ids = new Set();
  walkSteps(extractSteps(parseYaml(source)), (step) => {
    if (step?.id != null && step.id !== "") ids.add(String(step.id));
  });
  return ids;
}

/** The step's full block (children included), dedented to column 0. */
export function extractStepBlock(source, id) {
  const lines = source.split(/\r?\n/);
  const range = findStepRange(lines, id);
  if (!range) {
    const matches = [];
    walkSteps(extractSteps(parseYaml(source)), (step) => { if (step.id === id) matches.push(step); });
    return matches.length === 1 ? { text: dumpYaml(matches, { noRefs: true, lineWidth: -1 }), baseIndent: 0 } : null;
  }
  const text = lines
    .slice(range.startIdx, range.endIdx)
    .map((l) => (l.trim() === "" ? "" : l.slice(range.baseIndent)))
    .join("\n")
    .replace(/\n*$/, "\n");
  return { text, baseIndent: range.baseIndent };
}

export function extractStepBlockAtPath(source, path) {
  const steps = extractSteps(parseYaml(source));
  const step = findStepByPath(steps, path);
  if (!step) return null;
  let occurrences = 0;
  walkSteps(steps, (candidate) => { if (candidate.id === step.id) occurrences++; });
  return occurrences === 1 ? extractStepBlock(source, step.id)
    : { text: dumpYaml([step], { noRefs: true, lineWidth: -1 }), baseIndent: 0 };
}

function clipboardRenames(stepLists, existingIds) {
  const taken = new Set(existingIds), seen = new Set(), renames = new Map();
  for (const steps of stepLists) walkSteps(steps, (step) => {
    if (!step || typeof step.id !== "string" || !step.id.trim()) throw new Error("复制内容包含无效节点 ID");
    if (seen.has(step.id)) throw new Error("复制内容包含重复节点 ID，无法安全迁移内部引用");
    seen.add(step.id);
  });
  for (const steps of stepLists) walkSteps(steps, (step) => {
    const id = step.id;
    let next = id;
    if (taken.has(id)) {
      const base = id.replace(/[^\w.-]+/g, "_").replace(/^_+|_+$/g, "") || "step";
      let number = 1;
      while (taken.has(`${base}_c${number}`) || seen.has(`${base}_c${number}`)) number++;
      next = `${base}_c${number}`;
      renames.set(id, next);
    }
    taken.add(next);
  });
  return renames;
}

/**
 * Rename ids in a dedented block that collide with `existingIds` (nested ids
 * too), and rewrite `steps.<oldId>` template references inside the block so a
 * pasted copy keeps referring to its own steps. Returns { text, renames }.
 */
export function uniquifyStepIds(blockText, existingIds) {
  const steps = loadYaml(blockText);
  const renames = clipboardRenames([steps], existingIds);
  const text = rewriteYamlStepValues(blockText, renames, { renameIds: true, stepsRoot: true });
  return { text, renames };
}

/** Insert a dedented block as the sibling right after `targetId`. */
export function insertStepBlockAfter(source, targetId, blockText) {
  const lines = source.split(/\r?\n/);
  const range = findStepRange(lines, targetId);
  if (!range) return null;
  const pad = " ".repeat(range.baseIndent);
  const blockLines = blockText
    .replace(/\n+$/, "")
    .split("\n")
    .map((l) => (l === "" ? "" : pad + l));
  return lines.slice(0, range.endIdx).concat(blockLines).concat(lines.slice(range.endIdx)).join("\n");
}

/**
 * Paste `blocks` (dedented step blocks) in order after `targetId`; a null
 * target appends after the last top-level step. Colliding ids are renamed
 * before each insert.
 */
export function insertStepBlocks(source, targetId, blocks) {
  let targetPath = null;
  if (targetId != null) {
    const paths = [];
    walkSteps(extractSteps(parseYaml(source)), (step, info) => { if (String(step.id) === targetId) paths.push(info.path); });
    if (paths.length !== 1) throw new Error("粘贴目标不存在或 ID 不唯一，请重新选择节点");
    [targetPath] = paths;
  }
  return planStepBlockInsertion(source, targetPath, blocks).source;
}

/** Plan one insertion for the complete selection, with one shared ID mapping. */
export function planStepBlockInsertion(source, targetPath, blocks) {
  const document = yamlDocument(source), ast = document.ast || {};
  const steps = extractSteps(ast);
  const lists = blocks.map((block) => loadYaml(block));
  const renames = clipboardRenames(lists, collectStepIds(source));
  const texts = blocks.map((block) => rewriteYamlStepValues(block, renames, { renameIds: true, stepsRoot: true }));
  const inserted = texts.flatMap((text) => loadYaml(text));
  if (!inserted.length) return { source, insertedCount: 0, ids: [] };
  const list = targetPath ? targetPath.slice(0, -1).reduce((value, part) => value?.[part], steps) : steps;
  if (!Array.isArray(list) || targetPath && !findStepByPath(steps, targetPath)) throw new Error("粘贴目标已不存在，请重新选择节点");
  const index = targetPath ? targetPath.at(-1) + 1 : list.length;
  const expected = [...list.slice(0, index), ...inserted, ...list.slice(index)];
  const ids = inserted.map((step) => step.id);
  const matches = (candidate) => {
    try {
      const nextSteps = extractSteps(parseYaml(candidate));
      const nextList = targetPath ? targetPath.slice(0, -1).reduce((value, part) => value?.[part], nextSteps) : nextSteps;
      return JSON.stringify(nextList) === JSON.stringify(expected);
    } catch { return false; }
  };
  const span = document.spans.get(list);
  if (!span) {
    if (targetPath || steps.length) throw new Error("无法定位步骤列表，请先展开 YAML 引用");
    const next = { ...ast, spec: { ...ast.spec, steps: inserted } };
    return { source: dumpYaml(next, { noRefs: true, lineWidth: -1 }), insertedCount: inserted.length, ids };
  }
  let start = span.start, end = span.end;
  while (start < end && /\s/.test(source[start])) start++;
  while (end > start && /\s/.test(source[end - 1])) end--;
  const column = start - source.lastIndexOf("\n", start - 1) - 1;
  const inline = source[start] === "[";
  if (!inline) {
    const next = list[index], nextSpan = next && document.spans.get(next);
    let insertion = end;
    if (nextSpan) {
      let nextStart = nextSpan.start;
      while (/\s/.test(source[nextStart] || "")) nextStart++;
      insertion = source.lastIndexOf("\n", nextStart - 1) + 1;
    }
    const rendered = texts.map((text) => text.replace(/\n+$/, "")).join("\n").replace(/\n/g, `\n${" ".repeat(column)}`);
    const addition = nextSpan ? `${" ".repeat(column)}${rendered}\n` : `\n${" ".repeat(column)}${rendered}`;
    const candidate = source.slice(0, insertion) + addition + source.slice(insertion);
    if (matches(candidate)) return { source: candidate, insertedCount: inserted.length, ids };
  }
  const serialized = dumpYaml(expected, { noRefs: true, lineWidth: -1, indent: 2, ...(inline ? { flowLevel: 0 } : {}) }).replace(/\n$/, "");
  const replacement = inline ? serialized : serialized.replace(/\n/g, `\n${" ".repeat(column)}`);
  const candidate = source.slice(0, start) + replacement + source.slice(end);
  if (!matches(candidate)) throw new Error("无法安全粘贴步骤，请先在代码视图展开步骤列表");
  return { source: candidate, insertedCount: inserted.length, ids };
}

/** Delete the blocks of every id in `ids` (top-level or nested). */
export function removeStepBlocks(source, ids) {
  let lines = source.split(/\r?\n/);
  for (const id of ids) {
    const range = findStepRange(lines, id);
    if (!range) continue;
    lines = lines.slice(0, range.startIdx).concat(lines.slice(range.endIdx));
  }
  return lines.join("\n");
}

/** Resolve pathKeys to ids, both returned in document (walk) order. */
export function orderPathKeysByDocument(source, keys) {
  const want = new Set(keys);
  const orderedKeys = [];
  const orderedIds = [];
  walkSteps(extractSteps(parseYaml(source)), (step, info) => {
    const key = pathKey(info.path);
    const ancestorSelected = info.path.some((_part, index) => index > 0 && want.has(pathKey(info.path.slice(0, index))));
    if (want.has(key) && !ancestorSelected) {
      orderedKeys.push(key);
      orderedIds.push(String(step.id));
    }
  });
  return { orderedIds, orderedKeys };
}
