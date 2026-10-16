// YAML parser + textual mutation helpers. The Code view remains the source of
// truth; js-yaml gives the editor the same YAML semantics as the Rust runtime
// (quoted strings, block scalars, nested branches and flow-style collections).
import { load as loadYaml } from "./vendor/js-yaml.mjs";

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

export function walkSteps(steps, cb, parent = null, depth = 0) {
  steps.forEach((step, idx) => {
    const path = parent ? [...parent, idx] : [idx];
    cb(step, { parent, idx, depth, path });
    childStepBlocks(step).forEach((block) => {
      walkSteps(block.steps, cb, [...path, ...block.path], depth + 1);
    });
  });
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

export function emitStep(step, baseIndent) {
  const pad = " ".repeat(baseIndent);
  const cIndent = " ".repeat(baseIndent + 2);
  let out = `${pad}- id: ${yamlScalar(step.id)}\n${cIndent}action: ${yamlScalar(step.action)}\n`;
  if (step.when !== undefined && step.when !== null && step.when !== "") {
    out += emitYamlField("when", step.when, baseIndent + 2);
  }
  if (step.bind) out += emitYamlField("bind", step.bind, baseIndent + 2);
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
  if (oe && Object.keys(oe).length && (oe.mode !== "interrupt" || oe.output !== undefined)) {
    out += `${cIndent}on_error:\n`;
    if (oe.mode) out += emitYamlField("mode", oe.mode, baseIndent + 4);
    if (oe.retry_count) out += emitYamlField("retry_count", oe.retry_count, baseIndent + 4);
    if (oe.retry_interval_ms) {
      out += emitYamlField("retry_interval_ms", oe.retry_interval_ms, baseIndent + 4);
    }
    if (oe.output !== undefined) out += emitYamlField("output", oe.output, baseIndent + 4);
  }
  if (step.ai && (step.ai.mode || step.ai.model || step.ai.prompt)) {
    out += `${cIndent}ai:\n`;
    if (step.ai.mode) out += emitYamlField("mode", step.ai.mode, baseIndent + 4);
    if (step.ai.model) out += emitYamlField("model", step.ai.model, baseIndent + 4);
    if (step.ai.prompt) out += emitYamlField("prompt", step.ai.prompt, baseIndent + 4);
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
  if (/^[\w.\-/]+$/.test(s)) return s;
  return JSON.stringify(s);
}

export function yamlInline(v) {
  if (v === null || v === undefined) return "~";
  if (typeof v === "string") {
    if (v.includes("\n")) return `|\n      ${v.split("\n").join("\n      ")}`;
    if (/[:#{}[\]&*!|<>='"%@`,]/.test(v) || /^\s|\s$/.test(v)) return JSON.stringify(v);
    return v;
  }
  if (typeof v === "boolean" || typeof v === "number") return String(v);
  return JSON.stringify(v);
}

// ─── Step-block clipboard helpers (copy / duplicate / paste / batch delete) ──
// All operate on the YAML source text so comments inside untouched regions
// survive, mirroring the mutation helpers above.

function escapeRegExp(s) {
  return String(s).replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
}

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
  if (!range) return null;
  const text = lines
    .slice(range.startIdx, range.endIdx)
    .map((l) => (l.trim() === "" ? "" : l.slice(range.baseIndent)))
    .join("\n")
    .replace(/\n*$/, "\n");
  return { text, baseIndent: range.baseIndent };
}

/**
 * Rename ids in a dedented block that collide with `existingIds` (nested ids
 * too), and rewrite `steps.<oldId>` template references inside the block so a
 * pasted copy keeps referring to its own steps. Returns { text, renames }.
 */
export function uniquifyStepIds(blockText, existingIds) {
  const taken = new Set(existingIds);
  const renames = new Map();
  const lines = blockText.split("\n").map((line) => {
    const m = line.match(/^(\s*-\s+id:\s*)("([^"]*)"|'([^']*)'|[^#]*?)(\s*(?:#.*)?)$/);
    if (!m) return line;
    const rawId = (m[3] ?? m[4] ?? m[2]).trim();
    if (!rawId) return line;
    if (!taken.has(rawId)) {
      taken.add(rawId);
      return line;
    }
    const base = rawId.replace(/[^\w.-]+/g, "_").replace(/^_+|_+$/g, "") || "step";
    let n = 1;
    while (taken.has(`${base}_c${n}`)) n++;
    const newId = `${base}_c${n}`;
    taken.add(newId);
    renames.set(rawId, newId);
    return `${m[1]}${yamlScalar(newId)}${m[5] || ""}`;
  });
  let text = lines.join("\n");
  for (const [oldId, newId] of renames) {
    text = text.replace(new RegExp(`\\bsteps\\.${escapeRegExp(oldId)}\\b`, "g"), `steps.${newId}`);
  }
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

function topStepIdOfBlock(blockText) {
  const m = blockText.match(/^\s*-\s+id:\s*("([^"]*)"|'([^']*)'|\S+)/);
  if (!m) return null;
  return (m[2] ?? m[3] ?? m[1]).trim();
}

/**
 * Paste `blocks` (dedented step blocks) in order after `targetId`; a null
 * target appends after the last top-level step. Colliding ids are renamed
 * before each insert.
 */
export function insertStepBlocks(source, targetId, blocks) {
  let out = source;
  let anchor = targetId;
  if (anchor == null) {
    const steps = extractSteps(parseYaml(out));
    anchor = steps.length ? String(steps[steps.length - 1].id) : null;
  }
  for (const block of blocks) {
    const { text } = uniquifyStepIds(block, collectStepIds(out));
    const topId = topStepIdOfBlock(text);
    if (!topId) continue;
    let next = anchor == null ? null : insertStepBlockAfter(out, anchor, text);
    if (next == null) {
      const pad = " ".repeat(4);
      const blockLines = text
        .replace(/\n+$/, "")
        .split("\n")
        .map((l) => (l === "" ? "" : pad + l));
      next = out.replace(/\n*$/, "\n") + blockLines.join("\n") + "\n";
    }
    out = next;
    anchor = topId;
  }
  return out;
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
    if (want.has(key)) {
      orderedKeys.push(key);
      orderedIds.push(String(step.id));
    }
  });
  return { orderedIds, orderedKeys };
}
