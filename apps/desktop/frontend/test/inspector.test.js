import test from "node:test";
import assert from "node:assert/strict";

let invoke = () => {};
globalThis.window = { __TAURI__: { core: { invoke: (...args) => invoke(...args) } } };
globalThis.localStorage = { getItem: () => null };
const { state } = await import("../src/js/state.js");
const { renderInspector } = await import("../src/js/editor/inspector.js");
const { redoEdit, resetHistory, undoEdit } = await import("../src/js/editor/history.js");
const { extractSteps, findStepByPath, parseYaml } = await import("../src/js/yaml.js");

function decode(value = "") {
  return value.replaceAll("&quot;", '"').replaceAll("&lt;", "<").replaceAll("&gt;", ">").replaceAll("&amp;", "&");
}

function ui(context) {
  const previous = globalThis.document;
  const previousTimeout = globalThis.setTimeout;
  context.after(() => { globalThis.document = previous; globalThis.setTimeout = previousTimeout; });
  globalThis.setTimeout = () => 0;
  const ids = new Map();
  const element = () => ({
    handlers: {}, dataset: {}, type: "", value: "", checked: false, hidden: false,
    style: {}, classList: { toggle() {} }, textContent: "",
    addEventListener(name, handler) { this.handlers[name] = handler; },
    appendChild() {}, querySelectorAll() { return []; },
  });
  let form;
  function parseControls(markup, root) {
    const controls = [];
    for (const match of markup.matchAll(/<(input|textarea|select)\b([^>]*?)(?:\/>|>(?:(.*?)<\/\1>)?)/gs)) {
      const [, tag, attrs, content] = match;
      const control = element();
      control.type = /\btype="([^"]*)"/.exec(attrs)?.[1] || "text";
      control.value = decode(/\bvalue="([^"]*)"/.exec(attrs)?.[1] ?? (tag === "textarea" ? content : "") ?? "");
      control.checked = /\bchecked\b/.test(attrs);
      for (const attr of attrs.matchAll(/data-([\w-]+)="([^"]*)"/g)) {
        control.dataset[attr[1].replace(/-([a-z])/g, (_all, ch) => ch.toUpperCase())] = decode(attr[2]);
      }
      if (tag === "select") {
        const options = [...(content || "").matchAll(/<option value="([^"]*)"([^>]*)>/g)];
        control.value = decode((options.find((opt) => opt[2].includes("selected")) || options[0])?.[1]);
      }
      const id = /\bid="([^"]*)"/.exec(attrs)?.[1];
      if (id) ids.set(id, control);
      controls.push(control);
    }
    root.controls = controls;
  }
  const matchControl = (control, selector) => {
    const match = /^\[data-([\w-]+)(?:="([^"]*)")?\]$/.exec(selector);
    if (!match) return false;
    const key = match[1].replace(/-([a-z])/g, (_all, ch) => ch.toUpperCase());
    return Object.hasOwn(control.dataset, key) && (match[2] === undefined || control.dataset[key] === match[2]);
  };
  const body = element();
  let sourceMarkup = "";
  Object.defineProperty(body, "innerHTML", { get() { return sourceMarkup; }, set(markup) {
    sourceMarkup = markup;
    form = element();
    parseControls(markup, form);
    const groups = Object.fromEntries(["[data-advanced-retry]", "[data-advanced-error-retry]", "[data-inspector-error]"].map((selector) => [selector, element()]));
    form.querySelectorAll = (selector) => [...form.controls, ...(ids.get("schemaFields")?.controls || [])].filter((control) => matchControl(control, selector));
    form.querySelector = (selector) => groups[selector] || form.querySelectorAll(selector)[0] || null;
    ids.set("propForm", form);
    const schemaFields = element();
    Object.defineProperty(schemaFields, "innerHTML", { set(value) { parseControls(value, schemaFields); }, get() { return ""; } });
    ids.set("schemaFields", schemaFields);
    for (const id of ["applyStepBtn", "runThisStepBtn", "toggleBpBtn"]) ids.set(id, element());
  } });
  ids.set("inspectorBody", body);
  for (const id of ["codeEditor", "codeGutter", "saveFlowBtn", "statusText", "toastStack", "undoBtn", "redoBtn"]) ids.set(id, element());
  globalThis.document = { getElementById: (id) => ids.get(id) || null, createElement: element };
  const flow = (source, path = [0]) => {
    state.source = source;
    state.savedSource = source;
    state.ast = parseYaml(source);
    state.flowPath = "/flows/test.yaml";
    state.selectedStepPath = path;
    state.selectedStepId = findStepByPath(extractSteps(state.ast), path)?.id;
    state.viewMode = "code";
    state.schemaCache.clear();
    state.draftController = null;
    resetHistory();
  };
  const get = (key) => form.querySelector(`[data-advanced="${key}"]`);
  return { ids, flow, get, form: () => form, apply: () => ids.get("applyStepBtn").handlers.click(), error: () => form.querySelector("[data-inspector-error]").textContent };
}

test("inspector applies nested policies, captures drafts, and undo/redo restores exact source and selection", async (context) => {
  const view = ui(context);
  const source = `spec:\n  steps:\n    - id: parallel\n      action: control.parallel\n      branches:\n        - - id: repeated\n            action: control.log\n            with: {message: first}\n        - - id: repeated\n            action: control.log\n            resource: session\n            with: {message: second, untouched: preserve}\n`;
  const selected = [0, "branches", 1, 0];
  view.flow(source, selected);
  invoke = async () => ({ properties: { message: { type: "string" } }, required: ["message"] });
  let captures = 0;
  state.draftController = { capture() { captures++; } };
  await renderInspector();
  view.get("when").value = "vars.ready";
  view.get("bind").value = "result";
  view.get("retryEnabled").checked = true;
  view.get("retryTimes").value = "3";
  view.get("errorMode").value = "continue";
  view.get("errorOutputEnabled").checked = true;
  view.get("errorOutput").value = '{"state":"failed"}';
  view.form().querySelector('[data-with-key="message"]').value = "changed";
  view.apply();
  const changed = state.source;
  const step = findStepByPath(extractSteps(state.ast), selected);
  assert.equal(step.when, "vars.ready");
  assert.equal(step.bind, "result");
  assert.equal(step.retry.times, 3);
  assert.deepEqual(step.on_error, { mode: "continue", output: { state: "failed" } });
  assert.deepEqual(step.with, { message: "changed", untouched: "preserve" });
  assert.equal(step.resource, "session");
  assert.equal(findStepByPath(extractSteps(state.ast), [0, "branches", 0, 0]).with.message, "first");
  assert.equal(captures, 1);
  assert.equal(undoEdit(), true);
  assert.equal(state.source, source);
  assert.deepEqual(state.selectedStepPath, selected);
  assert.equal(redoEdit(), true);
  assert.equal(state.source, changed);
  assert.equal(captures, 3);
});

test("invalid values and stale inspector forms cannot change the buffer or create history", async (context) => {
  const view = ui(context);
  const source = "spec:\n  steps:\n    - id: a\n      action: control.log\n      with: {message: hi}\n";
  view.flow(source);
  invoke = async () => ({ properties: { message: { type: "string" } } });
  await renderInspector();
  view.get("errorOutputEnabled").checked = true;
  view.get("errorOutput").value = "{broken";
  view.apply();
  assert.match(view.error(), /JSON/);
  assert.equal(state.source, source);
  assert.equal(view.ids.get("undoBtn").disabled, true);
  state.source += "# a new code edit\n";
  view.apply();
  assert.match(view.error(), /已变化/);
  assert.equal(state.source, `${source}# a new code edit\n`);
});

test("late schema results cannot overwrite a newly selected node", async (context) => {
  const view = ui(context);
  view.flow("spec:\n  steps:\n    - id: a\n      action: first\n    - id: b\n      action: second\n");
  const pending = new Map();
  invoke = (_command, { id }) => new Promise((resolve) => pending.set(id, resolve));
  const first = renderInspector();
  state.selectedStepPath = [1];
  const second = renderInspector();
  pending.get("second")({ properties: { latest: { type: "string" } } });
  await second;
  pending.get("first")({ properties: { stale: { type: "string" } } });
  await first;
  assert.equal(view.form().querySelector('[data-with-key="stale"]'), null);
  assert.ok(view.form().querySelector('[data-with-key="latest"]'));
  assert.equal(view.ids.get("applyStepBtn").disabled, false);
});

test("failed schema requests keep applying disabled and retain existing action parameters", async (context) => {
  const view = ui(context);
  const source = "spec:\n  steps:\n    - id: a\n      action: first\n      with: {keep: value}\n";
  view.flow(source);
  invoke = async () => { throw new Error("offline"); };
  await renderInspector();
  assert.equal(view.ids.get("applyStepBtn").disabled, true);
  view.apply();
  assert.equal(state.source, source);
  assert.match(view.error(), /加载完成/);
});

test("action changes wait for their own schema and do not carry incompatible parameters", async (context) => {
  const view = ui(context);
  view.flow("spec:\n  steps:\n    - id: a\n      action: first\n      with: {old: value}\n");
  let finish;
  invoke = async (_command, { id }) => id === "first" ? { properties: { old: { type: "string" } } } : new Promise((resolve) => { finish = resolve; });
  await renderInspector();
  const action = view.form().querySelector('[data-prop="action"]');
  action.value = "second";
  const loading = action.handlers.change();
  assert.equal(view.ids.get("applyStepBtn").disabled, true);
  finish({ properties: { next: { type: "integer" } } });
  await loading;
  view.form().querySelector('[data-with-key="next"]').value = "4";
  view.apply();
  assert.deepEqual(extractSteps(state.ast)[0].with, { next: 4 });
  assert.equal(extractSteps(state.ast)[0].action, "second");
});

test("typed action parameters keep template expressions through policy edits", async (context) => {
  const view = ui(context);
  view.flow('spec:\n  steps:\n    - id: a\n      action: custom\n      with: {count: "{{ inputs.count }}", enabled: "{{ inputs.enabled }}", config: "{{ inputs.config }}"}\n');
  invoke = async () => ({ properties: { count: { type: "integer" }, enabled: { type: "boolean" }, config: { type: "object" } } });
  await renderInspector();
  view.get("when").value = "true";
  view.apply();
  assert.deepEqual(extractSteps(state.ast)[0].with, { count: "{{ inputs.count }}", enabled: "{{ inputs.enabled }}", config: "{{ inputs.config }}" });
});

test("ordinary node selection clears hidden multi-selection and repeated selection keeps form drafts", async (context) => {
  const view = ui(context);
  view.flow('spec:\n  steps:\n    - {id: a, action: control.log, with: {message: original}}\n    - {id: b, action: control.log, with: {message: second}}\n');
  invoke = async () => ({ properties: { message: { type: "string" } } });
  await renderInspector();
  const { selectStep } = await import("../src/js/editor/inspector.js");
  const form = view.form();
  form.querySelector('[data-with-key="message"]').value = "unsaved property draft";
  state.selectedStepPaths = new Set(["0", "1"]);
  selectStep([0]);
  assert.equal(state.selectedStepPaths.size, 0);
  assert.equal(view.form(), form);
  assert.equal(form.querySelector('[data-with-key="message"]').value, "unsaved property draft");
  await selectStep([1]);
  assert.equal(state.selectedStepId, "b");
});
