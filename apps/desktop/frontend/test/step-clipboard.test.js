import test from "node:test";
import assert from "node:assert/strict";

import {
  parseYaml,
  extractSteps,
  extractStepBlock,
  uniquifyStepIds,
  insertStepBlockAfter,
  insertStepBlocks,
  removeStepBlocks,
  collectStepIds,
  orderPathKeysByDocument,
  planStepBlockInsertion,
} from "../src/js/yaml.js";

const flow = `apiVersion: lumorpa.io/v1
kind: Flow
metadata:
  id: t
spec:
  steps:
    - id: fetch
      action: http.get
      with: { url: "https://x" }
    - id: loop
      action: control.for_each
      with: { list: "{{ steps.fetch.output.items }}" }
      do:
        - id: log_item
          action: control.log
          with: { message: "{{ item }}" }
    - id: done
      action: control.log
      with: { message: "{{ steps.loop.output }}" }
`;

test("collectStepIds walks nested step ids", () => {
  assert.deepEqual([...collectStepIds(flow)].sort(), ["done", "fetch", "log_item", "loop"]);
});

test("extractStepBlock returns the dedented block including children", () => {
  const block = extractStepBlock(flow, "loop");
  assert.ok(block);
  assert.equal(block.baseIndent, 4);
  assert.match(block.text, /^- id: loop\n/);
  assert.match(block.text, /\n {2}do:\n {4}- id: log_item\n/);
  assert.doesNotMatch(block.text, /- id: done/);
});

test("uniquifyStepIds renames colliding ids (nested too) and rewrites steps.<id> refs inside the block", () => {
  const block = extractStepBlock(flow, "loop").text;
  const existing = collectStepIds(flow);
  const { text, renames } = uniquifyStepIds(block, existing);
  assert.equal(renames.get("loop"), "loop_c1");
  assert.equal(renames.get("log_item"), "log_item_c1");
  assert.match(text, /^- id: loop_c1\n/);
  assert.match(text, /- id: log_item_c1\n/);
  // the copied block still references the ORIGINAL fetch step outside it…
  assert.match(text, /steps\.fetch\.output\.items/);
  // …and no reference to the old inner ids remains rewritten incorrectly.
  assert.doesNotMatch(text, /- id: loop\n/);
});

test("uniquifyStepIds keeps ids untouched when there is no collision", () => {
  const block = extractStepBlock(flow, "done").text;
  const { text, renames } = uniquifyStepIds(block, new Set(["other"]));
  assert.equal(renames.size, 0);
  assert.match(text, /^- id: done\n/);
});

test("uniquifyStepIds sanitizes quoted ids when renaming", () => {
  const block = '- id: "my step"\n  action: control.log\n  with: { message: hi }\n';
  const { text, renames } = uniquifyStepIds(block, new Set(["my step"]));
  assert.equal(renames.get("my step"), "my_step_c1");
  assert.match(text, /^- id: my_step_c1\n/);
});

test("insertStepBlockAfter inserts a sibling after the target with matching indent", () => {
  const block = extractStepBlock(flow, "done").text;
  const { text } = uniquifyStepIds(block, collectStepIds(flow));
  const next = insertStepBlockAfter(flow, "fetch", text);
  const ids = extractSteps(parseYaml(next)).map((s) => s.id);
  assert.deepEqual(ids, ["fetch", "done_c1", "loop", "done"]);
});

test("insertStepBlockAfter after a nested step inserts inside the same child block", () => {
  const block = "- id: extra\n  action: control.log\n  with: { message: x }\n";
  const next = insertStepBlockAfter(flow, "log_item", block);
  const steps = extractSteps(parseYaml(next));
  assert.deepEqual(steps[1].do.map((s) => s.id), ["log_item", "extra"]);
});

test("insertStepBlocks pastes multiple blocks in order after the target, re-iding collisions", () => {
  const blocks = [
    extractStepBlock(flow, "fetch").text,
    extractStepBlock(flow, "done").text,
  ];
  const next = insertStepBlocks(flow, "done", blocks);
  const ids = extractSteps(parseYaml(next)).map((s) => s.id);
  assert.deepEqual(ids, ["fetch", "loop", "done", "fetch_c1", "done_c1"]);
});

test("insertStepBlocks with null target appends at the end of top-level steps", () => {
  const blocks = ["- id: tail\n  action: control.log\n  with: { message: t }\n"];
  const next = insertStepBlocks(flow, null, blocks);
  const ids = extractSteps(parseYaml(next)).map((s) => s.id);
  assert.deepEqual(ids, ["fetch", "loop", "done", "tail"]);
});

test("removeStepBlocks deletes top-level and nested steps by id", () => {
  const next = removeStepBlocks(flow, ["fetch", "log_item"]);
  const steps = extractSteps(parseYaml(next));
  assert.deepEqual(steps.map((s) => s.id), ["loop", "done"]);
  const loopDo = steps[0].do;
  assert.ok(!Array.isArray(loopDo) || loopDo.length === 0);
});

test("orderPathKeysByDocument returns ids and keys in walk order", () => {
  const { orderedIds, orderedKeys } = orderPathKeysByDocument(flow, ["2", "1/:do/0", "0"]);
  assert.deepEqual(orderedIds, ["fetch", "log_item", "done"]);
  assert.deepEqual(orderedKeys, ["0", "1/:do/0", "2"]);
});

test("copying a selection migrates cross-block references once and keeps external references", () => {
  const source = 'spec:\n  steps:\n    - id: read\n      action: control.log\n      with: {message: hello}\n    - id: save\n      action: control.log\n      when: steps.read.result != null\n      with: {message: "{{ steps.read.result }} / {{ steps.external.result }}"}\n';
  const blocks = ["read", "save"].map((id) => extractStepBlock(source, id).text);
  const plan = planStepBlockInsertion(source, [1], blocks);
  const steps = extractSteps(parseYaml(plan.source));
  assert.equal(plan.insertedCount, 2);
  assert.deepEqual(plan.ids, ["read_c1", "save_c1"]);
  assert.equal(steps[3].when, "steps.read_c1.result != null");
  assert.equal(steps[3].with.message, "{{ steps.read_c1.result }} / {{ steps.external.result }}");
  assert.equal(steps[1].with.message, "{{ steps.read.result }} / {{ steps.external.result }}");
});

test("inline and compact list nodes paste into the exact target list", () => {
  const source = 'spec: {steps: [{id: gate, action: control.if, do: [{id: child, action: control.log, with: {message: hello}}]}], outputs: {status: ready}}\n';
  const block = extractStepBlock(source, "child").text;
  const plan = planStepBlockInsertion(source, [0, "do", 0], [block]);
  const ast = parseYaml(plan.source);
  assert.deepEqual(ast.spec.steps[0].do.map((step) => step.id), ["child", "child_c1"]);
  assert.deepEqual(ast.spec.outputs, { status: "ready" });
  assert.equal(plan.insertedCount, 1);
});

test("pasted ids reserve every selected original id and bracket references migrate", () => {
  const blocks = ['- {id: read, action: control.log}\n', '- {id: read_c1, action: control.log, with: {message: "{{ steps[\'read\'].result }}"}}\n'];
  const source = 'spec: {steps: [{id: read, action: control.log}]}\n';
  const ast = parseYaml(planStepBlockInsertion(source, null, blocks).source);
  assert.deepEqual(ast.spec.steps.map((step) => step.id), ["read", "read_c2", "read_c1"]);
  assert.equal(ast.spec.steps[2].with.message, '{{ steps["read_c2"].result }}');
});

test("selecting a parent and its child copies only one subtree", () => {
  const selection = orderPathKeysByDocument(flow, ["1", "1/:do/0"]);
  assert.deepEqual(selection.orderedKeys, ["1"]);
});
