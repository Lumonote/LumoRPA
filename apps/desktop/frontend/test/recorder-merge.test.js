import test from "node:test";
import assert from "node:assert/strict";

globalThis.window = { __TAURI__: { core: { invoke() {} } } };
globalThis.localStorage = { getItem: () => null };
const { mergeRecorderPatchIntoSource } = await import("../src/js/recorder.js");
const { parseYaml } = await import("../src/js/yaml.js");

const patch = `- id: click_1
  action: desktop.click
  with: { x: 10, y: 20, button: left }
- id: text_2
  action: desktop.type
  with: { text: 中文 }
`;

test("desktop merge adds exact categories and preserves unrelated source", () => {
  const source = `# user document
metadata:
  id: demo
spec:
  capabilities:
    network: [example.com]
    fs.read: [/data/**]
  steps:
    - id: existing
      action: control.noop
# trailing user note
`;
  const merged = mergeRecorderPatchIntoSource(source, patch);
  const ast = parseYaml(merged);
  assert.deepEqual(ast.spec.capabilities.desktop, ["mouse", "keyboard"]);
  assert.deepEqual(ast.spec.capabilities.network, ["example.com"]);
  assert.deepEqual(ast.spec.capabilities["fs.read"], ["/data/**"]);
  assert.equal(ast.spec.steps.length, 3);
  assert.ok(merged.startsWith("# user document\nmetadata:\n  id: demo\n"));
  assert.match(merged, /# trailing user note/);
  assert.ok(!merged.includes('desktop: ["*"]'));
});

test("desktop merge supports inline capabilities and steps and avoids duplicate ids", () => {
  const source = "metadata: {id: inline}\nspec: {capabilities: {desktop: [mouse], network: [example.com]}, steps: [{id: click_1, action: control.noop}]}\n";
  const ast = parseYaml(mergeRecorderPatchIntoSource(source, patch));
  assert.deepEqual(ast.spec.capabilities.desktop, ["mouse", "keyboard"]);
  assert.deepEqual(ast.spec.capabilities.network, ["example.com"]);
  assert.deepEqual(ast.spec.steps.map((step) => step.id), ["click_1", "click_1_2", "text_2"]);
});

test("desktop merge inserts missing capabilities and handles an empty inline steps list", () => {
  for (const source of ["metadata: {id: demo}\nspec:\n  steps: []\n", "metadata: {id: demo}\nspec: {steps: []}\n", "spec:\n  steps: []\n  capabilities: null\n", "spec: {steps: [], capabilities: null}\n"]) {
    const ast = parseYaml(mergeRecorderPatchIntoSource(source, patch));
    assert.deepEqual(ast.spec.capabilities.desktop, ["mouse", "keyboard"]);
    assert.equal(ast.spec.steps.length, 2);
  }
});

test("desktop merge keeps existing wildcard and does not grant screen or network", () => {
  const source = "metadata: {id: demo}\nspec:\n  capabilities: {desktop: ['*']}\n  steps: []\n";
  const ast = parseYaml(mergeRecorderPatchIntoSource(source, patch));
  assert.deepEqual(ast.spec.capabilities, { desktop: ["*"] });
});

test("browser-only merge leaves capability formatting intact", () => {
  const source = "metadata: {id: demo}\nspec:\n  capabilities: {network: [example.com]} # keep comment\n  steps: []\n";
  const merged = mergeRecorderPatchIntoSource(source, "- id: open\n  action: browser.open\n  with: {url: https://example.com}\n");
  assert.match(merged, /capabilities: \{network: \[example.com\]\} # keep comment/);
  assert.equal(parseYaml(merged).spec.capabilities.desktop, undefined);
});
