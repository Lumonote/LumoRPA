import test from "node:test";
import assert from "node:assert/strict";

import { diffLines } from "../src/js/diff.js";

test("identical inputs produce no adds or removes", () => {
  const { rows, added, removed } = diffLines("a\nb\nc", "a\nb\nc");
  assert.equal(added, 0);
  assert.equal(removed, 0);
  assert.deepEqual(rows.map((r) => r.type), ["same", "same", "same"]);
});

test("a changed line counts one del and one add in place", () => {
  const { rows, added, removed } = diffLines("a\nb\nc", "a\nB\nc");
  assert.equal(added, 1);
  assert.equal(removed, 1);
  assert.deepEqual(
    rows.map((r) => `${r.type}:${r.text}`),
    ["same:a", "del:b", "add:B", "same:c"],
  );
});

test("pure insertion keeps surrounding lines as same", () => {
  const { rows, added, removed } = diffLines("a\nc", "a\nb\nc");
  assert.equal(added, 1);
  assert.equal(removed, 0);
  assert.deepEqual(
    rows.map((r) => `${r.type}:${r.text}`),
    ["same:a", "add:b", "same:c"],
  );
});

test("empty old text marks every line added", () => {
  const { added, removed, rows } = diffLines("", "x\ny");
  assert.equal(removed, 1); // the single empty line is replaced
  assert.equal(added, 2);
  assert.equal(rows.filter((r) => r.type === "add").length, 2);
});

test("oversized middles fall back to block replace but still count", () => {
  const oldText = Array.from({ length: 50 }, (_, i) => `old${i}`).join("\n");
  const newText = Array.from({ length: 60 }, (_, i) => `new${i}`).join("\n");
  const { added, removed } = diffLines(oldText, newText, { maxLcs: 10 });
  assert.equal(removed, 50);
  assert.equal(added, 60);
});
