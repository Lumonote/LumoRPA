import test from "node:test";
import assert from "node:assert/strict";
import { getRunInstanceIndex, runInstancePage, runResultIdentity } from "../src/js/run-instances.js";

const ast = { spec: { steps: [{ id: "loop", action: "control.for_each", do: [{ id: "read" }, { id: "save" }] }] } };
const context = { flowPath: "/flows/a.yaml", source: "source-a", runId: "run-a" };

test("run result identity releases progress/state references and retains exact selected scope", () => {
  const full = { ...context, selectedRuntimePath: "gate/else/loop", previous: { retained: true }, stepDelta: { path: "loop[3]/read" }, stepStates: { read: { state: "ok" } } };
  assert.deepEqual(runResultIdentity(full, "run-a"), { ...context, selectedRuntimePath: "gate/else/loop" });
  assert.deepEqual(Object.keys(runResultIdentity(full, "run-a")).sort(), ["flowPath", "runId", "selectedRuntimePath", "source"]);
  assert.equal(runResultIdentity(full, "other-run"), null);
  assert.equal(runResultIdentity(null, "run-a"), null);
});

test("instance index retains every row and exact retry while sharing summaries", () => {
  const steps = [
    { seq: 1, path: "loop[0]/read", attempt: 1 }, { seq: 2, path: "loop[0]/read", attempt: 2 },
    { seq: 3, path: "loop[0]/save" }, { seq: 4, path: "loop[1]/read" }, { seq: 5, path: "loop[1]/save" },
  ];
  const index = getRunInstanceIndex(steps, context, ast);
  assert.deepEqual(index.get("loop/read"), [0, 1, 3]);
  assert.deepEqual(index.get("loop/save"), [2, 4]);
  assert.equal(index.get("loop/read").at(-1), 3);
  assert.equal(steps[index.get("loop/read")[1]].attempt, 2);
  assert.equal(getRunInstanceIndex(steps, { ...context }, ast), index, "identity-equivalent result contexts reuse the index");
  assert.deepEqual(steps.map((step) => step.seq), [1, 2, 3, 4, 5]);
});

test("instance index isolates source, flow, run, AST, selection scope and replacement rows", () => {
  const steps = [{ path: "loop[1]/read" }];
  let prior = getRunInstanceIndex(steps, context, ast);
  for (const next of [{ ...context, source: "source-b" }, { ...context, flowPath: "/flows/b.yaml" }, { ...context, runId: "run-b" }]) {
    const index = getRunInstanceIndex(steps, next, ast);
    assert.notEqual(index, prior); prior = index;
  }
  const literalAst = { spec: { steps: [{ id: "loop[1]", action: "control.log", do: [{ id: "read" }] }] } };
  const literal = getRunInstanceIndex(steps, context, literalAst);
  assert.deepEqual(literal.get("loop[1]/read"), [0]);
  const scopedAst = { spec: { steps: [{ id: "gate", action: "control.if", else: ast.spec.steps }] } };
  const scoped = getRunInstanceIndex(steps, { ...context, selectedRuntimePath: "gate/else/loop" }, scopedAst);
  assert.deepEqual(scoped.get("gate/else/loop/read"), [0]);
  assert.notEqual(getRunInstanceIndex([...steps], context, ast), prior);
  steps.push({ path: "loop[2]/save" });
  assert.deepEqual(getRunInstanceIndex(steps, context, ast).get("loop/save"), [1]);
});

test("instance paging reads only its bounded window after the one-time index build", () => {
  const count = 100000;
  let reads = 0;
  const steps = Array.from({ length: count }, (_, row) => ({ get path() { reads++; return `loop[${row}]/read`; } }));
  const index = getRunInstanceIndex(steps, context, ast);
  assert.equal(reads, count);
  const positions = index.get("loop/read");
  assert.equal(positions.length, count);
  assert.equal(getRunInstanceIndex(steps, context, ast), index);
  const tail = runInstancePage(positions, count - 1);
  assert.deepEqual(tail.indices, Array.from({ length: 200 }, (_, offset) => 99800 + offset));
  assert.equal(tail.previousIndex, 99600);
  assert.equal(tail.nextIndex, undefined);
  const middle = runInstancePage(positions, 234);
  assert.equal(middle.start, 200); assert.equal(middle.previousIndex, 0); assert.equal(middle.nextIndex, 400);
  assert.equal(reads, count, "cached selections and paging cannot scan or canonicalize summaries again");
  assert.equal(runInstancePage(positions, 100, 10000).indices.length, 200);
  assert.equal(runInstancePage([], 0).indices.length, 0);
});
