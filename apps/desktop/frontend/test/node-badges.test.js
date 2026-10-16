import assert from "node:assert/strict";
import test from "node:test";

import { renderStepFoot, stepBadges } from "../src/js/editor/node-badges.js";

test("a step with no control metadata has no footer", () => {
  assert.deepEqual(stepBadges({ id: "a", action: "file.read" }), []);
  assert.equal(renderStepFoot({ id: "a", action: "file.read" }), "");
});

test("retry policy is badged with its attempt count", () => {
  assert.deepEqual(stepBadges({ retry: { times: 3 } }), [
    { kind: "retry", text: "retry × 3" },
  ]);
});

test("retry defaults to one attempt when times is absent", () => {
  assert.deepEqual(stepBadges({ retry: {} }), [{ kind: "retry", text: "retry × 1" }]);
});

test("an explicit interrupt is not badged", () => {
  // It behaves exactly like the absent field, so a badge would advertise a
  // difference that does not exist.
  assert.deepEqual(stepBadges({ on_error: { mode: "interrupt" } }), []);
});

test("continue mode is badged as ignoring errors", () => {
  assert.deepEqual(stepBadges({ on_error: { mode: "continue" } }), [
    { kind: "on-error", mode: "continue", text: "忽略错误" },
  ]);
});

test("retry mode is badged with its retry count", () => {
  assert.deepEqual(stepBadges({ on_error: { mode: "retry", retry_count: 2 } }), [
    { kind: "on-error", mode: "retry", text: "重试 × 2" },
  ]);
});

test("retry mode without an explicit count reads as zero", () => {
  assert.deepEqual(stepBadges({ on_error: { mode: "retry" } }), [
    { kind: "on-error", mode: "retry", text: "重试 × 0" },
  ]);
});

test("a step can carry both a retry policy and failure handling", () => {
  const badges = stepBadges({
    retry: { times: 2 },
    on_error: { mode: "continue" },
  });
  assert.deepEqual(
    badges.map((b) => b.kind),
    ["retry", "on-error"],
    "policy badge comes first; failure handling is the outer decision",
  );
});

test("badge markup escapes values so a mode cannot inject markup", () => {
  const html = renderStepFoot({ on_error: { mode: '"><img src=x>' } });
  assert.ok(!html.includes("<img"), `raw markup leaked: ${html}`);
  assert.ok(html.includes("&quot;&gt;&lt;img"), `expected escaped mode in: ${html}`);
});

test("footer markup wraps badges in the expected container", () => {
  const html = renderStepFoot({ on_error: { mode: "retry", retry_count: 1 } });
  assert.equal(
    html,
    '<div class="graph-node-foot">' +
      '<span class="badge on-error retry">重试 × 1</span>' +
      "</div>",
  );
});

test("non-object input yields no badges rather than throwing", () => {
  // The canvas renders whatever the parser produced; a malformed step must not
  // take down the whole graph view.
  for (const bad of [null, undefined, "x", 42]) {
    assert.deepEqual(stepBadges(bad), [], `input ${String(bad)}`);
  }
});
