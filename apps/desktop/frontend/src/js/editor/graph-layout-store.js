import { call, invoke } from "../api.js";
import { state } from "../state.js";
import { toast } from "../dom.js";
import { normalizeGraphLayouts } from "./graph-presentation.js";

const queues = new Map();
const scheduled = new Map();
const writable = new Set();
const localKey = (path) => `lumo.graphLayout.v1:${path}`;

export async function loadGraphLayout(path) {
  // Flush this document's preceding writes before reading its sidecar.
  await flushScheduledGraphLayouts(path);
  await queues.get(path)?.catch(() => {});
  let cached = state.graphLayouts?.[path] || {};
  let locallyUnsaved = false;
  try { const local = JSON.parse(localStorage.getItem(localKey(path)) || "null"); if (local) { cached = local.version === 1 && local.positions ? local.positions : local; locallyUnsaved = localStorage.getItem(`${localKey(path)}:pending`) === "true"; } } catch { /* Migrate legacy local cache. */ }
  if (invoke) {
    try {
      const response = await call("graph_layout_read", { path });
      if (response?.writable) writable.add(path); else writable.delete(path);
      if (response?.layout?.version === 1 && !locallyUnsaved) cached = response.layout.positions;
    } catch (error) { toast("布局读取失败", `已保留本机缓存：${String(error)}`, "warn"); }
  }
  return normalizeGraphLayouts(cached);
}

export function persistGraphLayout(path, positions) {
  clearTimeout(scheduled.get(path));
  scheduled.delete(path);
  // Capture now: a flow switch or another edit must not change an in-flight save.
  const layout = { version: 1, positions: structuredClone(positions || {}) };
  try { localStorage.setItem(localKey(path), JSON.stringify(layout.positions)); if (writable.has(path)) localStorage.setItem(`${localKey(path)}:pending`, "true"); }
  catch (error) { toast("布局缓存未保存", String(error), "warn"); }
  if (!path || !writable.has(path) || !invoke) return Promise.resolve();
  const save = (queues.get(path) || Promise.resolve()).catch(() => {}).then(() => call("graph_layout_write", { path, layout }));
  queues.set(path, save);
  save.then(() => { if (queues.get(path) === save) { try { localStorage.removeItem(`${localKey(path)}:pending`); } catch { /* The sidecar is already saved. */ } } }).catch(() => {});
  save.catch((error) => toast("布局文件未保存", `本机缓存仍保留；请检查目录权限：${String(error)}`, "bad")).finally(() => { if (queues.get(path) === save) queues.delete(path); });
  return save;
}

export function scheduleGraphLayoutSave(path) {
  if (!path) return;
  clearTimeout(scheduled.get(path));
  scheduled.set(path, setTimeout(() => flushScheduledGraphLayouts(path), 350));
}

export async function flushScheduledGraphLayouts(path) {
  const paths = path === undefined ? [...scheduled.keys()] : [path];
  const saves = [];
  for (const key of paths) {
    if (!scheduled.has(key)) continue;
    // History can replace the document object before this timer runs.
    saves.push(persistGraphLayout(key, state.graphLayouts?.[key] || {}));
  }
  await Promise.allSettled(saves);
}

export async function copyGraphLayout(from, to) {
  const layout = structuredClone(state.graphLayouts?.[from] || {});
  state.graphLayouts[to] = layout;
  if (invoke) {
    try { const result = await call("graph_layout_read", { path: to }); if (result?.writable) writable.add(to); }
    catch (error) { toast("布局迁移失败", String(error), "warn"); }
  }
  await persistGraphLayout(to, layout).catch(() => {});
}
