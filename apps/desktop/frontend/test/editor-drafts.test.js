import test from "node:test";
import assert from "node:assert/strict";
import { createDraftController, handleEditorExit, draftRecoveryState } from "../src/js/editor-drafts.js";

const deferred = () => { let resolve, reject; const promise = new Promise((a, b) => { resolve = a; reject = b; }); return { promise, resolve, reject }; };
const draft = (source = "edited") => ({ path: "/flows/a.yaml", source, savedSource: "disk" });

test("draft autosave preserves invalid YAML and serializes a newer edit after an old write", async () => {
  const first = deferred(); const writes = []; let snapshot = draft("spec: [");
  const controller = createDraftController({ getSnapshot: () => snapshot, call: async (command, args) => {
    writes.push([command, args]); if (writes.length === 1) await first.promise;
  } });
  controller.start(); controller.capture(); await Promise.resolve();
  snapshot = draft("newer"); controller.capture();
  const flushing = controller.flush(); first.resolve(); await flushing;
  assert.deepEqual(writes.map(([, args]) => args.draft.source), ["spec: [", "newer"]);
});

test("a save acknowledgement cannot delete the newer unsaved revision", async () => {
  let snapshot = draft(); const calls = [];
  const controller = createDraftController({ getSnapshot: () => snapshot, call: async (cmd, args) => calls.push([cmd, args]) });
  controller.start(); await controller.flush();
  snapshot = { ...draft("new edit"), savedSource: "edited" };
  await controller.flush();
  assert.equal(calls.at(-1)[0], "editor_draft_upsert");
  assert.equal(calls.at(-1)[1].draft.source, "new edit");
  snapshot.savedSource = "new edit"; await controller.flush();
  assert.equal(calls.at(-1)[0], "editor_draft_delete");
});

test("draft storage failures reject flush and prevent exit confirmation", async () => {
  const calls = []; const controller = createDraftController({ getSnapshot: () => draft(), call: async () => { throw new Error("disk full"); } });
  controller.start();
  await assert.rejects(handleEditorExit({ request: { requestId: "q", kind: "quit" }, controller, getSnapshot: () => draft(), choose: async () => "keep", call: async (cmd) => calls.push(cmd) }), /disk full/);
  assert.deepEqual(calls, []);
});

test("exit waits for durable draft flush, cancellation does not confirm, and save errors stop exit", async () => {
  const gate = deferred(); const calls = []; let snapshot = draft();
  const controller = createDraftController({ getSnapshot: () => snapshot, call: async () => gate.promise }); controller.start();
  const exiting = handleEditorExit({ request: { requestId: "q", kind: "quit" }, controller, getSnapshot: () => snapshot, choose: async () => "keep", call: async (cmd) => calls.push(cmd) });
  await Promise.resolve(); assert.deepEqual(calls, []); gate.resolve(); await exiting;
  assert.deepEqual(calls, ["editor_confirm_exit"]);
  calls.length = 0;
  await handleEditorExit({ request: { requestId: "q2" }, controller, getSnapshot: () => snapshot, choose: async () => "cancel", call: async (cmd) => calls.push(cmd) });
  assert.deepEqual(calls, ["editor_cancel_exit"]);
  await assert.rejects(handleEditorExit({ request: { requestId: "q3" }, controller, getSnapshot: () => snapshot, choose: async () => "save", save: async () => { throw new Error("invalid YAML"); }, call: async (cmd) => calls.push(cmd) }), /invalid YAML/);
  assert.deepEqual(calls, ["editor_cancel_exit"]);
});

test("recovery distinguishes changed disk content and leaves the disk baseline intact", () => {
  const entry = { source: "draft", baseSource: "old disk" };
  assert.equal(draftRecoveryState(entry, "external edit"), "conflict");
  assert.equal(draftRecoveryState(entry, "old disk"), "recoverable");
  assert.equal(draftRecoveryState(entry, "draft"), "redundant");
  assert.equal(draftRecoveryState(entry, null), "missing");
});

test("a failed draft listing followed by a clean disk load cannot delete unseen recovery data", async () => {
  let snapshot = { ...draft("disk"), savedSource: "disk" };
  const writes = [];
  const controller = createDraftController({ getSnapshot: () => snapshot, call: async (command, args) => {
    if (command === "editor_draft_list") throw new Error("one corrupted draft");
    writes.push([command, args]);
  } });
  controller.start();
  await assert.rejects(controller.list(), /corrupted draft/);
  controller.capture(); await controller.flush();
  assert.deepEqual(writes, [], "unseen drafts must survive a default clean flow load");
  snapshot = draft("new edit");
  await controller.flush();
  assert.equal(writes.at(-1)[0], "editor_draft_upsert");
  snapshot.savedSource = snapshot.source;
  await controller.flush();
  assert.equal(writes.at(-1)[0], "editor_draft_delete", "a confirmed save may clean up its own recovery data");
});
