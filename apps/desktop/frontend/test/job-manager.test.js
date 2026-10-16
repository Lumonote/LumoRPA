import test from "node:test";
import assert from "node:assert/strict";
import { jobNextRunLabel, renderJobs, runJobAction } from "../src/js/job-manager.js";

test("renders durable job state and controls", () => {
  const html = renderJobs([{ id: "j1", state: "paused", scheduleKind: "cron", attempt: 1, maxAttempts: 3 }]);
  assert.match(html, /CRON/i);
  assert.match(html, /Resume/);
  assert.match(html, /Cancel/);
});

test("queued cron jobs show the next local fire time with timezone", () => {
  const availableAt = "2026-01-01T01:00:00Z";
  const label = jobNextRunLabel({
    state: "queued",
    availableAt,
    scheduleSpec: { expression: "0 0 9 * * *", timezone: "Asia/Shanghai" },
  });
  assert.match(label, /下次 /);
  assert.match(label, /\(Asia\/Shanghai\)/);
  assert.equal(jobNextRunLabel({ state: "running", availableAt }), "");
  assert.equal(jobNextRunLabel({ state: "queued" }), "");
  const html = renderJobs([{ id: "j2", state: "queued", scheduleKind: "cron", availableAt, scheduleSpec: { timezone: "Asia/Shanghai" } }]);
  assert.match(html, /下次 /);
});

test("maps job controls to backend commands", async () => {
  const calls = [];
  for (const action of ["pause", "resume", "cancel"]) await runJobAction({ action, jobId: "j1" }, async (command) => calls.push(command));
  assert.deepEqual(calls, ["job_pause", "job_resume", "job_cancel"]);
});
