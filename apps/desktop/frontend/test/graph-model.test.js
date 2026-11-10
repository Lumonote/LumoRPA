import test from "node:test";
import assert from "node:assert/strict";
import { buildGraphModel, graphPortPoint, GRAPH_INPUT_ROW_HEIGHT } from "../src/js/editor/graph-model.js";
import { extractSteps, parseYaml } from "../src/js/yaml.js";

const step = (id, extra = {}) => ({ id, action: "control.log", with: {}, ...extra });

test("graph model arranges execution horizontally and retains every parameter port", () => {
  const withFields = Object.fromEntries(Array.from({ length: 12 }, (_, i) => [`field${i}`, i]));
  const model = buildGraphModel([step("first", { with: withFields }), step("second")]);
  assert.equal(model.nodes.get("0").inputs.length, 12);
  assert.deepEqual(model.nodes.get("0").inputs.at(-1), { name: "field11", value: 11, row: 11, type: "number", required: false });
  assert.equal(model.nodes.get("0").output, "result");
  assert.ok(model.nodes.get("1").x > model.nodes.get("0").x + 260);
  assert.equal(model.nodes.get("1").y, 0);
  assert.equal(model.nodes.get("0").h - model.nodes.get("1").h, 11 * GRAPH_INPUT_ROW_HEIGHT);
  assert.deepEqual(model.edges.map(({ from, to, kind }) => ({ from, to, kind })), [{ from: "0", to: "1", kind: "sequence" }]);
});

test("nested control blocks have explicit branch labels and nonoverlapping lanes", () => {
  const model = buildGraphModel([step("gate", {
    do: [step("yes"), step("next")], else: [step("no")],
    catch: [step("recover")], finally: [step("cleanup")],
    branches: [[step("parallel-a")], [], [step("parallel-b")]],
  }), step("after")]);
  const parent = model.nodes.get("0");
  assert.deepEqual(parent.branches.map((branch) => branch.label), ["执行", "否则", "捕获异常", "最终执行", "并行分支 1", "并行分支 3"]);
  assert.deepEqual(parent.branches.map((branch) => branch.row), [0, 1, 2, 3, 4, 5]);
  assert.equal(model.edges.filter((edge) => edge.kind === "branch").length, 6);
  assert.ok(model.edges.some((edge) => edge.kind === "sequence" && edge.from === "0/:do/0" && edge.to === "0/:do/1"));
  const branches = parent.branches.map((branch) => model.nodes.get(branch.firstKey));
  assert.ok(branches[0].y >= parent.y + parent.h);
  for (let i = 1; i < branches.length; i++) assert.ok(branches[i].y >= branches[i - 1].y + branches[i - 1].h);
  assert.equal(model.nodes.get("0/:branches/2/0").layoutKey, "id:gate/branches:2/id:parallel-b");
  assert.equal(model.nodes.get("0/:branches/2/0").idChain, "gate/branch[2]/parallel-b");
  assert.ok(model.nodes.get("1").x > model.nodes.get("0/:do/1").x + 260);
  assert.equal(parent.h, 142 + 6 * GRAPH_INPUT_ROW_HEIGHT);
  assert.equal(model.edges.find((edge) => edge.kind === "branch" && edge.to === "0/:finally/0").branchIndex, 3);
});

test("port geometry follows complete input and branch rows and result footer", () => {
  const node = buildGraphModel([step("gate", { with: { a: 1, b: 2 }, do: [step("child")] })]).nodes.get("0");
  assert.deepEqual(graphPortPoint(node), { x: 0, y: 58 });
  assert.deepEqual(graphPortPoint(node, { direction: "output" }), { x: 260, y: 58 });
  assert.deepEqual(graphPortPoint(node, { kind: "data", input: "b" }), { x: 0, y: 114 });
  assert.deepEqual(graphPortPoint(node, { direction: "output", kind: "branch", branchIndex: 0 }), { x: 260, y: 142 });
  assert.deepEqual(graphPortPoint(node, { direction: "output", kind: "data" }), { x: 260, y: node.h - 21 });
});

test("saved positions follow stable IDs through insert, reorder and branch scopes", () => {
  const layouts = { "id:alpha": { x: 50, y: 90 }, "id:beta": { x: 700, y: 90 }, "id:gate/else/id:child": { x: 90, y: 900 }, 0: { x: 999, y: 999 } };
  const model = buildGraphModel([step("new"), step("beta"), step("alpha"), step("gate", { do: [step("child")], else: [step("child")] })], { layouts });
  assert.deepEqual([model.nodes.get("2").x, model.nodes.get("2").y], [50, 90]);
  assert.equal(model.nodes.get("1").x, 700);
  assert.equal(model.nodes.get("0").x, 0, "old numeric path positions must never be reused");
  assert.equal(model.nodes.get("3/:else/0").y, 900);
  assert.notEqual(model.nodes.get("3/:do/0").y, 900);
});

test("ambiguous sibling IDs and descendants cannot inherit an unrelated saved layout", () => {
  const model = buildGraphModel([step("dup", { do: [step("nested")] }), step("dup"), { action: "control.log" }], { layouts: { "id:dup": { x: 999, y: 999 }, "id:dup/do/id:nested": { x: 999, y: 999 } } });
  assert.equal(model.nodes.get("0").layoutKey, null);
  assert.equal(model.nodes.get("1").layoutKey, null);
  assert.equal(model.nodes.get("0/:do/0").layoutKey, null);
  assert.equal(model.nodes.get("2").layoutKey, null);
  assert.equal(model.nodes.get("0").x, 0);
});

test("data dependencies resolve unique IDs across nested input objects without inventing ports", () => {
  const model = buildGraphModel([step("read-file"), step("write", { with: {
    content: '{{ steps["read-file"].result.text }} / {{steps["read-file"].result.text}}',
    options: { files: ["{{ steps['read-file'].result }}", "steps.read-file.result is ordinary text"] },
    ignored: "{{ steps.missing.result }} {{ steps.write.result }}",
  } })]);
  const data = model.edges.filter((edge) => edge.kind === "data");
  assert.deepEqual(data.map(({ from, to, input, inputPath }) => ({ from, to, input, inputPath })), [
    { from: "0", to: "1", input: "content", inputPath: "content" },
    { from: "0", to: "1", input: "options", inputPath: "options.files[0]" },
  ]);
  assert.equal(new Set(model.edges.map((edge) => edge.id)).size, model.edges.length);
});

test("template subtraction and printed reference-like literals do not create false data links", () => {
  const model = buildGraphModel([step("read-file"), step("read"), step("write", { with: {
    subtraction: "{{ steps.read-file.result }}",
    literal: '{{ "steps.read.result" }}',
    nestedLiteral: "{{ 'steps[\"read-file\"].result' }}",
    mixed: '{{ "steps.missing.result" ~ steps.read.result }}',
  } })]);
  const data = model.edges.filter((edge) => edge.kind === "data");
  assert.deepEqual(data.map(({ from, to, input }) => ({ from, to, input })), [{ from: "1", to: "2", input: "mixed" }]);
});

test("schema input ports include missing properties, types and requirements without changing source values", () => {
  const empty = step("new", { action: "custom" });
  const configured = step("configured", { action: "custom", with: { unknown: "keep", count: 0 } });
  const schemas = new Map([["custom", { properties: { count: { type: "integer" }, text: { type: "string" }, flag: { type: "boolean" } }, required: ["count", "text"] }]]);
  const model = buildGraphModel([empty, configured], { schemas });
  assert.deepEqual(model.nodes.get("0").inputs, [
    { name: "count", value: undefined, row: 0, type: "integer", required: true },
    { name: "text", value: undefined, row: 1, type: "string", required: true },
    { name: "flag", value: undefined, row: 2, type: "boolean", required: false },
  ]);
  assert.deepEqual(model.nodes.get("1").inputs.map(({ name, value }) => ({ name, value })), [
    { name: "unknown", value: "keep" }, { name: "count", value: 0 }, { name: "text", value: undefined }, { name: "flag", value: undefined },
  ]);
  assert.equal(model.nodes.get("1").h - model.nodes.get("0").h, GRAPH_INPUT_ROW_HEIGHT);
  assert.deepEqual(empty.with, {});
  assert.deepEqual(configured.with, { unknown: "keep", count: 0 });
});

test("branch lane placement accounts for schema-only input height", () => {
  const properties = Object.fromEntries(Array.from({ length: 8 }, (_, index) => [`p${index}`, { type: "string" }]));
  const model = buildGraphModel([step("parent", { do: [step("child")] })], { schemas: new Map([["control.log", { properties }]]) });
  const parent = model.nodes.get("0");
  assert.equal(parent.h, 72 + (8 + 1) * GRAPH_INPUT_ROW_HEIGHT + 42);
  assert.ok(model.nodes.get("0/:do/0").y >= parent.h + 72);
  assert.equal(graphPortPoint(parent, { direction: "output", kind: "branch" }).y, 72 + 8 * GRAPH_INPUT_ROW_HEIGHT + 14);
});

test("runtime paths match conditional and try scopes while loop iterations remain canonical", () => {
  const model = buildGraphModel([
    step("condition", { action: "control.if", do: [step("read")], else: [step("read")] }),
    step("attempt", { action: "control.try", do: [step("read")], catch: [step("read")], finally: [step("read")] }),
    step("loop", { action: "control.for_each", do: [step("condition", { action: "control.if", do: [step("read")] })] }),
  ]);
  assert.equal(model.nodes.get("0/:do/0").runtimePath, "condition/do/read");
  assert.equal(model.nodes.get("0/:else/0").runtimePath, "condition/else/read");
  assert.equal(model.nodes.get("1/:do/0").runtimePath, "attempt/try/read");
  assert.equal(model.nodes.get("1/:catch/0").runtimePath, "attempt/catch/read");
  assert.equal(model.nodes.get("1/:finally/0").runtimePath, "attempt/finally/read");
  assert.equal(model.nodes.get("2/:do/0/:do/0").runtimePath, "loop/condition/do/read");
  assert.equal(model.nodes.get("2/:do/0/:do/0").idChain, "loop/condition/read");
});

test("parallel branches and legacy do entries use independent VM branch paths", () => {
  const model = buildGraphModel([
    step("parallel", { action: "control.parallel", branches: [[step("read")], [step("read")]], do: [step("ignored")] }),
    step("legacy", { action: "control.parallel", do: [step("first"), step("second")] }),
  ]);
  assert.equal(model.nodes.get("0/:branches/0/0").runtimePath, "parallel/branch[0]/read");
  assert.equal(model.nodes.get("0/:branches/1/0").runtimePath, "parallel/branch[1]/read");
  assert.equal(model.nodes.has("0/:do/0"), false, "branches takes precedence over do in the VM");
  assert.equal(model.nodes.get("1/:do/0").runtimePath, "legacy/branch[0]/first");
  assert.equal(model.nodes.get("1/:do/1").runtimePath, "legacy/branch[1]/second");
  assert.equal(model.edges.some((edge) => edge.kind === "sequence" && edge.from === "1/:do/0"), false);
  assert.deepEqual(model.nodes.get("1").branches.map((branch) => branch.label), ["并行分支 1", "并行分支 2"]);
});

test("data edges omit globally ambiguous IDs instead of choosing a wrong branch", () => {
  const model = buildGraphModel([step("gate", { do: [step("read")], else: [step("read")] }), step("use", { with: { text: "{{ steps.read.result }}" } })]);
  assert.equal(model.edges.filter((edge) => edge.kind === "data").length, 0);
});

test("bounds include negative saved coordinates and reject invalid positions", () => {
  const model = buildGraphModel([step("left"), step("right")], { layouts: { "id:left": { x: -300, y: -200 }, "id:right": { x: Infinity, y: 10 } } });
  assert.deepEqual(model.bounds, { x: -300, y: -200, width: 916, height: 342 });
  assert.deepEqual(buildGraphModel([]).bounds, { x: 0, y: 0, width: 0, height: 0 });
  assert.deepEqual(buildGraphModel(null).bounds, { x: 0, y: 0, width: 0, height: 0 });
});

test("recursive YAML step and parameter aliases project once without recursing forever", () => {
  const ast = parseYaml(`spec:
  steps:
    - {id: read, action: file.read}
    - &self
      id: recursive
      action: control.if
      with: &parameters
        value: "{{ steps.read.result }}"
        nested: *parameters
      do:
        - *self
    - {id: after, action: control.log}
`);
  const model = buildGraphModel(extractSteps(ast));
  assert.equal(model.nodes.size, 4);
  assert.equal(model.nodes.get("1").recursiveReference, false);
  assert.equal(model.nodes.get("1/:do/0").recursiveReference, true);
  assert.equal(model.nodes.get("1/:do/0").layoutKey, null);
  assert.deepEqual(model.nodes.get("1/:do/0").branches, []);
  assert.equal(model.nodes.get("2").step.id, "after");
  assert.ok(model.edges.some((edge) => edge.kind === "data" && edge.from === "0" && edge.to === "1" && edge.input === "nested"));
  assert.ok(Number.isFinite(model.bounds.width) && Number.isFinite(model.bounds.height));
});

test("the same nonrecursive YAML alias expands independently in distinct branches", () => {
  const ast = parseYaml(`spec:
  steps:
    - id: gate
      action: control.if
      do:
        - &shared
          id: shared
          action: control.if
          do:
            - {id: leaf, action: control.log}
      else:
        - *shared
`);
  const model = buildGraphModel(extractSteps(ast));
  assert.equal(model.nodes.size, 5);
  for (const path of ["0/:do/0", "0/:else/0"]) {
    assert.equal(model.nodes.get(path).recursiveReference, false);
    assert.equal(model.nodes.get(`${path}/:do/0`).step.id, "leaf");
    assert.equal(model.nodes.get(path).branches.length, 1);
  }
  assert.notEqual(model.nodes.get("0/:do/0").layoutKey, model.nodes.get("0/:else/0").layoutKey);
});
