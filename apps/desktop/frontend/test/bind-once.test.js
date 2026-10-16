import test from "node:test";
import assert from "node:assert/strict";

import { bindOnce } from "../src/js/bind-once.js";

test("attaches exactly once per element+key across repeated calls", () => {
  const el = {};
  let attached = 0;
  assert.equal(bindOnce(el, "drop", () => attached++), true);
  assert.equal(bindOnce(el, "drop", () => attached++), false);
  assert.equal(bindOnce(el, "drop", () => attached++), false);
  assert.equal(attached, 1);
});

test("a different key on the same element attaches again", () => {
  const el = {};
  let attached = 0;
  bindOnce(el, "a", () => attached++);
  bindOnce(el, "b", () => attached++);
  assert.equal(attached, 2);
});

test("a fresh element with the same key attaches independently", () => {
  let attached = 0;
  bindOnce({}, "drop", () => attached++);
  bindOnce({}, "drop", () => attached++);
  assert.equal(attached, 2);
});

test("null element is a safe no-op", () => {
  let attached = 0;
  assert.equal(bindOnce(null, "drop", () => attached++), false);
  assert.equal(attached, 0);
});

test("attach receives the element itself", () => {
  const el = { tag: "root" };
  let received = null;
  bindOnce(el, "k", (node) => { received = node; });
  assert.equal(received, el);
});
