import test from "node:test";
import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { URL } from "node:url";
import { buildFlowSchedule, jobCanEdit, scheduleFormValues, renderScheduleForm, createJobScheduler } from "../src/js/job-scheduler.js";

const values = { kind:"cron", expression:"0 0 9 * * *", timezone:"Asia/Shanghai", misfire:"skip", inputs:'{"day":"today"}', maxAttempts:"4", timeoutSeconds:"60", priority:"5", profileId:"safe" };
test("flow scheduling preserves timezone, inputs and retry settings", () => {
  const request = buildFlowSchedule({ path:"/flows/daily.yaml", ...values });
  assert.deepEqual(request.schedule,{kind:"cron",expression:"0 0 9 * * *",timezone:"Asia/Shanghai",misfire:"skip"});
  assert.deepEqual(request.inputs,{day:"today"});
  assert.equal(request.maxAttempts,4);
  assert.equal(request.timeoutMs,60_000);
  assert.equal(request.profileId,"safe");
});
test("invalid inputs, dates, timezones and retry budgets fail before IPC", () => {
  for (const invalid of [{inputs:"[]"},{inputs:"bad"},{maxAttempts:"0"},{maxAttempts:"3.5"},{timezone:"Wrong/Zone"},{expression:"* *"},{kind:"one_shot",runAt:"bad"},{kind:"one_shot",runAt:"2000-01-01T01:00"}]) {
    assert.throws(() => buildFlowSchedule({path:"/flows/daily.yaml",...values,...invalid}));
  }
});
test("editing roundtrips stored schedule and only exposes untouched occurrences", () => {
  const job = {state:"paused",attempts:0,run_id:null,payload:{suppliedPlan:{metadata:{flowPath:"/flows/daily.yaml"},nodes:[{arguments:{day:"<today>"},timeoutMs:70_000}]}},schedule_kind:"cron",schedule_spec:{expression:"0 0 9 * * *",timezone:"Asia/Shanghai",misfire:"skip"},max_attempts:5,priority:8};
  assert.equal(jobCanEdit(job),true);
  assert.equal(jobCanEdit({...job,state:"waiting_approval"}),false);
  assert.equal(jobCanEdit({...job,run_id:"old"}),false);
  assert.equal(jobCanEdit({...job,attempts:1}),false);
  const stored = scheduleFormValues(job);
  assert.equal(stored.timeoutSeconds,70);
  assert.equal(stored.maxAttempts,5);
  assert.equal(stored.timezone,"Asia/Shanghai");
  const html = renderScheduleForm(stored);
  assert.match(html,/&lt;today&gt;/);
  assert.doesNotMatch(html,/<today>/);
});

test("an unchanged one-shot edit preserves its original seconds and milliseconds", () => {
  const runAt = "2099-01-01T08:09:47.123Z";
  const job = { schedule_kind: "one_shot", schedule_spec: { runAt }, payload: { suppliedPlan: { metadata: { flowPath: "/flows/daily.yaml" }, nodes: [{ arguments: {}, timeoutMs: 300000 }] } } };
  const form = scheduleFormValues(job);
  assert.equal(buildFlowSchedule(form).schedule.run_at, runAt);
  assert.match(renderScheduleForm(form), /type="datetime-local"[^>]*step="0.001"/);
});

test("an unchanged one-shot edit preserves the second occurrence of a DST fallback", () => {
  const script = `
    import { scheduleFormValues, buildFlowSchedule } from ${JSON.stringify(new URL("../src/js/job-scheduler.js", import.meta.url).href)};
    const runAt = "2099-11-01T06:30:00.000Z";
    const values = scheduleFormValues({ schedule_kind: "one_shot", schedule_spec: { runAt }, payload: { suppliedPlan: { metadata: { flowPath: "/flows/a.yaml" }, nodes: [{ arguments: {} }] } } });
    const unchanged = buildFlowSchedule(values).schedule.run_at;
    values.runAt = "2099-11-01T03:30:00.000";
    console.log(JSON.stringify([unchanged, buildFlowSchedule(values).schedule.run_at]));
  `;
  const result = execFileSync(globalThis.process.execPath, ["--input-type=module", "-e", script], { env: { ...globalThis.process.env, TZ: "America/New_York" }, encoding: "utf8" });
  assert.deepEqual(JSON.parse(result), ["2099-11-01T06:30:00.000Z", "2099-11-01T08:30:00.000Z"]);
});

function schedulerUi(context) {
  const previousDocument = globalThis.document, previousFormData = globalThis.FormData;
  context.after(() => { globalThis.document = previousDocument; globalThis.FormData = previousFormData; });
  const created = [];
  globalThis.FormData = class {
    constructor(form) { this.values = form.values; }
    *[Symbol.iterator]() { yield* Object.entries(this.values); }
  };
  globalThis.document = {
    body: { append() {} },
    createElement() {
      const elements = Object.fromEntries(["form", "[data-schedule-cancel]", "[data-schedule-error]"].map((key) => [key, { handlers: {}, addEventListener(name, handler) { this.handlers[name] = handler; }, textContent: "" }]));
      const controls = [{ disabled: false }, { disabled: true }, { disabled: false }];
      const dialog = { style: {}, elements, controls, handlers: {}, closed: false,
        addEventListener(name, handler) { this.handlers[name] = handler; },
        querySelector: (selector) => elements[selector], querySelectorAll: (selector) => selector === "button" ? [controls[0]] : controls,
        close() { this.closed = true; }, remove() {}, focus() {}, showModal() {},
      };
      elements.form.values = { path: "/flows/daily.yaml", ...values };
      created.push(dialog);
      return dialog;
    },
  };
  const submit = (dialog) => dialog.elements.form.handlers.submit({ preventDefault() {}, target: dialog.elements.form });
  return { created, submit };
}

test("rapid scheduling clicks share one pending save and open only one dialog", async (context) => {
  const { created } = schedulerUi(context);
  let finish, saves = 0;
  const saving = new Promise((resolve) => { finish = resolve; });
  const scheduler = createJobScheduler({ call: async () => {}, ensureSaved: () => { saves++; return saving; }, getCurrentFlow: () => ({ path: "/flows/daily.yaml", inputs: "{}" }) });
  const first = scheduler.open(), second = scheduler.open();
  finish();
  await Promise.all([first, second]);
  assert.equal(saves, 1);
  assert.equal(created.length, 1);
});

test("a rejected initial save releases the opening lock for a later attempt", async (context) => {
  const { created } = schedulerUi(context);
  let fail = true;
  const scheduler = createJobScheduler({ call: async () => {}, ensureSaved: async () => { if (fail) throw new Error("disk unavailable"); }, getCurrentFlow: () => ({ path: "/flows/daily.yaml", inputs: "{}" }) });
  await assert.rejects(scheduler.open(), /disk unavailable/);
  fail = false;
  await scheduler.open();
  assert.equal(created.length, 1);
});

test("switching flows while the initial save is pending cannot schedule the unsaved new target", async (context) => {
  const { created } = schedulerUi(context);
  let finish;
  let current = { path: "/flows/A.yaml", inputs: "{}", dirty: true };
  const scheduler = createJobScheduler({ call: async () => {}, ensureSaved: () => new Promise((resolve) => { finish = resolve; }), getCurrentFlow: () => current });
  const opening = scheduler.open();
  current = { path: "/flows/B.yaml", inputs: "{}", dirty: false };
  finish("/flows/A.yaml");
  await assert.rejects(opening, /流程已切换|重新打开/);
  assert.equal(created.length, 0);
});

test("new same-file edits made during the initial save stop scheduling until saved", async (context) => {
  const { created } = schedulerUi(context);
  let dirty = true;
  const scheduler = createJobScheduler({ call: async () => {}, ensureSaved: async () => "/flows/A.yaml", getCurrentFlow: () => ({ path: "/flows/A.yaml", inputs: "{}", dirty }) });
  await assert.rejects(scheduler.open(), /新编辑|保存/);
  assert.equal(created.length, 0);
  dirty = false;
  await scheduler.open();
  assert.equal(created.length, 1);
});

test("schedule submission freezes the captured form, blocks duplicate submit and cannot close mid-save", async (context) => {
  const { created, submit } = schedulerUi(context);
  let finish;
  const calls = [];
  const scheduler = createJobScheduler({ ensureSaved: async () => {}, getCurrentFlow: () => ({ path: "/flows/daily.yaml", inputs: "{}" }), call: (command, args) => { calls.push({ command, args }); return new Promise((resolve) => { finish = resolve; }); } });
  await scheduler.open();
  const dialog = created[0];
  const request = submit(dialog);
  assert.ok(dialog.controls.every((control) => control.disabled));
  await submit(dialog);
  dialog.elements["[data-schedule-cancel]"].handlers.click();
  dialog.handlers.cancel({ preventDefault() {} });
  assert.equal(dialog.closed, false);
  assert.equal(calls.length, 1);
  assert.deepEqual(calls[0].args.request.inputs, { day: "today" });
  finish({ job: { id: "created" } });
  await request;
  assert.equal(dialog.closed, true);
  assert.deepEqual(dialog.controls.map((control) => control.disabled), [false, true, false]);
});
