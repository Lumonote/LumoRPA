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

// Failure handling has to survive a canvas round-trip. If the emitter drops
// `on_error`, editing any other field on the node silently resets the step to
// fail-fast — and a dropped `output` turns a downstream
// `{{ steps.x.result.* }}` into an undefined reference.

test("emitStep round-trips a continue fallback with its default output", () => {
  const source = emitStep(
    {
      id: "boom",
      action: "file.read",
      on_error: { mode: "continue", output: { status: "degraded" } },
    },
    4,
  );
  const step = extractSteps(parseYaml(`spec:\n  steps:\n${source}\n`))[0];

  assert.equal(step.on_error.mode, "continue");
  assert.equal(step.on_error.output.status, "degraded");
});

test("emitStep round-trips retry mode with its count and interval", () => {
  const source = emitStep(
    {
      id: "flaky",
      action: "http.request",
      on_error: { mode: "retry", retry_count: 3, retry_interval_ms: 250 },
    },
    4,
  );
  const step = extractSteps(parseYaml(`spec:\n  steps:\n${source}\n`))[0];

  assert.equal(step.on_error.mode, "retry");
  assert.equal(step.on_error.retry_count, 3);
  assert.equal(step.on_error.retry_interval_ms, 250);
});

test("emitStep omits an on_error block that changes nothing", () => {
  // Re-saving an untouched node must not grow the file with a no-op block.
  const source = emitStep(
    { id: "plain", action: "file.read", on_error: { mode: "interrupt" } },
    4,
  );
  assert.ok(!source.includes("on_error"), `unexpected block in:\n${source}`);
});

test("emitStep keeps an explicit default mode when it carries an output", () => {
  // `mode: interrupt` alone is a no-op, but paired with `output` it is not:
  // the fallback still has to reach the downstream reference.
  const source = emitStep(
    {
      id: "boom",
      action: "file.read",
      on_error: { mode: "interrupt", output: "n/a" },
    },
    4,
  );
  const step = extractSteps(parseYaml(`spec:\n  steps:\n${source}\n`))[0];

  assert.match(source, /on_error:/);
  assert.equal(step.on_error.output, "n/a");
});

test("emitStep keeps both a retry policy and failure handling", () => {
  const source = emitStep(
    {
      id: "both",
      action: "http.request",
      retry: { times: 2, backoff: "fixed", initial_ms: 100 },
      on_error: { mode: "continue" },
    },
    4,
  );
  const step = extractSteps(parseYaml(`spec:\n  steps:\n${source}\n`))[0];

  assert.equal(step.retry.times, 2);
  assert.equal(step.on_error.mode, "continue");
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
