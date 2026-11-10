// Execution colors belong to one exact editor source and run, never to the
// globally selected history record. Keep runtime paths intact while recording:
// branch scopes distinguish same-named children and loop iterations can overlap.
import { childStepBlocks, extractSteps } from "../yaml.js";

const LABELS = { idle: "待执行", running: "执行中", ok: "已完成", failed: "执行失败", skipped: "已跳过", cancelled: "已取消" };
const normalizedState = (value) => ({ ai_healed: "ok", timeout: "failed" })[value]
  || (Object.hasOwn(LABELS, value) ? value : "idle");
const duration = (value) => typeof value === "number" && Number.isFinite(value) && value >= 0 ? value : undefined;
const stepPath = (step) => typeof step?.path === "string" && step.path ? step.path : typeof step?.stepId === "string" ? step.stepId : "";

export const createGraphRunContext = ({ flowPath, source, runId }) => ({ flowPath, source, runId, stepStates: {} });

export const recordGraphRunProgress = (context, event) => {
  if (!context?.runId || context.terminal || event?.runId !== context.runId) return context;
  if (!["step_started", "step_finished"].includes(event.type)) return context;
  const path = stepPath(event);
  if (!path) return context;
  const entry = { state: event.type === "step_started" ? "running" : normalizedState(event.state) };
  const ms = duration(event.durationMs);
  if (ms !== undefined) entry.durationMs = ms;
  return { ...context, stepStates: { ...context.stepStates, [path]: entry } };
};

export const finishGraphRunContext = (context, outcome = "failed") => {
  if (!context?.runId) return context;
  const final = normalizedState(outcome);
  const stepStates = Object.fromEntries(Object.entries(context.stepStates || {}).map(([path, entry]) => [
    path, entry.state === "running" ? { ...entry, state: final } : entry,
  ]));
  return { ...context, stepStates, terminal: outcome };
};

export const applyGraphRunResponse = (context, response) => {
  const runId = response?.report?.runId || response?.run?.id;
  if (!context?.runId || runId !== context.runId || (response?.run?.id && response.run.id !== runId)) return context;
  const stepStates = { ...context.stepStates };
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
  return finishGraphRunContext({ ...context, stepStates }, outcome);
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
const matchesRuntimePath = (expected, actual, scopes) => {
  if (expected === actual) return true;
  const target = expected.split("/");
  const parts = actual.split("/");
  let cursor = 0;
  for (let index = 0; index < target.length; index += 1) {
    const part = target[index];
    const indexed = index < target.length - 1 && scopes.has(target.slice(0, index + 1).join("/"));
    const candidate = parts[cursor++];
    if (part !== candidate && !(indexed && candidate?.startsWith(`${part}[`) && /^\[\d+\]$/.test(candidate.slice(part.length)))) return false;
    if (indexed && /^item\.\d+$/.test(parts[cursor] || "")) cursor += 1;
  }
  return cursor === parts.length;
};

export const getGraphRunState = (node, editorState) => {
  const context = editorState?.editorRunContext;
  const path = node?.runtimePath || node?.idChain;
  if (!context?.runId || !path || context.flowPath !== editorState.flowPath || context.source !== editorState.source) {
    return { state: "idle", label: LABELS.idle };
  }
  const scopes = loopScopes(editorState.ast);
  const entries = Object.entries(context.stepStates || {})
    .filter(([runtimePath]) => matchesRuntimePath(path, runtimePath, scopes)).map(([, entry]) => entry);
  const status = ["failed", "cancelled", "running", "ok", "skipped"].find((value) => entries.some((entry) => entry.state === value)) || "idle";
  const result = { state: status, label: LABELS[status] };
  const durations = entries.map((entry) => duration(entry.durationMs)).filter((ms) => ms !== undefined);
  if (durations.length) result.durationMs = durations.reduce((sum, ms) => sum + ms, 0);
  return result;
};
