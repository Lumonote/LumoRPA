import { graphRuntimePath } from "./editor/graph-run-state.js";

// Result navigation needs source identity, never the retained progress chain.
export function runResultIdentity(context, runId) {
  if (!context?.runId || context.runId !== runId) return null;
  const { flowPath, source, selectedRuntimePath = null } = context;
  return { runId, flowPath, source, selectedRuntimePath };
}

const instanceIndexes = new WeakMap();
const emptyIndex = new Map();

// Summaries are replaced when a run/history page changes. Store numeric row
// positions rather than duplicating summaries or canonicalizing every click.
export function getRunInstanceIndex(steps, context, ast) {
  if (!Array.isArray(steps) || !context?.runId) return emptyIndex;
  const cached = instanceIndexes.get(steps);
  if (cached && cached.ast === ast && cached.length === steps.length
    && cached.runId === context.runId && cached.flowPath === context.flowPath
    && cached.source === context.source && cached.selectedRuntimePath === (context.selectedRuntimePath ?? null)) return cached.index;
  const index = new Map();
  for (let row = 0; row < steps.length; row++) {
    const path = steps[row]?.path || steps[row]?.stepId;
    if (typeof path !== "string" || !path) continue;
    const canonical = graphRuntimePath(context, path, ast);
    let positions = index.get(canonical);
    if (!positions) { positions = []; index.set(canonical, positions); }
    positions.push(row);
  }
  instanceIndexes.set(steps, { ...runResultIdentity(context, context.runId), ast, length: steps.length, index });
  return index;
}

export function runInstancePage(positions, selectedIndex, limit = 200) {
  limit = Math.max(1, Math.min(200, Number.isInteger(limit) ? limit : 200));
  let low = 0, high = positions.length - 1, position = 0;
  while (low <= high) {
    const middle = Math.floor((low + high) / 2), candidate = positions[middle];
    if (candidate === selectedIndex) { position = middle; break; }
    if (candidate < selectedIndex) low = middle + 1; else high = middle - 1;
  }
  const start = Math.floor(position / limit) * limit;
  return {
    start, total: positions.length, indices: positions.slice(start, start + limit),
    previousIndex: start ? positions[start - limit] : undefined,
    nextIndex: start + limit < positions.length ? positions[start + limit] : undefined,
  };
}
