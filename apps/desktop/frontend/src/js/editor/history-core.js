// Pure undo/redo snapshot stacks — no DOM, no app state. The editor wiring
// (history.js) feeds it opaque snapshots; consecutive pushes carrying the same
// label inside the coalesce window collapse into the earliest snapshot so a
// typing burst in the code view becomes a single undo entry.

export function createHistoryCore({ cap = 50, now = () => Date.now() } = {}) {
  const undoStack = [];
  const redoStack = [];
  let lastLabel = null;
  let lastPushAt = -Infinity;

  return {
    push(snapshot, { label = null, coalesceMs = 0 } = {}) {
      const t = now();
      const coalesce =
        label !== null &&
        coalesceMs > 0 &&
        label === lastLabel &&
        t - lastPushAt <= coalesceMs &&
        undoStack.length > 0 &&
        redoStack.length === 0;
      lastLabel = label;
      lastPushAt = t;
      redoStack.length = 0;
      if (coalesce) return;
      undoStack.push(snapshot);
      if (undoStack.length > cap) undoStack.shift();
    },
    undo(current) {
      if (!undoStack.length) return null;
      const snap = undoStack.pop();
      redoStack.push(current);
      lastLabel = null;
      return snap;
    },
    redo(current) {
      if (!redoStack.length) return null;
      const snap = redoStack.pop();
      undoStack.push(current);
      lastLabel = null;
      return snap;
    },
    canUndo: () => undoStack.length > 0,
    canRedo: () => redoStack.length > 0,
    size: () => undoStack.length,
    reset() {
      undoStack.length = 0;
      redoStack.length = 0;
      lastLabel = null;
      lastPushAt = -Infinity;
    },
  };
}
