import assert from "node:assert/strict";
import test from "node:test";
import { graphResultExpression, planGraphConnection } from "../src/js/editor/graph-connections.js";
import { extractSteps, findStepByPath, parseYaml } from "../src/js/yaml.js";

const source = `# preserved header
metadata: {id: graph-demo}
spec:
  steps:
    - id: read
      action: file.read
      with: {path: data.json}
    - id: log
      action: control.log
      resource: session
      custom: {preserve: [true, 0]}
      retry: {times: 2}
      with: {message: "", keep: unchanged}
    # preserved sibling comment
    - id: after
      action: control.log
      with: {message: end}
`;
const connection = { fromPath: [0], toPath: [1], input: "message" };
const read = (text, path = [1]) => findStepByPath(extractSteps(parseYaml(text)), path);

test("data link edits the target parameter while retaining policies, custom fields and surrounding YAML", () => {
  const planned = planGraphConnection(source, connection);
  assert.equal(planned.changed, true);
  assert.equal(planned.requiresReplace, false);
  assert.equal(planned.expression, "{{ steps.read.result }}");
  assert.equal(planned.previousValue, "");
  assert.deepEqual(read(planned.source), { ...read(source), with: { message: planned.expression, keep: "unchanged" } });
  assert.equal(planned.source.slice(0, planned.source.indexOf("    - id: log")), source.slice(0, source.indexOf("    - id: log")));
  assert.equal(planned.source.slice(planned.source.indexOf("    # preserved sibling comment")), source.slice(source.indexOf("    # preserved sibling comment")));
  assert.equal(read(source).with.message, "");
});

test("replacement confirmation preserves false, zero and structured values as meaningful inputs", () => {
  for (const value of [false, 0, [], {}, "original", "{{ inputs.message }}"]) {
    const text = `spec: {steps: [{id: read, action: file.read}, {id: log, action: control.log, with: {message: ${JSON.stringify(value)}}}]}`;
    const planned = planGraphConnection(text, connection);
    assert.equal(planned.requiresReplace, true, JSON.stringify(value));
    assert.deepEqual(planned.previousValue, value);
    assert.equal(read(planned.source).with.message, planned.expression);
  }
});

test("reconnecting an identical full reference is a byte-preserving no-op", () => {
  for (const value of ["{{ steps.read.result }}", "{{steps.read.result}}", "{{ steps['read'].result }}", '{{ steps["read"].result }}']) {
    const text = `spec: {steps: [{id: read, action: file.read}, {id: log, action: control.log, with: {message: ${JSON.stringify(value)}}}]}`;
    const planned = planGraphConnection(text, connection);
    assert.equal(planned.changed, false);
    assert.equal(planned.requiresReplace, false);
    assert.equal(planned.source, text);
  }
});

test("disconnect removes only an exact source result parameter and retains all other fields", () => {
  const linked = planGraphConnection(source, connection).source;
  const disconnected = planGraphConnection(linked, { ...connection, disconnect: true });
  assert.equal(disconnected.changed, true);
  assert.equal(disconnected.requiresReplace, false);
  assert.deepEqual(read(disconnected.source), { ...read(source), with: { keep: "unchanged" } });
  for (const value of ["prefix {{ steps.read.result }}", "{{ steps.read.result.name }}", "{{ steps.after.result }}", "{{ steps.read.result | tojson }}"]) {
    const text = `spec: {steps: [{id: read, action: file.read}, {id: log, action: control.log, with: {message: ${JSON.stringify(value)}}}]}`;
    assert.throws(() => planGraphConnection(text, { ...connection, disconnect: true }), /完整的结果绑定/);
  }
});

test("links reject self references, future dependencies and cross-branch scopes", () => {
  assert.throws(() => planGraphConnection(source, { ...connection, toPath: [0] }), /自身/);
  assert.throws(() => planGraphConnection(source, { ...connection, fromPath: [1], toPath: [0] }), /尚未执行|循环/);
  const nested = `spec:
  steps:
    - id: parent
      action: control.parallel
      branches:
        - - {id: a, action: control.log}
          - {id: b, action: control.log, with: {message: ""}}
        - - {id: c, action: control.log}
`;
  const allowed = planGraphConnection(nested, { fromPath: [0, "branches", 0, 0], toPath: [0, "branches", 0, 1], input: "message" });
  assert.equal(read(allowed.source, [0, "branches", 0, 1]).with.message, "{{ steps.a.result }}");
  assert.throws(() => planGraphConnection(nested, { fromPath: [0, "branches", 0, 0], toPath: [0, "branches", 1, 0], input: "message" }), /跨分支/);
  assert.throws(() => planGraphConnection(nested, { fromPath: [0], toPath: [0, "branches", 0, 0], input: "message" }), /嵌套作用域/);
});

test("ambiguous IDs, invalid paths and dangerous parameter names never produce an edit", () => {
  const duplicate = source.replace("id: after", "id: read");
  assert.throws(() => planGraphConnection(duplicate, connection), /重复/);
  for (const input of ["", " ", "__proto__", "constructor", "prototype"]) assert.throws(() => planGraphConnection(source, { ...connection, input }), /参数名称/);
  for (const path of [[], null, [-1], [0.5], [99]]) assert.throws(() => planGraphConnection(source, { ...connection, fromPath: path }), /路径|不存在/);
  assert.throws(() => planGraphConnection("spec: [broken", connection), /YAML/);
  assert.throws(() => planGraphConnection("spec: {steps: wrong}", connection), /步骤结构/);
  assert.throws(() => planGraphConnection(source.replace('with: {message: "", keep: unchanged}', "with: []"), connection), /映射结构/);
});

test("hyphenated step IDs use template bracket access rather than a subtraction expression", () => {
  const text = source.replace("id: read", "id: read-file");
  const planned = planGraphConnection(text, connection);
  assert.equal(planned.expression, '{{ steps["read-file"].result }}');
  assert.equal(read(planned.source).with.message, planned.expression);
  assert.equal(planGraphConnection(planned.source, connection).changed, false);
  assert.equal(Object.hasOwn(read(planGraphConnection(planned.source, { ...connection, disconnect: true }).source).with, "message"), false);
  assert.throws(() => graphResultExpression("read.file"), /ID/);
});

test("shared target aliases fail before any source can be applied", () => {
  const text = `spec:\n  steps:\n    - {id: read, action: file.read}\n    - &common {id: log, action: control.log, with: {message: ""}}\n    - *common\n`;
  assert.throws(() => planGraphConnection(text, connection), /共享引用/);
});

test("legacy parallel do entries are isolated branches, not sequential data sources", () => {
  const text = "spec: {steps: [{id: parallel, action: control.parallel, do: [{id: a, action: file.read}, {id: b, action: control.log}]}]}";
  assert.throws(() => planGraphConnection(text, { fromPath: [0, "do", 0], toPath: [0, "do", 1], input: "message" }), /独立分支/);
});
