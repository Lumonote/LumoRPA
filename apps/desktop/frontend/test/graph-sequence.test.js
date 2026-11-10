import assert from "node:assert/strict";
import test from "node:test";
import { planGraphSequence } from "../src/js/editor/graph-sequence.js";
import { extractSteps, parseYaml } from "../src/js/yaml.js";

const source = `# preserved header
metadata: {id: sequence-demo}
spec:
  steps:
    - id: a
      action: control.log
      with: {message: a}
    - id: b
      action: control.if
      custom: {keep: [0, false]}
      when: "true"
      do:
        - id: child
          action: control.log
          with: {message: nested}
    - id: c
      action: control.log
      with: {message: c}
  # preserved footer
  outputs: {keep: true}
`;
const ids = (text) => extractSteps(parseYaml(text)).map((step) => step.id);

test("execution link moves the target subtree after its source and preserves unknown fields", () => {
  const planned = planGraphSequence(source, { fromPath: [2], toPath: [1] });
  assert.deepEqual(planned.previousOrder, ["a", "b", "c"]);
  assert.deepEqual(planned.nextOrder, ["a", "c", "b"]);
  assert.deepEqual(ids(planned.source), planned.nextOrder);
  assert.equal(planned.requiresReplace, true);
  assert.deepEqual(extractSteps(parseYaml(planned.source))[2], extractSteps(parseYaml(source))[1]);
  assert.ok(planned.source.startsWith(source.slice(0, source.indexOf("    - id: a"))));
  assert.ok(planned.source.endsWith("  # preserved footer\n  outputs: {keep: true}\n"));
  assert.equal(ids(source).join(), "a,b,c");
});

test("upstream and downstream targets obey the same immediate-after semantics", () => {
  assert.deepEqual(ids(planGraphSequence(source, { fromPath: [0], toPath: [2] }).source), ["a", "c", "b"]);
  assert.deepEqual(ids(planGraphSequence(source, { fromPath: [2], toPath: [0] }).source), ["b", "c", "a"]);
  const noOp = planGraphSequence(source, { fromPath: [0], toPath: [1] });
  assert.equal(noOp.source, source);
  assert.equal(noOp.changed, false);
  assert.equal(noOp.requiresReplace, false);
});

test("compact nested branches can be reordered without changing neighboring scopes", () => {
  const text = `spec:
  steps:
    - id: loop
      action: control.for_each
      with: {in: [1, 2]}
      do:
        - {id: a, action: control.log, with: {message: a}, custom: keep}
        - {id: b, action: control.log, with: {message: b}}
        - {id: c, action: control.log, with: {message: c}}
      else:
        - {id: untouched, action: control.log}
`;
  const planned = planGraphSequence(text, { fromPath: [0, "do", 0], toPath: [0, "do", 2] });
  const step = extractSteps(parseYaml(planned.source))[0];
  assert.deepEqual(step.do.map((node) => node.id), ["a", "c", "b"]);
  assert.equal(step.do[0].custom, "keep");
  assert.deepEqual(step.else, [{ id: "untouched", action: "control.log" }]);
  const inline = "spec: {steps: [{id: parallel, action: control.parallel, branches: [[{id: a, action: control.log}, {id: b, action: control.log}, {id: c, action: control.log}]]}]}";
  const changed = planGraphSequence(inline, { fromPath: [0, "branches", 0, 0], toPath: [0, "branches", 0, 2] });
  assert.deepEqual(extractSteps(parseYaml(changed.source))[0].branches[0].map((node) => node.id), ["a", "c", "b"]);
});

test("literal block scalars retain exact parsed values after reordering", () => {
  const text = `spec:
  steps:
    - id: a
      action: control.log
      with:
        message: |+
          first


    - id: b
      action: control.log
      with: {message: second}
    - id: c
      action: control.log
`;
  const before = extractSteps(parseYaml(text));
  const planned = planGraphSequence(text, { fromPath: [2], toPath: [0] });
  const after = extractSteps(parseYaml(planned.source));
  assert.deepEqual(after, [before[1], before[2], before[0]]);
});

test("a reorder cannot move a result provider after its existing consumer", () => {
  const text = "spec: {steps: [{id: a, action: file.read}, {id: b, action: control.log, with: {message: '{{ steps.a.result }}'}}, {id: c, action: control.log}]}";
  assert.throws(() => planGraphSequence(text, { fromPath: [2], toPath: [0] }), /b.*依赖 a.*尚未执行/);
  assert.deepEqual(ids(planGraphSequence(text, { fromPath: [2], toPath: [1] }).source), ["a", "c", "b"]);
  const earlier = text.replace("[{id: a", "[{id: start, action: control.log}, {id: a");
  assert.throws(() => planGraphSequence(earlier, { fromPath: [0], toPath: [2] }), /b.*依赖 a.*尚未执行/);
});

test("dependencies inside a moved subtree and raw when conditions remain forward", () => {
  const nested = "spec: {steps: [{id: read-file, action: file.read}, {id: parent, action: control.if, do: [{id: nested, action: control.log, with: {message: '{{ steps[\"read-file\"].result }}'}}]}, {id: c, action: control.log}]}";
  assert.throws(() => planGraphSequence(nested, { fromPath: [2], toPath: [0] }), /nested.*依赖 read-file/);
  const when = "spec: {steps: [{id: a, action: file.read}, {id: b, action: control.log, when: steps.a.result}, {id: c, action: control.log}]}";
  assert.throws(() => planGraphSequence(when, { fromPath: [2], toPath: [0] }), /b.*依赖 a/);
  const condition = "spec: {steps: [{id: a, action: file.read}, {id: b, action: control.if, with: {cond: steps.a.result}, do: []}, {id: c, action: control.log}]}";
  assert.throws(() => planGraphSequence(condition, { fromPath: [2], toPath: [0] }), /b.*依赖 a/);
  const dynamic = "spec: {steps: [{id: a, action: file.read}, {id: b, action: control.log, with: {message: '{{ steps[inputs.key].result }}'}}, {id: c, action: control.log}]}";
  assert.throws(() => planGraphSequence(dynamic, { fromPath: [2], toPath: [0] }), /动态节点结果引用/);
});

test("printed reference-like text does not invent dependencies", () => {
  const text = `spec:
  steps:
    - {id: a, action: control.log}
    - id: b
      action: control.log
      with: {message: '{{ "steps.a.result" }}'}
    - {id: c, action: control.log}
`;
  assert.deepEqual(ids(planGraphSequence(text, { fromPath: [2], toPath: [0] }).source), ["b", "c", "a"]);
});

test("ambiguous IDs, missing nodes, self links and crossed scopes fail without editing", () => {
  assert.throws(() => planGraphSequence(source, { fromPath: [0], toPath: [0] }), /自身/);
  assert.throws(() => planGraphSequence(source, { fromPath: [0], toPath: [1, "do", 0] }), /跨分支/);
  assert.throws(() => planGraphSequence(source, { fromPath: [99], toPath: [0] }), /不存在/);
  assert.throws(() => planGraphSequence(source.replace("id: c\n", "id: a\n"), { fromPath: [0], toPath: [2] }), /重复节点 ID/);
  const parallel = "spec: {steps: [{id: p, action: control.parallel, do: [{id: a, action: control.log}, {id: b, action: control.log}]}]}";
  assert.throws(() => planGraphSequence(parallel, { fromPath: [0, "do", 1], toPath: [0, "do", 0] }), /独立分支/);
});
