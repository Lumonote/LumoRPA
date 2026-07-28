import test from "node:test";
import assert from "node:assert/strict";
import { renderUpdateCenter, updateProgressPercent } from "../src/js/update-center.js";

test("renders signed updater configuration and channels", () => {
  const html = renderUpdateCenter({ currentVersion: "1.2.3", channel: "beta", configured: true });
  assert.match(html, /1\.2\.3/);
  assert.match(html, /value="beta" selected/);
  assert.doesNotMatch(html, /data-update-check disabled/);
});

test("clamps updater download progress", () => {
  assert.equal(updateProgressPercent(50, 100), 50);
  assert.equal(updateProgressPercent(200, 100), 100);
  assert.equal(updateProgressPercent(1, 0), 0);
});
