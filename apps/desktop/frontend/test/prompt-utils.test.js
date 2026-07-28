import test from "node:test";
import assert from "node:assert/strict";

import { normalizeHumanValue, pickRunningRunIds, formatCountdown } from "../src/js/prompt-utils.js";

test("normalizeHumanValue: input coerces to string, confirm/approve to bool", () => {
  assert.equal(normalizeHumanValue("input", "hello"), "hello");
  assert.equal(normalizeHumanValue("input", null), "");
  assert.equal(normalizeHumanValue("confirm", true), true);
  assert.equal(normalizeHumanValue("approve", false), false);
  for (const yes of ["y", "YES", "true", "1", "是"]) {
    assert.equal(normalizeHumanValue("confirm", yes), true, yes);
  }
  for (const no of ["n", "no", "", "0", "否"]) {
    assert.equal(normalizeHumanValue("approve", no), false, no);
  }
});

test("pickRunningRunIds filters running runs with ids", () => {
  const runs = [
    { id: "a", state: "running" },
    { id: "b", state: "success" },
    { id: null, state: "running" },
    null,
  ];
  assert.deepEqual(pickRunningRunIds(runs), ["a"]);
  assert.deepEqual(pickRunningRunIds(undefined), []);
});

test("formatCountdown clamps negatives and formats minutes", () => {
  assert.equal(formatCountdown(-5), "0秒");
  assert.equal(formatCountdown(1000), "1秒");
  assert.equal(formatCountdown(61_000), "1分01秒");
});
