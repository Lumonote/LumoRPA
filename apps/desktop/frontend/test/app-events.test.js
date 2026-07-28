import test from "node:test";
import assert from "node:assert/strict";

import { normalizeToastEvent, reduceRunProgress } from "../src/js/app-events.js";

test("normalizes backend toast payloads for the existing toast helper", () => {
  assert.deepEqual(
    normalizeToastEvent({ kind: "warning", message: "skill failed" }),
    { title: "提示", body: "skill failed", kind: "warn" },
  );
});

test("reduces step and log progress without requesting a persisted-run refresh", () => {
  let progress = reduceRunProgress(undefined, {
    type: "step_started",
    runId: "run-1",
    path: "login/click",
    stepId: "click",
    action: "browser.click",
  });
  assert.equal(progress.currentStep, "login/click");
  assert.equal(progress.status, "running");

  progress = reduceRunProgress(progress, {
    type: "log",
    runId: "run-1",
    stepPath: "login/click",
    line: "clicked",
  });
  assert.deepEqual(progress.logs, ["clicked"]);

  progress = reduceRunProgress(progress, {
    type: "step_finished",
    runId: "run-1",
    path: "login/click",
    stepId: "click",
    state: "ok",
    attempt: 1,
    error: null,
  });
  assert.equal(progress.currentStep, null);
  assert.equal(progress.lastStep.state, "ok");
  assert.equal(progress.refreshRequested, false);
});

test("maps error/unknown toast kinds onto the ok/warn/bad palette", () => {
  assert.equal(normalizeToastEvent({ kind: "error", message: "x" }).kind, "bad");
  assert.equal(normalizeToastEvent({ kind: "sparkle", message: "x" }).kind, "ok");
  assert.equal(normalizeToastEvent().body, "");
});

test("a failed step marks the run status and a new runId resets progress", () => {
  let progress = reduceRunProgress(undefined, {
    type: "step_started", runId: "run-1", path: "a", stepId: "a", action: "control.log",
  });
  progress = reduceRunProgress(progress, {
    type: "step_finished", runId: "run-1", path: "a", stepId: "a", state: "failed", attempt: 1, error: "boom",
  });
  assert.equal(progress.status, "failed");
  assert.equal(progress.currentStep, null);

  const fresh = reduceRunProgress(progress, {
    type: "step_started", runId: "run-2", path: "b", stepId: "b", action: "control.log",
  });
  assert.equal(fresh.runId, "run-2");
  assert.equal(fresh.status, "running");
  assert.deepEqual(fresh.logs, []);
});

test("caps buffered log lines and counts dropped ones", () => {
  let progress = reduceRunProgress(undefined, {
    type: "step_started", runId: "run-1", path: "a", stepId: "a", action: "control.log",
  });
  for (let i = 0; i < 1002; i += 1) {
    progress = reduceRunProgress(progress, { type: "log", runId: "run-1", stepPath: "a", line: `l${i}` });
  }
  assert.equal(progress.logs.length, 1000);
  assert.equal(progress.logs[0], "l2");
  assert.equal(progress.droppedLogs, 2);
});
