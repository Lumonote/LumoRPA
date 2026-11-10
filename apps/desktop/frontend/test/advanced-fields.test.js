import test from "node:test";
import assert from "node:assert/strict";

globalThis.window = {};
const { advancedFormValues, applyAdvancedValues, renderAdvancedFields, RETRY_ERROR_KINDS } = await import("../src/js/editor/advanced-fields.js");
const { emitStep, extractSteps, findStepByPath, mutateStepAtPath, parseYaml } = await import("../src/js/yaml.js");

test("advanced policies preserve zero delays, JSON fallback and unrelated node fields", () => {
  const original = { id: "fetch", action: "http.request", resource: "session", ai: { mode: "off" }, custom: { keep: true } };
  const updated = applyAdvancedValues(original, {
    ...advancedFormValues(original), when: "{{ vars.enabled }}", bind: "response",
    retryEnabled: true, retryTimes: "2", retryInitialMs: "0", retryBackoff: "exponential", retryOn: ["network", "timeout"],
    errorMode: "retry", errorRetryCount: "1", errorRetryIntervalMs: "0", errorOutputEnabled: true, errorOutput: '{"status":"failed","value":null}',
  });
  const actual = extractSteps(parseYaml(`spec:\n  steps:\n${emitStep(updated, 4)}\n`))[0];
  assert.deepEqual(actual, updated);
  assert.equal(original.retry, undefined);
});

test("advanced policy validation rejects lossy or invalid form values", () => {
  const values = { ...advancedFormValues(), retryEnabled: true, errorMode: "retry" };
  for (const patch of [
    { retryTimes: "1.5" }, { retryTimes: "-1" }, { retryTimes: "4294967296" },
    { retryInitialMs: "" }, { retryInitialMs: "9007199254740992" },
    { retryBackoff: "unknown" }, { retryOn: ["unknown"] },
    { errorMode: "ignore" }, { errorRetryCount: "-1" }, { errorRetryIntervalMs: "1e3" },
    { errorOutputEnabled: true, errorOutput: "{bad}" },
  ]) assert.throws(() => applyAdvancedValues({}, { ...values, ...patch }), /有效|整数|范围|支持/);
});

test("turning off retry and fallback clears policies while retaining other fields", () => {
  const original = { id: "a", action: "control.log", when: "yes", bind: "out", retry: { times: 3 }, on_error: { mode: "retry", retry_count: 4, retry_interval_ms: 50, output: "bad" } };
  assert.deepEqual(applyAdvancedValues(original, { ...advancedFormValues(), when: "", bind: "" }), { id: "a", action: "control.log" });
  const continued = applyAdvancedValues(original, { ...advancedFormValues(original), errorMode: "continue" });
  assert.deepEqual(continued.on_error, { mode: "continue", output: "bad" });
});

test("failure outputs retain explicit null, false, zero, empty strings and string-like scalars", () => {
  for (const output of [null, false, 0, "", "null", "true", "2026-11-10", [1, false], { nested: "~" }]) {
    const step = { id: "check", action: "control.log", on_error: { mode: "continue", output } };
    assert.deepEqual(extractSteps(parseYaml(`spec:\n  steps:\n${emitStep(step, 4)}\n`))[0], step);
  }
});

test("advanced form escapes source content and lists runtime retry categories", () => {
  const view = renderAdvancedFields({ when: '"><img src=x>', bind: "a&b", on_error: { mode: "continue", output: "</textarea><script>bad</script>" } });
  assert.match(view, /&lt;img/);
  assert.match(view, /&lt;\/textarea&gt;/);
  assert.doesNotMatch(view, /<script>|<img/);
  for (const kind of RETRY_ERROR_KINDS) assert.ok(view.includes(`data-retry-kind="${kind}"`));
  assert.match(view, /仍失败则终止/);
});

test("path mutation edits only the selected duplicate ID within compact parallel branches", () => {
  const source = `# untouched header\nspec:\n  steps:\n    - id: parallel\n      action: control.parallel\n      branches:\n        - - id: duplicate\n            action: control.log\n            with: {message: first}\n        - - id: duplicate\n            action: control.log\n            with: {message: second}\n    # preserved next step\n    - id: after\n      action: control.log\n`;
  const path = [0, "branches", 1, 0];
  const original = findStepByPath(extractSteps(parseYaml(source)), path);
  const result = mutateStepAtPath(source, path, { ...original, when: "false", with: { message: "changed" } });
  const steps = extractSteps(parseYaml(result));
  assert.equal(findStepByPath(steps, [0, "branches", 0, 0]).with.message, "first");
  assert.equal(findStepByPath(steps, path).with.message, "changed");
  assert.equal(findStepByPath(steps, path).when, "false");
  assert.ok(result.startsWith("# untouched header\n"));
  assert.match(result, /# preserved next step\n {4}- id: after/);
});

test("path mutation supports inline maps and non-id-first nodes without altering adjacent nodes", () => {
  const source = `spec: {steps: [{action: control.log, id: first, with: {message: a}}, {id: second, action: control.log}]}\n`;
  const result = mutateStepAtPath(source, [0], { id: "first", action: "control.log", with: { message: "yes" }, resource: "session" });
  const steps = extractSteps(parseYaml(result));
  assert.equal(steps[0].with.message, "yes");
  assert.equal(steps[0].resource, "session");
  assert.deepEqual(steps[1], { id: "second", action: "control.log" });
  assert.ok(result.endsWith("{id: second, action: control.log}]}\n"));
});

test("path mutation refuses vanished nodes or shared YAML aliases", () => {
  const source = `spec:\n  steps:\n    - &common {id: duplicate, action: control.log}\n    - *common\n`;
  assert.throws(() => mutateStepAtPath(source, [3], {}), /不存在/);
  assert.throws(() => mutateStepAtPath(source, [1], { id: "new", action: "control.log" }), /共享引用/);
});
