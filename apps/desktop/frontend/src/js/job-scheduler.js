const html = (value) => String(value ?? "").replaceAll("&", "&amp;").replaceAll("<", "&lt;").replaceAll(">", "&gt;").replaceAll('"', "&quot;").replaceAll("'", "&#39;");
const localDate = (value) => {
  const date = new Date(value);
  return new Date(date.getTime() - date.getTimezoneOffset() * 60_000).toISOString().slice(0, 23);
};
const planOf = (job) => job?.payload?.suppliedPlan || job?.payload?.supplied_plan;
export function jobCanEdit(job) {
  return ["queued", "paused"].includes(job?.state) && !job.run_id && !job.runId && Number(job.attempts || 0) === 0 && Boolean(planOf(job)?.metadata?.flowPath);
}
export function scheduleFormValues(job) {
  const plan = planOf(job);
  const spec = job?.schedule_spec || {};
  return {
    path: plan?.metadata?.flowPath || "", kind: job?.schedule_kind || "one_shot",
    runAt: localDate(spec.runAt || Date.now() + 3_600_000),
    originalRunAt: spec.runAt || null,
    expression: spec.expression || "0 0 9 * * *", timezone: spec.timezone || "UTC",
    misfire: spec.misfire || "catch_up_once", inputs: JSON.stringify(plan?.nodes?.[0]?.arguments || {}, null, 2),
    maxAttempts: job?.max_attempts ?? 3, timeoutSeconds: (plan?.nodes?.[0]?.timeoutMs || 300_000) / 1000,
    priority: job?.priority ?? 5, profileId: job?.payload?.profileId || "",
  };
}

export function buildFlowSchedule(values, originalRunAt = values.originalRunAt) {
  const inputs = JSON.parse(values.inputs);
  if (!inputs || Array.isArray(inputs) || typeof inputs !== "object") throw new Error("流程输入必须是 JSON 对象");
  if (!values.path) throw new Error("请先保存并选择流程");
  const maxAttempts = Number(values.maxAttempts), timeoutSeconds = Number(values.timeoutSeconds), priority = Number(values.priority);
  if (!Number.isInteger(maxAttempts) || maxAttempts < 1 || maxAttempts > 20 || !Number.isInteger(timeoutSeconds) || timeoutSeconds < 1 || timeoutSeconds > 3600 || !Number.isInteger(priority) || priority < -100 || priority > 100) throw new Error("尝试次数 1–20，超时 1–3600 秒，优先级 -100–100，均须为整数");
  let schedule;
  if (values.kind === "cron") {
    const expression = values.expression.trim();
    if (![6, 7].includes(expression.split(/\s+/).length)) throw new Error("Cron 需要 6 或 7 段（首段为秒）");
    const timezone = values.timezone.trim();
    if (!timezone) throw new Error("请填写 IANA 时区");
    new Intl.DateTimeFormat("en", { timeZone: timezone }).format();
    if (!["skip", "catch_up_once"].includes(values.misfire)) throw new Error("请选择错过执行时间后的策略");
    schedule = { kind: "cron", expression, timezone, misfire: values.misfire };
  } else if (values.kind === "one_shot") {
    // A datetime-local field cannot distinguish the two occurrences during a
    // DST fallback. Preserve the exact original instant on an unchanged edit.
    const unchanged = originalRunAt && new Date(values.runAt).getTime() === new Date(localDate(originalRunAt)).getTime();
    const date = new Date(unchanged ? originalRunAt : values.runAt);
    if (!Number.isFinite(date.getTime()) || date.getTime() <= Date.now()) throw new Error("请选择未来的本机时间");
    schedule = { kind: "one_shot", run_at: date.toISOString() };
  } else throw new Error("请选择执行方式");
  return { path: values.path, inputs, schedule, maxAttempts, timeoutMs: timeoutSeconds * 1000, priority, profileId: values.profileId?.trim() || null };
}

export function renderScheduleForm(values) {
  const selected = (a, b) => a === b ? " selected" : "";
  return `<form data-flow-schedule style="display:grid;gap:12px"><h3>流程定时任务</h3><p>应用运行时执行已保存流程；审批等待会保留，重启后可继续。执行结果可在能力中心的任务详情查看。</p><label>流程文件<input name="path" readonly value="${html(values.path)}" style="width:100%"></label><label>执行方式<select name="kind"><option value="one_shot"${selected(values.kind,"one_shot")}>一次性</option><option value="cron"${selected(values.kind,"cron")}>重复执行</option></select></label><label>一次性执行时间（本机时区）<input name="runAt" type="datetime-local" step="0.001" value="${html(values.runAt)}"></label><label>Cron（秒 分 时 日 月 周，可加年）<input name="expression" value="${html(values.expression)}" style="width:100%"></label><label>重复执行时区（IANA）<input name="timezone" value="${html(values.timezone)}"></label><label>错过执行时间<select name="misfire"><option value="catch_up_once"${selected(values.misfire,"catch_up_once")}>恢复后补跑一次</option><option value="skip"${selected(values.misfire,"skip")}>跳至下一次</option></select></label><label>流程输入（JSON）<textarea name="inputs" rows="5" style="width:100%">${html(values.inputs)}</textarea></label><label>最多尝试次数（含首次）<input name="maxAttempts" type="number" min="1" max="20" value="${html(values.maxAttempts)}"></label><label>单次超时（秒）<input name="timeoutSeconds" type="number" min="1" max="3600" value="${html(values.timeoutSeconds)}"></label><label>优先级<input name="priority" type="number" min="-100" max="100" value="${html(values.priority)}"></label><label>Agent 配置（留空使用默认）<input name="profileId" value="${html(values.profileId)}"></label><p data-schedule-error role="status"></p><div><button type="submit" class="primary">保存定时任务</button> <button type="button" data-schedule-cancel>取消</button></div></form>`;
}

export function createJobScheduler({ call, getCurrentFlow, ensureSaved, onSaved = () => {} }) {
  let dialog = null, opening = false;
  return {
    async open(job = null) {
      if (dialog) { dialog.focus(); return; }
      if (opening) return;
      opening = true;
      try {
      if (job && !jobCanEdit(job)) throw new Error("任务已开始，请刷新列表后操作");
      const values = scheduleFormValues(job);
      if (!job) {
        const savedPath = await ensureSaved();
        const current = getCurrentFlow();
        if (savedPath !== undefined && savedPath !== current.path) throw new Error("保存期间流程已切换，请重新打开定时任务");
        if (current.dirty) throw new Error("保存期间出现新编辑，请保存后重新打开定时任务");
        Object.assign(values, current);
      }
      if (!values.path) throw new Error("请先保存流程");
      const current = document.createElement("dialog");
      dialog = current;
      current.style.cssText = "max-width:680px;width:85%;max-height:88vh;overflow:auto;padding:24px;border-radius:12px;background:var(--surface);color:var(--text);border:1px solid var(--line)";
      current.innerHTML = renderScheduleForm(values);
      let busy = false;
      const close = () => { current.close(); current.remove(); if (dialog === current) dialog = null; };
      current.querySelector("[data-schedule-cancel]").addEventListener("click", () => { if (!busy) close(); });
      current.addEventListener("cancel", (event) => { event.preventDefault(); if (!busy) close(); });
      const idempotencyKey = crypto.randomUUID();
      current.querySelector("form").addEventListener("submit", async (event) => {
        event.preventDefault();
        if (busy) return;
        busy = true;
        let disabled = [];
        try {
          const request = buildFlowSchedule(Object.fromEntries(new FormData(event.target)), values.originalRunAt);
          disabled = [...current.querySelectorAll("button, input, select, textarea")].map((control) => [control, control.disabled]);
          disabled.forEach(([control]) => { control.disabled = true; });
          const result = job
            ? await call("job_update_schedule", { jobId: job.id, expectedUpdatedAt: job.updated_at, request })
            : await call("job_schedule_flow", { request: { ...request, idempotencyKey } });
          close();
          onSaved(result.job || result);
        } catch (error) { current.querySelector("[data-schedule-error]").textContent = String(error); }
        finally { busy = false; disabled.forEach(([control, wasDisabled]) => { control.disabled = wasDisabled; }); }
      });
      document.body.append(current); current.showModal();
      } finally { opening = false; }
    },
  };
}
