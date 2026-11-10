import test from "node:test";
import assert from "node:assert/strict";

import { bindCapsuleRunNavigation, normalizeToastEvent, reduceRunProgress } from "../src/js/app-events.js";

test("capsule run navigation awaits opening the exact run and returns the listener disposer", async () => {
  let receive;
  let finish;
  const calls = [];
  const stop = () => {};
  const unlisten = await bindCapsuleRunNavigation({
    listen: async (name, handler) => {
      assert.equal(name, "lumo://capsule-open-run");
      receive = handler;
      return stop;
    },
    openRun: (runId) => {
      calls.push(runId);
      return new Promise((resolve) => { finish = resolve; });
    },
    onError: (error) => assert.fail(error),
  });
  let settled = false;
  const opening = receive({ payload: { runId: "associated-run" } }).then(() => { settled = true; });
  assert.deepEqual(calls, ["associated-run"]);
  assert.equal(settled, false);
  finish();
  await opening;
  assert.equal(settled, true);
  assert.equal(unlisten, stop);
});

test("capsule run navigation ignores invalid run ids and reports navigation errors", async () => {
  let receive;
  const calls = [];
  const errors = [];
  const failure = new Error("mission control unavailable");
  await bindCapsuleRunNavigation({
    listen: async (_name, handler) => { receive = handler; return () => {}; },
    openRun: async (runId) => { calls.push(runId); throw failure; },
    onError: (error) => errors.push(error),
  });
  for (const event of [undefined, {}, { payload: {} }, ...[null, "", "   ", 12, {}].map((runId) => ({ payload: { runId } }))]) {
    await receive(event);
  }
  assert.deepEqual(calls, []);
  await receive({ payload: { runId: "run-1" } });
  assert.deepEqual(calls, ["run-1"]);
  assert.deepEqual(errors, [failure]);
});

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
