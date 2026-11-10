import test from "node:test";
import assert from "node:assert/strict";
import { appendGraphStep, insertGraphStepAfter, nextGraphStepId, resolveSelectionAfterEdit } from "../src/js/editor/graph-editing.js";
import { parseYaml, extractSteps } from "../src/js/yaml.js";
import { fitGraphBounds, screenToWorld } from "../src/js/editor/graph-viewport.js";

test("canvas insertion stays inside steps before outputs and retains untouched source", () => {
  const source = '# project note\nspec:\n  steps:\n    - id: first\n      action: control.log\n      with: {message: hi}\n  outputs:\n    result: true\nmetadata:\n  id: demo\n';
  const next = appendGraphStep(source, "file.read");
  assert.deepEqual(extractSteps(parseYaml(next.source)).map((step) => step.id), ["first", "file_read_1"]);
  assert.deepEqual(parseYaml(next.source).spec.outputs, { result: true });
  assert.ok(next.source.startsWith("# project note\n"));
  assert.match(next.source, /metadata:\n {2}id: demo/);
});

test("empty inline steps can receive a node without losing comments or outputs", () => {
  const source = '# keep\nspec:\n  steps: []\n  outputs: {value: 42}\n';
  const next = appendGraphStep(source, "control.log");
  assert.equal(extractSteps(parseYaml(next.source))[0].action, "control.log");
  assert.ok(next.source.startsWith("# keep\n"));
  assert.match(next.source, /outputs: \{value: 42\}/);
});

test("new nodes have deterministic unique IDs including nested collisions", () => {
  const source = 'spec:\n  steps:\n    - id: control_log_1\n      action: control.if\n      do:\n        - id: control_log_2\n          action: control.log\n';
  assert.equal(nextGraphStepId(source, "control.log"), "control_log_3");
  const next = appendGraphStep("", "control.log");
  assert.equal(extractSteps(parseYaml(next.source)).length, 1);
});

test("selection follows a stable node through insertion, reorder and deletion", () => {
  const old = parseYaml('spec:\n  steps:\n    - {id: a, action: control.log}\n    - {id: b, action: control.log}\n');
  const next = parseYaml('spec:\n  steps:\n    - {id: b, action: control.log}\n    - {id: new, action: control.log}\n    - {id: a, action: control.log}\n');
  assert.deepEqual(resolveSelectionAfterEdit(old, next, [1]), [0]);
  assert.equal(resolveSelectionAfterEdit(old, parseYaml('spec: {steps: []}'), [1]), null);
});

test("selection distinguishes identical child IDs in do and else after insertion changes numeric paths", () => {
  const previous = parseYaml('spec: {steps: [{id: gate, action: control.if, do: [{id: same, action: control.log}], else: [{id: same, action: control.log}]}]}');
  const next = parseYaml('spec: {steps: [{id: inserted, action: control.log}, {id: gate, action: control.if, do: [{id: same, action: control.log}], else: [{id: new, action: control.log}, {id: same, action: control.log}]}]}');
  assert.deepEqual(resolveSelectionAfterEdit(previous, next, [0, "else", 0]), [1, "else", 1]);
  assert.deepEqual(resolveSelectionAfterEdit(previous, next, [0, "do", 0]), [1, "do", 0]);
  const deleted = parseYaml('spec: {steps: [{id: gate, action: control.if, do: [{id: same, action: control.log}], else: [{id: replacement, action: control.log}]}]}');
  assert.equal(resolveSelectionAfterEdit(previous, deleted, [0, "else", 0]), null);
});

test("ambiguous sibling IDs and ancestors clear selection instead of guessing the old index", () => {
  const previous = parseYaml('spec: {steps: [{id: gate, action: control.if, do: [{id: same, action: control.log}]}]}');
  const duplicateChild = parseYaml('spec: {steps: [{id: gate, action: control.if, do: [{id: same, action: control.log}, {id: same, action: control.log}]}]}');
  assert.equal(resolveSelectionAfterEdit(previous, duplicateChild, [0, "do", 0]), null);
  const duplicateParent = parseYaml('spec: {steps: [{id: gate, action: control.if, do: [{id: same, action: control.log}]}, {id: gate, action: control.if, do: [{id: same, action: control.log}]}]}');
  assert.equal(resolveSelectionAfterEdit(previous, duplicateParent, [0, "do", 0]), null);
  assert.equal(resolveSelectionAfterEdit(duplicateChild, previous, [0, "do", 0]), null);
});

test("empty null and inline steps retain configured input types and unknown fields", () => {
  for (const value of ["null", "[]"]) {
    const source = `# keep\nspec: {steps: ${value}, inputs: {enabled: false, count: 0, code: "001", date: "2026-11-10", nullable: null}, custom: {keep: [true, 0]}, outputs: {value: 42}}\n`;
    const next = parseYaml(appendGraphStep(source, "control.log").source);
    assert.deepEqual(next.spec.inputs, parseYaml(source).spec.inputs);
    assert.deepEqual(next.spec.custom, { keep: [true, 0] });
    assert.deepEqual(next.spec.outputs, { value: 42 });
    assert.equal(next.spec.steps.length, 1);
  }
});

test("nonempty inline steps append inside the sequence without changing surrounding flow mappings", () => {
  const source = '# keep\nspec: {steps: [{id: first, action: control.log, with: {message: "001"}}], inputs: {count: 0, enabled: false}, outputs: {value: 42}, custom: strange}\n';
  const next = appendGraphStep(source, "file.read");
  const ast = parseYaml(next.source);
  assert.deepEqual(ast.spec.steps[0], parseYaml(source).spec.steps[0]);
  assert.equal(ast.spec.steps[1].id, "file_read_1");
  assert.deepEqual(ast.spec.inputs, { count: 0, enabled: false });
  assert.equal(ast.spec.custom, "strange");
  assert.ok(next.source.startsWith('# keep\nspec: {steps: [{id: first, action: control.log, with: {message: "001"}}, '));
  assert.ok(next.source.endsWith('], inputs: {count: 0, enabled: false}, outputs: {value: 42}, custom: strange}\n'));
});

test("append preserves a trailing literal block scalar and rejects invalid step structures", () => {
  const source = 'spec:\n  steps:\n    - id: first\n      action: control.log\n      with:\n        message: |+\n          line\n\n\n  outputs: {keep: true}\n';
  const next = appendGraphStep(source, "control.log");
  assert.deepEqual(extractSteps(parseYaml(next.source))[0], extractSteps(parseYaml(source))[0]);
  assert.deepEqual(parseYaml(next.source).spec.outputs, { keep: true });
  assert.throws(() => appendGraphStep("spec: {steps: invalid}", "control.log"), /steps/);
});

test("path insertion chooses the exact do or else sibling despite duplicate IDs", () => {
  const source = `# keep
spec:
  steps:
    - id: gate
      action: control.if
      custom: {unchanged: false}
      do:
        - id: same
          action: control.log
          with: {message: do}
      else:
        - id: same
          action: control.log
          with: {message: else}
        - id: last
          action: control.log
  outputs: {keep: true}
`;
  const inserted = insertGraphStepAfter(source, [0, "else", 0], "file.read");
  const gate = extractSteps(parseYaml(inserted.source))[0];
  assert.deepEqual(gate.do, extractSteps(parseYaml(source))[0].do);
  assert.deepEqual(gate.else.map((node) => node.id), ["same", "file_read_1", "last"]);
  assert.equal(gate.else[0].with.message, "else");
  assert.deepEqual(gate.custom, { unchanged: false });
  assert.ok(inserted.source.startsWith(source.slice(0, source.indexOf("      else:"))));
  assert.ok(inserted.source.endsWith("  outputs: {keep: true}\n"));
  const doInserted = insertGraphStepAfter(source, [0, "do", 0], "file.read");
  assert.deepEqual(extractSteps(parseYaml(doInserted.source))[0].else, extractSteps(parseYaml(source))[0].else);
  assert.deepEqual(extractSteps(parseYaml(doInserted.source))[0].do.map((node) => node.id), ["same", "file_read_1"]);
});

test("path insertion supports inline nested lists and non-id-first mappings", () => {
  const source = '# keep\nspec: {steps: [{id: p, action: control.parallel, branches: [[{action: control.log, id: same, custom: 0}], [{id: same, action: control.log}]]}], inputs: {code: "001"}, custom: false}\n';
  const inserted = insertGraphStepAfter(source, [0, "branches", 1, 0], "control.log");
  const ast = parseYaml(inserted.source);
  assert.deepEqual(ast.spec.steps[0].branches[0], parseYaml(source).spec.steps[0].branches[0]);
  assert.deepEqual(ast.spec.steps[0].branches[1].map((node) => node.id), ["same", "control_log_1"]);
  assert.deepEqual(ast.spec.inputs, { code: "001" });
  assert.equal(ast.spec.custom, false);
  assert.ok(inserted.source.startsWith('# keep\nspec: {steps: [{id: p, action: control.parallel, branches: [[{action: control.log, id: same, custom: 0}], [{id: same, action: control.log}, '));
  assert.throws(() => insertGraphStepAfter(source, [4], "control.log"), /不存在/);
});

test("fit includes nodes in negative world space and centers their full bounds", () => {
  const bounds = { x: -400, y: -200, width: 1100, height: 500 };
  const transform = fitGraphBounds(bounds, 800, 500);
  const center = screenToWorld(transform, 400, 250);
  assert.ok(Math.abs(center.x - 150) < 1e-8 && Math.abs(center.y - 50) < 1e-8);
  assert.ok(bounds.width * transform.scale <= 800 - 112 + 0.001);
  assert.ok(bounds.height * transform.scale <= 500 - 112 + 0.001);
});
