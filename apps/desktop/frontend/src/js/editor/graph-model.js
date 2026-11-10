// Pure canvas projection of the YAML execution tree. Layout never changes the
// program: sequence/branch edges describe control flow; data edges describe
// existing {{ steps.ID.result }} expressions and do not invent dependencies.
import { childStepBlocks, pathKey } from "../yaml.js";

export const GRAPH_NODE_WIDTH = 260;
export const GRAPH_COLUMN_GAP = 96;
export const GRAPH_ROW_GAP = 72;
export const GRAPH_NODE_HEAD_HEIGHT = 72;
export const GRAPH_INPUT_ROW_HEIGHT = 28;
export const GRAPH_NODE_FOOT_HEIGHT = 42;

const validPosition = (value) => Number.isFinite(value?.x) && Number.isFinite(value?.y);
const fieldsOf = (step) => step?.with && typeof step.with === "object" && !Array.isArray(step.with)
  ? Object.entries(step.with) : [];
const inputType = (value) => value === undefined ? "any" : value === null ? "null" : Array.isArray(value) ? "array" : typeof value;
const inputsOf = (step, schemas) => {
  const schema = schemas?.get?.(step?.action);
  const properties = schema?.properties && typeof schema.properties === "object" && !Array.isArray(schema.properties) ? schema.properties : {};
  const fields = new Map(fieldsOf(step));
  for (const name of Object.keys(properties)) if (!fields.has(name)) fields.set(name, undefined);
  const required = new Set(Array.isArray(schema?.required) ? schema.required : []);
  return [...fields].map(([name, value], row) => ({
    name, value, row,
    type: properties[name]?.type || inputType(value),
    required: required.has(name),
  }));
};
const nodeHeight = (inputs, branches = []) => GRAPH_NODE_HEAD_HEIGHT + (Math.max(inputs.length, 1) + branches.length) * GRAPH_INPUT_ROW_HEIGHT + GRAPH_NODE_FOOT_HEIGHT;
const stableId = (step) => typeof step?.id === "string" && step.id.trim() ? step.id : null;
const branchLabel = (block) => block.parallelIndex !== undefined ? `并行分支 ${block.parallelIndex + 1}` : ({ do: "执行", else: "否则", catch: "捕获异常", finally: "最终执行" })[block.kind]
  || `并行分支 ${block.path[1] + 1}`;
const blocksOf = (step) => {
  if (step?.action !== "control.parallel") return childStepBlocks(step);
  if (Array.isArray(step.branches)) return childStepBlocks(step).filter((block) => block.kind === "branches");
  if (!Array.isArray(step.do)) return [];
  const counts = new Map();
  for (const child of step.do) counts.set(stableId(child), (counts.get(stableId(child)) || 0) + 1);
  return step.do.map((child, index) => ({ kind: "do", path: ["do"], steps: [child], parallelIndex: index,
    indexOffset: index, ambiguous: counts.get(stableId(child)) > 1 }));
};
const runtimeScope = (step, block) => {
  if (step.action === "control.parallel") return `branch[${block.parallelIndex ?? block.path[1]}]`;
  if (["control.for", "control.for_each", "control.while"].includes(step.action) && block.kind === "do") return null;
  if (step.action === "control.try" && block.kind === "do") return "try";
  return block.kind === "branches" ? `branch[${block.path[1]}]` : block.kind;
};

export const graphPortPoint = (node, { direction = "input", kind = "execution", input, branchIndex = 0 } = {}) => {
  const x = node.x + (direction === "output" ? node.w : 0);
  if (kind === "data" && direction === "output") return { x, y: node.y + node.h - GRAPH_NODE_FOOT_HEIGHT / 2 };
  if (kind === "data") {
    const row = node.inputs.find((field) => field.name === input)?.row ?? 0;
    return { x, y: node.y + GRAPH_NODE_HEAD_HEIGHT + GRAPH_INPUT_ROW_HEIGHT / 2 + row * GRAPH_INPUT_ROW_HEIGHT };
  }
  if (kind === "branch") return { x, y: node.y + GRAPH_NODE_HEAD_HEIGHT + (Math.max(node.inputs.length, 1) + branchIndex) * GRAPH_INPUT_ROW_HEIGHT + GRAPH_INPUT_ROW_HEIGHT / 2 };
  return { x, y: node.y + 58 };
};

// Preserve nested paths so the inspector can explain which expression feeds a
// top-level input port, including objects and array entries.
const expressionsIn = (value, path, seen = new Set()) => {
  if (typeof value === "string") return [{ value, path }];
  if (!value || typeof value !== "object" || seen.has(value)) return [];
  seen.add(value);
  const fields = Array.isArray(value) ? value.map((item, index) => [index, item]) : Object.entries(value);
  const expressions = fields.flatMap(([key, item]) => expressionsIn(item,
    typeof key === "number" ? `${path}[${key}]` : `${path}.${key}`, seen));
  seen.delete(value);
  return expressions;
};

const resultReferences = (text) => {
  const ids = new Set();
  for (const match of text.matchAll(/\{\{([\s\S]*?)\}\}/g)) {
    const expression = match[1];
    const pattern = /(?<![\w.])steps\s*(?:\.\s*([A-Za-z_][A-Za-z0-9_]*)|\[\s*["']([^"']+)["']\s*\])\s*(?:\.\s*result\b|\[\s*["']result["']\s*\])/g;
    for (const reference of expression.matchAll(pattern)) {
      // A template may print a literal like "steps.read.result". Such text is
      // not a dependency, even though it happens to resemble a result access.
      let quote = null;
      for (let index = 0; index < reference.index; index++) {
        const character = expression[index];
        if (quote && character === "\\") { index++; continue; }
        if (quote === character) quote = null;
        else if (!quote && (character === '"' || character === "'")) quote = character;
      }
      if (!quote) ids.add(reference[1] || reference[2]);
    }
  }
  return [...ids];
};

const boundsOf = (nodes) => {
  const all = [...nodes.values()];
  if (!all.length) return { x: 0, y: 0, width: 0, height: 0 };
  const x = Math.min(...all.map((node) => node.x));
  const y = Math.min(...all.map((node) => node.y));
  return {
    x, y,
    width: Math.max(...all.map((node) => node.x + node.w)) - x,
    height: Math.max(...all.map((node) => node.y + node.h)) - y,
  };
};

/**
 * buildGraphModel([{ id: "read", action: "file.read", with: { path: "a.txt" } }])
 * => { nodes: Map("0" => { layoutKey: "id:read", inputs: [{name:"path", row:0, ...}], ... }),
 *      edges: [], bounds: {x:0, y:0, width:260, height:142} }
 *
 * Optional schemas is a Map(action => JSON Schema). Existing unknown input keys
 * remain visible; absent schema properties become ports without editing source.
 * Stable layout keys include ancestor IDs and branch kinds. Missing or repeated
 * sibling IDs have layoutKey=null (as do their descendants), so index-keyed or
 * ambiguous saved positions can never migrate to a different step after edits.
 */
export const buildGraphModel = (steps, { layouts = {}, schemas = new Map() } = {}) => {
  const nodes = new Map();
  const edges = [];
  const byId = new Map();
  const addEdge = (edge) => edges.push({ id: `${edge.kind}:${edge.from}>${edge.to}${edge.input ? `:${edge.input}:${edge.inputPath}` : ""}`, ...edge });

  const layoutList = (list, parentPath, prefix, idChain, runtimeChain, x, y, indexOffset = 0, ancestors = new Set()) => {
    const safeList = Array.isArray(list) ? list : [];
    const counts = new Map();
    for (const step of safeList) {
      const id = stableId(step);
      if (id) counts.set(id, (counts.get(id) || 0) + 1);
    }
    const maxHeight = Math.max(0, ...safeList.map((step) => nodeHeight(inputsOf(step, schemas), ancestors.has(step) ? [] : blocksOf(step))));
    let cursorX = x;
    let bottom = y + maxHeight;
    let previous = null;
    let firstKey = null;
    safeList.forEach((step, index) => {
      const path = [...parentPath, index + indexOffset];
      const key = pathKey(path);
      const id = stableId(step);
      const recursiveReference = ancestors.has(step);
      const layoutKey = !recursiveReference && prefix !== null && id && counts.get(id) === 1 ? `${prefix}id:${encodeURIComponent(id)}` : null;
      const inputs = inputsOf(step, schemas);
      // YAML aliases may point to an ancestor. Render the reference once but
      // do not expand it again; aliases reused in other branches still expand.
      const blocks = recursiveReference ? [] : blocksOf(step);
      const nodeRuntimeChain = [...runtimeChain, id || ""];
      const node = { key, path, layoutKey, step, x: cursorX, y, w: GRAPH_NODE_WIDTH, h: nodeHeight(inputs, blocks), inputs, output: "result", branches: [], recursiveReference, idChain: [...idChain, id || ""].join("/"), runtimePath: nodeRuntimeChain.join("/") };
      nodes.set(key, node);
      if (id) byId.set(id, [...(byId.get(id) || []), node]);
      firstKey ??= key;
      if (previous) addEdge({ from: previous.key, to: key, kind: "sequence" });
      previous = node;
      let branchY = y + maxHeight + GRAPH_ROW_GAP;
      let right = cursorX + GRAPH_NODE_WIDTH;
      for (const [branchIndex, block] of blocks.entries()) {
        const branchName = block.kind === "branches" ? `branches:${block.path[1]}` : block.kind;
        const branchIds = block.kind === "branches" ? [...idChain, id || "", block.label] : [...idChain, id || ""];
        const scope = runtimeScope(step, block);
        const child = layoutList(block.steps, [...path, ...block.path], layoutKey && !block.ambiguous ? `${layoutKey}/${branchName}/` : null,
          branchIds, scope ? [...nodeRuntimeChain, scope] : nodeRuntimeChain, cursorX + GRAPH_COLUMN_GAP, branchY, block.indexOffset || 0, new Set([...ancestors, step]));
        const label = branchLabel(block);
        node.branches.push({ kind: block.kind, path: [...path, ...block.path], firstKey: child.firstKey, label, row: branchIndex });
        if (child.firstKey) addEdge({ from: key, to: child.firstKey, kind: "branch", label, branchIndex });
        branchY = child.bottom + GRAPH_ROW_GAP;
        bottom = Math.max(bottom, child.bottom);
        right = Math.max(right, child.right);
      }
      cursorX = right + GRAPH_COLUMN_GAP;
    });
    return { firstKey, right: safeList.length ? cursorX - GRAPH_COLUMN_GAP : x, bottom };
  };
  layoutList(steps, [], "", [], [], 0, 0);

  for (const node of nodes.values()) {
    const position = node.layoutKey && Object.hasOwn(layouts || {}, node.layoutKey) ? layouts[node.layoutKey] : null;
    if (validPosition(position)) { node.x = position.x; node.y = position.y; }
    for (const input of node.inputs) {
      const seenEdges = new Set();
      for (const expression of expressionsIn(input.value, input.name)) {
        for (const id of resultReferences(expression.value)) {
          const candidates = byId.get(id) || [];
          if (candidates.length !== 1 || candidates[0] === node) continue;
          const from = candidates[0].key;
          const identity = `${from}:${expression.path}`;
          if (seenEdges.has(identity)) continue;
          seenEdges.add(identity);
          addEdge({ from, to: node.key, kind: "data", input: input.name, inputPath: expression.path, label: `${id}.result → ${expression.path}` });
        }
      }
    }
  }
  return { nodes, edges, bounds: boundsOf(nodes) };
};
