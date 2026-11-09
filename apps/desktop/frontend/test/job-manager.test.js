import test from "node:test";
import assert from "node:assert/strict";
import { jobNextRunLabel, renderJobs, runJobAction } from "../src/js/job-manager.js";
import { createJobReviewController, loadJobReview, renderJobReview, resolveUnknownNode } from "../src/js/job-manager.js";

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

test("the next occurrence is displayed in the schedule timezone", () => {
  const date = "2026-01-01T01:00:00Z";
  const timezone = "America/New_York";
  const label = jobNextRunLabel({ state:"queued", available_at:date, schedule_spec:{timezone} });
  assert.ok(label.includes(new Date(date).toLocaleString(undefined, { timeZone:timezone })));
});

test("waiting approvals and unknown results expose review without unsafe resume", () => {
  const html = renderJobs([
    { id: "waiting", state: "waiting_approval", run_id: "run-1" },
    { id: "uncertain", state: "unknown", run_id: "run-2" },
    { id: "done", state: "completed", run_id: "run-3" },
  ]);
  assert.match(html, /data-job-action="review"/);
  assert.match(html, /等待审批/);
  assert.match(html, /核验执行结果/);
  assert.doesNotMatch(html, /data-job-action="resume"/);
  assert.doesNotMatch(html, /data-job-action="pause"/);
});

test("job approval review restores pending requests from durable events", async () => {
  const review = await loadJobReview({ id: "j1", state: "waiting_approval", run_id: "r1" }, async (command, args) => {
    assert.equal(command, "agent_events");
    assert.deepEqual(args, { runId: "r1", afterSeq: 0 });
    return [
      { runId: "r1", seq: 1, kind: "permission.requested", nodeId: "send", payload: { planHash: "hash", nodeIds: ["send"], risk: "L3", capabilityId: "flow:send" } },
    ];
  });
  assert.match(renderJobReview(review), /data-agent-action="approve"/);
  assert.match(renderJobReview(review), /data-risk="L3"/);
});

test("a recurring job can open its previous result after the next occurrence is queued", async () => {
  const calls=[];
  const job={id:"daily",state:"queued",schedule_kind:"cron",run_id:null};
  assert.match(renderJobs([job]),/data-job-action="review"/);
  const review=await loadJobReview(job,async (command,args) => {
    calls.push([command,args]);
    if(command==="job_latest_run")return "previous";
    return [{runId:"previous",seq:1,kind:"run.completed",payload:{}}];
  });
  assert.equal(review.projection.runId,"previous");
  assert.equal(review.projection.status,"completed");
  assert.deepEqual(calls[0],["job_latest_run",{jobId:"daily"}]);
});

test("unknown retry requires explicit confirmation and completed output must parse", async () => {
  let calls = 0;
  const call = async () => { calls++; };
  await resolveUnknownNode({ jobId: "j1", nodeId: "send", outcome: "retry" }, call, () => false);
  assert.equal(calls, 0);
  await assert.rejects(resolveUnknownNode({ jobId: "j1", nodeId: "send", outcome: "completed", result: "bad json" }, call, () => true));
  assert.equal(calls, 0);
  let payload;
  await resolveUnknownNode({ jobId: "j1", nodeId: "send", outcome: "completed", result: '{"sent":true}' }, async (_command, args) => { payload = args; }, () => true);
  assert.deepEqual(payload, { jobId: "j1", nodeId: "send", outcome: "completed", result: { sent: true } });
});

test("review refresh keeps typed verification output and ignores a closed request", async () => {
  let resolve;
  const controller = createJobReviewController(async () => [{ nodeId: "send", risk: "L2" }]);
  const job = { id: "j1", state: "unknown" };
  await controller.open(job);
  controller.setResult("send", '{"receipt":"<actual>"}');
  await controller.refresh([job]);
  assert.match(controller.markup(), /&quot;receipt&quot;:&quot;&lt;actual&gt;&quot;/);
  const pending = controller.open({ id: "j2", state: "unknown" }, async () => new Promise((done) => { resolve = done; }));
  controller.close();
  resolve([]);
  await pending;
  assert.equal(controller.current, null);
});

test("old review fetch and operation cannot overwrite a newly selected job", async () => {
  let resolve;
  const controller = createJobReviewController(async () => []);
  await controller.open({ id: "j1", state: "unknown" });
  const operation = controller.snapshot();
  const pending = controller.refresh([{ id: "j1", state: "unknown" }], async () => new Promise((done) => { resolve = done; }));
  await controller.open({ id: "j2", state: "unknown" });
  resolve([{ nodeId: "old" }]);
  await pending;
  assert.equal(controller.current.job.id, "j2");
  assert.equal(controller.isCurrent(operation), false);
  assert.doesNotMatch(controller.markup(), /old/);
});

test("automatic refresh cannot replace a pending selection with the previous job", async () => {
  let resolve;
  const controller = createJobReviewController(async () => []);
  await controller.open({ id:"A",state:"unknown" });
  const pending = controller.open({ id:"B",state:"unknown" }, async () => new Promise((done) => { resolve=done; }));
  await controller.refresh([{id:"A",state:"unknown"},{id:"B",state:"unknown"}]);
  resolve([]);
  await pending;
  assert.equal(controller.current.job.id,"B");
});
