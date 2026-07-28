import assert from "node:assert/strict";
import test from "node:test";

import {
  emitStep,
  extractSteps,
  findStepByPath,
  parseYaml,
  pathKey,
  stepIdChain,
  walkSteps,
} from "../src/js/yaml.js";

const parallelFlow = `apiVersion: lumorpa.io/v1
kind: Flow
metadata:
  id: branch-test
spec:
  steps:
    - id: parallel
      action: control.parallel
      branches:
        - - id: a_wait
            action: control.sleep
            with: { ms: 1 }
          - id: a_store
            action: control.set_var
            with: { name: a_done, value: true }
        - - id: b_wait
            action: control.sleep
            with: { ms: 1 }
    - id: joined
      action: control.log
      with: { message: done }
`;

test("parseYaml preserves control.parallel branches as nested step lists", () => {
  const ast = parseYaml(parallelFlow);
  const parallel = extractSteps(ast)[0];

  assert.equal(parallel.id, "parallel");
  assert.equal(parallel.branches.length, 2);
  assert.equal(parallel.branches[0][0].id, "a_wait");
  assert.equal(parallel.branches[0][1].id, "a_store");
  assert.equal(parallel.branches[1][0].id, "b_wait");
});

test("walkSteps, findStepByPath, and stepIdChain understand branch paths", () => {
  const ast = parseYaml(parallelFlow);
  const steps = extractSteps(ast);
  const visited = [];

  walkSteps(steps, (step, info) => {
    visited.push([step.id, pathKey(info.path), info.depth]);
  });

  assert.deepEqual(visited, [
    ["parallel", "0", 0],
    ["a_wait", "0/:branches/0/0", 1],
    ["a_store", "0/:branches/0/1", 1],
    ["b_wait", "0/:branches/1/0", 1],
    ["joined", "1", 0],
  ]);
  assert.equal(findStepByPath(steps, [0, "branches", 0, 1]).id, "a_store");
  assert.equal(stepIdChain(steps, [0, "branches", 1, 0]), "parallel/branch[1]/b_wait");
});

test("emitStep writes branches that parse back to the same branch shape", () => {
  const source = emitStep(
    {
      id: "parallel",
      action: "control.parallel",
      branches: [
        [{ id: "first", action: "control.log", with: { message: "a" } }],
        [{ id: "second", action: "control.log", with: { message: "b" } }],
      ],
    },
    4,
  );
  const ast = parseYaml(`spec:\n  steps:\n${source}\n`);
  const parallel = extractSteps(ast)[0];

  assert.match(source, /branches:\n\s+-\n\s+- id: first/);
  assert.equal(parallel.branches.length, 2);
  assert.equal(parallel.branches[0][0].id, "first");
  assert.equal(parallel.branches[1][0].with.message, "b");
});
