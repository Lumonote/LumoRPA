import test from "node:test";
import assert from "node:assert/strict";
import { renderJobs, runJobAction } from "../src/js/job-manager.js";

test("renders durable job state and controls", () => {
  const html = renderJobs([{ id: "j1", state: "paused", scheduleKind: "cron", attempt: 1, maxAttempts: 3 }]);
  assert.match(html, /CRON/i);
  assert.match(html, /Resume/);
  assert.match(html, /Cancel/);
});

test("maps job controls to backend commands", async () => {
  const calls = [];
  for (const action of ["pause", "resume", "cancel"]) await runJobAction({ action, jobId: "j1" }, async (command) => calls.push(command));
  assert.deepEqual(calls, ["job_pause", "job_resume", "job_cancel"]);
});
