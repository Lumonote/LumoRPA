// Identity migrations for an atomic node rename. Source paths stay fixed while
// stable canvas identities and runtime paths (including descendants) change.
import { buildGraphModel } from "./graph-model.js";
import { extractSteps, stepIdChain } from "../yaml.js";

export function buildStepIdentityMap(previousAst, nextAst) {
  const previousSteps = extractSteps(previousAst), nextSteps = extractSteps(nextAst);
  const previous = buildGraphModel(previousSteps), next = buildGraphModel(nextSteps);
  const layoutKeys = new Map(), runtimePaths = new Map();
  for (const [key, node] of previous.nodes) {
    const target = next.nodes.get(key);
    if (!target) continue;
    if (node.layoutKey && target.layoutKey && node.layoutKey !== target.layoutKey) layoutKeys.set(node.layoutKey, target.layoutKey);
    for (const [before, after] of [[node.runtimePath, target.runtimePath], [stepIdChain(previousSteps, node.path), stepIdChain(nextSteps, target.path)]]) {
      if (before && after && before !== after) runtimePaths.set(before, after);
    }
  }
  return { layoutKeys, runtimePaths };
}

export function migrateGraphIdentities(layouts = {}, layoutKeys) {
  const next = structuredClone(layouts);
  for (const [before] of layoutKeys) delete next[before];
  for (const [before, after] of layoutKeys) if (Object.hasOwn(layouts, before)) next[after] = structuredClone(layouts[before]);
  const studio = next.__studio;
  if (Array.isArray(studio?.groups)) studio.groups.forEach((group) => {
    if (Array.isArray(group.members)) group.members = group.members.map((key) => layoutKeys.get(key) || key);
  });
  if (studio?.routes && typeof studio.routes === "object") {
    studio.routes = Object.fromEntries(Object.entries(studio.routes).map(([key, value]) => {
      try {
        const parts = JSON.parse(key);
        if (Array.isArray(parts) && (parts.length === 5 || parts.length === 6)) {
          parts[0] = layoutKeys.get(parts[0]) || parts[0]; parts[1] = layoutKeys.get(parts[1]) || parts[1];
          return [JSON.stringify(parts), value];
        }
      } catch { /* Preserve route metadata from unknown versions. */ }
      return [key, value];
    }));
  }
  return next;
}

export function migrateBreakpointIdentities(breakpoints, runtimePaths) {
  const paths = [...runtimePaths].sort((a, b) => b[0].length - a[0].length);
  return new Set([...breakpoints].map((key) => {
    const match = paths.find(([before]) => key === before || key.startsWith(`${before}/`) || key.startsWith(`${before}[`));
    return match ? match[1] + key.slice(match[0].length) : key;
  }));
}
