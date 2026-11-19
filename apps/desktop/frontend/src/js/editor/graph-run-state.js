// Execution colors belong to one exact editor source and run, never to the
// globally selected history record. Keep runtime paths intact while recording:
// branch scopes distinguish same-named children and loop iterations can overlap.
import { childStepBlocks, extractSteps } from "../yaml.js";

const LABELS = { idle: "待执行", running: "执行中", ok: "已完成", failed: "执行失败", skipped: "已跳过", cancelled: "已取消" };
const normalizedState = (value) => ({ ai_healed: "ok", timeout: "failed" })[value]
  || (Object.hasOwn(LABELS, value) ? value : "idle");
const duration = (value) => typeof value === "number" && Number.isFinite(value) && value >= 0 ? value : undefined;
const stepPath = (step) => typeof step?.path === "string" && step.path ? step.path : typeof step?.stepId === "string" ? step.stepId : "";

export const createGraphRunContext = ({ flowPath, source, runId, selectedRuntimePath = null }) => ({ flowPath, source, runId, selectedRuntimePath, stepStates: {} });

export const recordGraphRunProgress = (context, event) => {
  if (!context?.runId || context.terminal || event?.runId !== context.runId) return context;
  if (!["step_started", "step_finished"].includes(event.type)) return context;
  const path = stepPath(event);
  if (!path) return context;
  const entry = { state: event.type === "step_started" ? "running" : normalizedState(event.state) };
  const ms = duration(event.durationMs);
  if (ms !== undefined) entry.durationMs = ms;
  return { ...context, previous: context, stepStates: null, stepDelta: { path, entry } };
};

export const finishGraphRunContext = (context, outcome = "failed") => {
  if (!context?.runId) return context;
  const final = normalizedState(outcome);
  const stepStates = Object.fromEntries(Object.entries(materializeStates(context)).map(([path, entry]) => [
    path, entry.state === "running" ? { ...entry, state: final } : entry,
  ]));
  return { ...context, previous: null, stepDelta: null, stepStates, terminal: outcome };
};

export const applyGraphRunResponse = (context, response) => {
  const runId = response?.report?.runId || response?.run?.id;
  if (!context?.runId || runId !== context.runId || (response?.run?.id && response.run.id !== runId)) return context;
  const stepStates = materializeStates(context);
  for (const step of response.steps || []) {
    const path = stepPath(step);
    if (!path) continue;
    const entry = { state: normalizedState(step.state) };
    const ms = duration(step.durationMs);
    if (ms !== undefined) entry.durationMs = ms;
    stepStates[path] = entry;
  }
  const outcome = response.report?.pausedAt ? "idle"
    : response.run?.state === "cancelled" ? "cancelled"
      : response.report?.success === true ? "ok" : "failed";
  return finishGraphRunContext({ ...context, stepStates, previous: null, stepDelta: null }, outcome);
};

const loopScopeCache = new WeakMap();
const loopScopes = (ast) => {
  if (!ast || typeof ast !== "object") return new Set();
  if (loopScopeCache.has(ast)) return loopScopeCache.get(ast);
  const scopes = new Set();
  const seen = new WeakSet();
  const walk = (steps, prefix) => {
    for (const step of Array.isArray(steps) ? steps : []) {
      if (!step || typeof step !== "object" || seen.has(step)) continue;
      seen.add(step);
      const path = [...prefix, String(step.id || "")];
      if (["control.for", "control.for_each", "control.while"].includes(step.action)) scopes.add(path.join("/"));
      for (const block of childStepBlocks(step)) {
        if (block.kind === "do" && step.action === "control.parallel") {
          block.steps.forEach((child, index) => walk([child], [...path, `branch[${index}]`]));
          continue;
        }
        const scope = block.kind === "branches" ? block.label
          : block.kind === "do" ? step.action === "control.if" ? "do" : step.action === "control.try" ? "try" : null : block.kind;
        walk(block.steps, scope ? [...path, scope] : path);
      }
      seen.delete(step);
    }
  };
  walk(extractSteps(ast), []);
  loopScopeCache.set(ast, scopes);
  return scopes;
};

// Current VMs emit `loop[2]/child`; older records may contain
// `loop/item.2/child`. Only loop ancestors declared in this source may carry
// iteration suffixes. Literal indexed IDs and branch scopes stay untouched.
export const graphRuntimePath = (context, actual, ast) => {
  let path = actual;
  if (context?.selectedRuntimePath) {
    const selected = context.selectedRuntimePath;
    const root = selected.split("/").at(-1);
    if (path === root || path.startsWith(`${root}/`) || path.startsWith(`${root}[`)) path = selected + path.slice(root.length);
  }
  const scopes = loopScopes(ast);
  const result = [];
  for (const part of path.split("/")) {
    if (/^item\.\d+$/.test(part) && scopes.has(result.join("/"))) continue;
    const base = part.replace(/\[\d+\]$/, "");
    result.push(base !== part && scopes.has([...result, base].join("/")) ? base : part);
  }
  return result.join("/");
};

const materializeStates = (context) => {
  const changes = [];
  let cursor = context;
  while (cursor?.stepDelta) { changes.push(cursor.stepDelta); cursor = cursor.previous; }
  const entries = Object.assign(Object.create(null), cursor?.stepStates || {});
  for (let index = changes.length - 1; index >= 0; index--) entries[changes[index].path] = changes[index].entry;
  return entries;
};

const indexes = new WeakMap();
const emptyIndex = new Map();
const idleState = Object.freeze({ state: "idle", label: LABELS.idle });

// One pass per context/AST, shared by every node in a frame. Runtime paths are
// indexed once instead of scanning every recorded iteration for every card.
export const getGraphRunIndex = (editorState) => {
  const context = editorState?.editorRunContext;
  if (!context?.runId || context.flowPath !== editorState.flowPath || context.source !== editorState.source) return emptyIndex;
  const cached = indexes.get(context);
  if (cached && cached.ast === editorState.ast) return cached.index;
  // Transfer the private projection from the nearest already-rendered context.
  // Context deltas remain immutable; re-reading an old context rebuilds it.
  const changes = [];
  let cursor = context;
  let inherited;
  while (cursor?.stepDelta) {
    changes.push(cursor.stepDelta);
    cursor = cursor.previous;
    const candidate = indexes.get(cursor);
    if (candidate && candidate.ast === editorState.ast) { inherited = candidate; indexes.delete(cursor); break; }
  }
  const index = inherited?.index || new Map();
  const apply = (path, entry) => {
    const canonical = graphRuntimePath(context, path, editorState.ast);
    let aggregate = index.get(canonical);
    if (!aggregate) {
      const values = new Map();
      aggregate = { state: "idle", label: LABELS.idle, values, counts: {}, durationCount: 0, durationTotal: 0,
        get instances() { return [...values].map(([runtimePath, value]) => ({ ...value, path: runtimePath })); },
      };
      index.set(canonical, aggregate);
    }
    const previous = aggregate.values.get(path);
    if (previous) {
      aggregate.counts[previous.state]--;
      if (duration(previous.durationMs) !== undefined) { aggregate.durationCount--; aggregate.durationTotal -= previous.durationMs; }
    }
    aggregate.values.set(path, entry);
    aggregate.counts[entry.state] = (aggregate.counts[entry.state] || 0) + 1;
    if (duration(entry.durationMs) !== undefined) { aggregate.durationCount++; aggregate.durationTotal += entry.durationMs; }
    aggregate.state = ["failed", "cancelled", "running", "ok", "skipped"].find((value) => aggregate.counts[value] > 0) || "idle";
    aggregate.label = LABELS[aggregate.state];
    if (aggregate.durationCount) aggregate.durationMs = aggregate.durationTotal;
    else delete aggregate.durationMs;
  };
  if (!inherited) for (const [path, entry] of Object.entries(cursor?.stepStates || {})) apply(path, entry);
  for (let item = changes.length - 1; item >= 0; item--) apply(changes[item].path, changes[item].entry);
  indexes.set(context, { ast: editorState.ast, index });
  return index;
};

export const getGraphRunState = (node, editorState, index = getGraphRunIndex(editorState)) => {
  const path = node?.runtimePath || node?.idChain;
  const aggregate = index.get(path);
  if (!aggregate) return idleState;
  const result = { state: aggregate.state, label: aggregate.label };
  if (aggregate.durationMs !== undefined) result.durationMs = aggregate.durationMs;
  return result;
};
