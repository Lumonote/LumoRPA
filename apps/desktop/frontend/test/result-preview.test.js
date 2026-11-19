import test from "node:test";
import assert from "node:assert/strict";
import { previewResult } from "../src/js/result-preview.js";

test("JSON previews preserve valid falsy values", () => {
  for (const value of [false, 0, "", null]) assert.equal(previewResult(value).text, JSON.stringify(value));
});

test("preview traversal does not visit an entire large result", () => {
  let reads = 0;
  const value = Object.fromEntries(Array.from({ length: 10000 }, (_, index) => [index, index]));
  const guarded = new Proxy(value, { get(target, key) { reads++; return target[key]; } });
  const result = previewResult(guarded, { maxItems: 20 });
  assert.ok(result.truncated);
  assert.ok(reads <= 20);
  assert.ok(result.text.length < 2000);
});
