// Recovery writes never replace runnable flow files. Every editor source
// (code, visual edits, undo, recorder and history) shares this persistence queue.
export function createDraftController({ call, getSnapshot, onError = () => {} }) {
  let enabled = false, scheduled = false, tail = Promise.resolve();
  const submitted = new Map();
  const key = (path) => path || null;
  const signature = ({ source, savedSource }) => JSON.stringify([source, savedSource]);
  const enqueue = (operation) => {
    tail = tail.catch(() => {}).then(operation);
    tail.catch(onError);
    return tail;
  };
  function captureNow() {
    if (!enabled) return;
    const snapshot = { ...getSnapshot() };
    const path = key(snapshot.path);
    if (!path && !snapshot.source) return;
    const next = signature(snapshot);
    if (submitted.get(path) === next) return;
    const dirty = snapshot.source !== snapshot.savedSource;
    // An initial clean load does not acknowledge any existing recovery data.
    // In particular, a failed list() must never turn default flow loading into
    // deletion of drafts that the operator has not been able to review.
    if (!dirty && !submitted.has(path)) return;
    submitted.set(path, next);
    enqueue(async () => {
      try {
        if (dirty) await call("editor_draft_upsert", { draft: { version: 1, path, source: snapshot.source, baseSource: snapshot.savedSource, updatedAt: Date.now() } });
        else await call("editor_draft_delete", { path });
      } catch (error) {
        if (submitted.get(path) === next) submitted.delete(path);
        throw error;
      }
    });
  }
  return {
    start() { enabled = true; },
    capture() {
      if (!enabled || scheduled) return;
      scheduled = true;
      queueMicrotask(() => { scheduled = false; captureNow(); });
    },
    async flush() {
      for (;;) {
        captureNow();
        const pending = tail;
        await pending;
        captureNow();
        if (tail === pending) return;
      }
    },
    list: () => call("editor_draft_list"),
    discard(path) {
      const id = key(path);
      submitted.delete(id);
      return enqueue(() => call("editor_draft_delete", { path: id }));
    },
    async move(previousPath) {
      // Save-as/example-copy writes the current snapshot before dropping the
      // original recovery key. A failed write must retain the old draft.
      await this.flush();
      if (key(previousPath) !== key(getSnapshot().path)) await this.discard(previousPath);
    },
  };
}

export function draftRecoveryState(draft, diskSource) {
  if (diskSource === null) return "missing";
  if (draft.source === diskSource) return "redundant";
  return draft.baseSource === diskSource ? "recoverable" : "conflict";
}

export function observeEditorState(state, controller) {
  for (const field of ["source", "savedSource", "flowPath"]) {
    let value = state[field];
    Object.defineProperty(state, field, { enumerable: true, configurable: true,
      get: () => value,
      set(next) { value = next; controller.capture(); },
    });
  }
}

export async function handleEditorExit({ request, controller, getSnapshot, choose, save, call }) {
  await controller.flush();
  if (getSnapshot().source !== getSnapshot().savedSource) {
    const action = await choose();
    if (!action || action === "cancel") { await call("editor_cancel_exit", { requestId: request.requestId }); return; }
    if (action === "save") {
      await save();
      if (getSnapshot().source !== getSnapshot().savedSource) throw new Error("保存期间出现新编辑，请再次退出");
    }
  }
  await controller.flush();
  await call("editor_confirm_exit", { requestId: request.requestId });
}

// Native dialogs with explicit choices avoid overloading Cancel as Discard.
export function chooseEditorAction({ title, detail, actions, entries = [] }) {
  return new Promise((resolve) => {
    const dialog = document.createElement("dialog");
    dialog.style.cssText = "max-width:620px;width:80%;padding:24px;border-radius:12px;color:var(--text);background:var(--surface);border:1px solid var(--line)";
    const heading = document.createElement("h3"); heading.textContent = title; dialog.append(heading);
    const body = document.createElement("p"); body.textContent = detail; body.style.whiteSpace = "pre-wrap"; dialog.append(body);
    let select;
    if (entries.length) {
      select = document.createElement("select"); select.style.cssText = "width:100%;margin-bottom:16px";
      entries.forEach((entry, index) => { const option = document.createElement("option"); option.value = String(index); option.textContent = `${entry.path || "未命名流程"} · ${new Date(entry.updatedAt).toLocaleString()}`; select.append(option); });
      dialog.append(select);
    }
    let settled = false;
    const finish = (action) => { if (settled) return; settled = true; dialog.close(); dialog.remove(); resolve({ action, entry: entries[Number(select?.value || 0)] }); };
    for (const [action, label] of actions) { const button = document.createElement("button"); button.textContent = label; button.style.marginRight = "8px"; button.addEventListener("click", () => finish(action)); dialog.append(button); }
    dialog.addEventListener("cancel", (event) => { event.preventDefault(); finish("cancel"); });
    document.body.append(dialog); dialog.showModal();
  });
}
