import assert from "node:assert/strict";
import test from "node:test";

globalThis.window = {};
globalThis.localStorage = { getItem: () => null };
const { state } = await import("../src/js/state.js");
const { copySelectedSteps, pasteStepsFromClipboard } = await import("../src/js/editor/mutations.js");
const { resetHistory, undoEdit } = await import("../src/js/editor/history.js");
const { parseYaml } = await import("../src/js/yaml.js");

function mount(context, source, blocks = []) {
  const previous = { document: globalThis.document, localStorage: globalThis.localStorage, setTimeout: globalThis.setTimeout };
  context.after(() => Object.assign(globalThis, previous));
  globalThis.setTimeout = () => 0;
  const storage = new Map([["lumo.stepClipboard", JSON.stringify({ blocks })]]);
  globalThis.localStorage = { getItem: (key) => storage.get(key) || null, setItem: (key, value) => storage.set(key, String(value)) };
  const element = () => ({ value: "", textContent: "", innerHTML: "", style: {}, classList: { toggle() {} },
    addEventListener() {}, appendChild(child) { this.children.push(child); }, children: [], querySelectorAll: () => [] });
  const ids = new Map(["codeEditor", "codeGutter", "saveFlowBtn", "statusText", "undoBtn", "redoBtn", "inspectorBody", "toastStack"].map((id) => [id, element()]));
  globalThis.document = { getElementById: (id) => ids.get(id) || null, createElement: element };
  state.source = source; state.savedSource = source; state.ast = parseYaml(source);
  state.flowPath = "/flows/mutations.yaml"; state.viewMode = "code";
  state.selectedStepPath = null; state.selectedStepId = null; state.selectedStepPaths = new Set();
  state.graphLayouts = {}; state.breakpoints = new Set(); state.draftController = null;
  resetHistory();
  return { ids, storage, toasts: () => ids.get("toastStack").children.map((node) => node.innerHTML).join("\n") };
}

test("clipboard paste reports the actual inserted steps and a single undo restores source", (context) => {
  const source = "spec: {steps: [{id: existing, action: control.log}]}\n";
  const view = mount(context, source, ["- {id: read, action: control.log}\n- {id: save, action: control.log, with: {message: '{{ steps.read.result }}'}}\n"]);
  assert.equal(pasteStepsFromClipboard(), 2);
  assert.deepEqual(state.ast.spec.steps.map((step) => step.id), ["existing", "read", "save"]);
  assert.match(view.toasts(), /已粘贴 2 个步骤/);
  assert.equal(undoEdit(), true);
  assert.equal(state.source, source);
  assert.equal(view.ids.get("undoBtn").disabled, true);
});

test("invalid clipboard content cannot mutate source, add undo history or show success", (context) => {
  const source = "spec: {steps: [{id: existing, action: control.log}]}\n";
  const view = mount(context, source, ["- {id: broken, action: [invalid\n"]);
  assert.equal(pasteStepsFromClipboard(), 0);
  assert.equal(state.source, source);
  assert.equal(view.ids.get("undoBtn").disabled, true);
  assert.match(view.toasts(), /粘贴失败/);
  assert.doesNotMatch(view.toasts(), /已粘贴/);
});

test("copying an ancestor and its selected child stores one complete subtree", (context) => {
  const source = "spec: {steps: [{id: gate, action: control.if, do: [{id: child, action: control.log}]}]}\n";
  const view = mount(context, source);
  state.selectedStepPaths = new Set(["0", "0/:do/0"]);
  assert.equal(copySelectedSteps(), 1);
  const blocks = JSON.parse(view.storage.get("lumo.stepClipboard")).blocks;
  assert.equal(blocks.length, 1);
  const steps = parseYaml(blocks[0]);
  assert.equal(steps[0].id, "gate");
  assert.equal(steps[0].do[0].id, "child");
});
