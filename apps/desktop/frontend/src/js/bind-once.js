// Bind-once guard for listeners attached to LONG-LIVED elements from render
// functions. Nodes recreated by an innerHTML re-render don't need this — their
// listeners die with the node. Persistent containers (e.g. #stepList) DO:
// without the guard every re-render stacks another copy of the same listener,
// so one drop event would fire N accumulated handlers.

const bound = new WeakMap();

export function bindOnce(el, key, attach) {
  if (!el) return false;
  let keys = bound.get(el);
  if (!keys) {
    keys = new Set();
    bound.set(el, keys);
  }
  if (keys.has(key)) return false;
  keys.add(key);
  attach(el);
  return true;
}
