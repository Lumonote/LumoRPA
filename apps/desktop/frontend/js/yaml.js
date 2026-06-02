// "Good-enough" LumoFlow YAML parser + textual mutation helpers. The Code view
// is the source of truth; this lets the Graph/Tree/Steps views render and the
// inspector/editor patch steps in place while preserving comments.

export function parseYaml(text) {
  const lines = text.split(/\r?\n/);
  const root = {};
  const stack = [{ indent: -1, value: root, kind: "map" }];
  let i = 0;

  function isBlank(line) { return /^\s*(#|$)/.test(line); }
  function indentOf(line) {
    const m = line.match(/^( *)/);
    return m ? m[1].length : 0;
  }

  while (i < lines.length) {
    const raw = lines[i];
    if (isBlank(raw)) { i++; continue; }
    const ind = indentOf(raw);
    const line = raw.slice(ind);

    while (stack.length > 1 && ind <= stack[stack.length - 1].indent) {
      stack.pop();
    }
    const top = stack[stack.length - 1];

    if (line.startsWith("- ") || line === "-") {
      // List entry. Top must be a list.
      if (top.kind !== "list") {
        // Promote: parent key should hold a list. The previous map entry that
        // pointed here was treated as scalar; rewrite to list.
        // (This happens when key: appears with the list children on next lines.)
        // We assume top is now expecting a list.
        top.kind = "list";
        top.value = [];
        // Re-attach to parent key
        const parent = stack[stack.length - 2];
        if (parent && parent.lastKey) {
          if (parent.kind === "map") parent.value[parent.lastKey] = top.value;
        }
      }
      const after = line.slice(2);
      if (after === "" || /^\s*$/.test(after)) {
        // Empty item: object whose keys come at deeper indent
        const obj = {};
        top.value.push(obj);
        stack.push({ indent: ind, value: obj, kind: "map", lastKey: null });
        i++;
        continue;
      }
      if (after.includes(":") && !after.startsWith("{")) {
        // Inline first key like "- id: foo"
        const obj = {};
        top.value.push(obj);
        const child = { indent: ind, value: obj, kind: "map", lastKey: null };
        stack.push(child);
        // Re-process current line as a map entry at +2 indent
        const fakeLine = " ".repeat(ind + 2) + after;
        lines[i] = fakeLine;
        continue;
      }
      // Scalar list item
      top.value.push(parseScalar(after));
      i++;
      continue;
    }

    // map entry
    const colon = line.indexOf(":");
    if (colon < 0) { i++; continue; }
    const key = line.slice(0, colon).trim();
    const restRaw = line.slice(colon + 1);
    const rest = restRaw.replace(/\s+#.*$/, "").trim();

    if (top.kind !== "map") {
      // promote (rare); skip safety.
      i++; continue;
    }

    if (rest === "" || rest === "|" || rest === ">") {
      if (rest === "|" || rest === ">") {
        // Block scalar — collect until indent <= current.
        const blockLines = [];
        i++;
        const blockIndent = ind + 2;
        while (i < lines.length) {
          const bRaw = lines[i];
          if (bRaw.trim() === "") { blockLines.push(""); i++; continue; }
          if (indentOf(bRaw) < blockIndent) break;
          blockLines.push(bRaw.slice(blockIndent));
          i++;
        }
        top.value[key] = rest === "|" ? blockLines.join("\n") : blockLines.join(" ").trim();
        top.lastKey = key;
        continue;
      }
      // Empty: child container coming.
      const placeholder = {};
      top.value[key] = placeholder;
      top.lastKey = key;
      stack.push({ indent: ind, value: placeholder, kind: "map", lastKey: null });
      i++;
      continue;
    }
    // Inline scalar / flow-style
    top.value[key] = parseScalar(rest);
    top.lastKey = key;
    i++;
  }

  return root;
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

export function walkSteps(steps, cb, parent = null, depth = 0) {
  steps.forEach((step, idx) => {
    cb(step, { parent, idx, depth, path: parent ? [...parent, idx] : [idx] });
    ["do", "else", "catch", "finally"].forEach((kind) => {
      if (Array.isArray(step[kind])) {
        walkSteps(step[kind], cb, parent ? [...parent, idx, kind] : [idx, kind], depth + 1);
      }
    });
  });
}

export function findStepByPath(steps, path) {
  let cur = steps;
  let parent = null;
  for (let i = 0; i < path.length; i++) {
    const seg = path[i];
    if (typeof seg === "number") {
      parent = cur[seg];
      if (i === path.length - 1) return parent;
      cur = parent;
    } else {
      cur = parent[seg] || [];
    }
  }
  return parent;
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

export function findStepRange(lines, originalId) {
  const escaped = String(originalId).replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
  const re = new RegExp(`^(\\s*)-\\s*id:\\s*${escaped}\\b`);
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
  const escapeId = originalId.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
  const re = new RegExp(`^(\\s*)-\\s*id:\\s*${escapeId}\\b`);
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
    out += `${cIndent}when: ${yamlInline(step.when)}\n`;
  }
  if (step.bind) out += `${cIndent}bind: ${yamlScalar(step.bind)}\n`;
  if (step.with && Object.keys(step.with).length) {
    out += `${cIndent}with:\n`;
    for (const [k, v] of Object.entries(step.with)) {
      out += `${cIndent}  ${k}: ${yamlInline(v)}\n`;
    }
  }
  if (step.retry && Object.keys(step.retry).length) {
    out += `${cIndent}retry:\n`;
    for (const [k, v] of Object.entries(step.retry)) {
      out += `${cIndent}  ${k}: ${yamlInline(v)}\n`;
    }
  }
  if (step.ai && (step.ai.mode || step.ai.model || step.ai.prompt)) {
    out += `${cIndent}ai:\n`;
    if (step.ai.mode) out += `${cIndent}  mode: ${step.ai.mode}\n`;
    if (step.ai.model) out += `${cIndent}  model: ${yamlScalar(step.ai.model)}\n`;
    if (step.ai.prompt) out += `${cIndent}  prompt: ${yamlInline(step.ai.prompt)}\n`;
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
  return out.replace(/\n$/, "");
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
