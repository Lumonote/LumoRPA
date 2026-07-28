const escapeHtml = (value) => String(value ?? "").replaceAll("&", "&amp;").replaceAll("<", "&lt;").replaceAll(">", "&gt;").replaceAll('"', "&quot;").replaceAll("'", "&#39;");

export function renderJobs(jobs = []) {
  if (!jobs.length) return '<div class="hub-empty"><strong>暂无 Agent 任务</strong><span>定时或一次性任务会显示在这里。</span></div>';
  return `<div class="job-list">${jobs.map((job) => `<article class="hub-row"><div><strong>${escapeHtml(job.id)}</strong><span>${escapeHtml(job.schedule_kind || job.scheduleKind || "one_shot")} · attempt ${Number(job.attempt || 0)}/${Number(job.max_attempts || job.maxAttempts || 0)}</span></div><div class="hub-tags"><span class="hub-tag">${escapeHtml(String(job.state || "unknown").toUpperCase())}</span>${job.state === "paused" ? `<button data-job-action="resume" data-job-id="${escapeHtml(job.id)}">Resume</button>` : `<button data-job-action="pause" data-job-id="${escapeHtml(job.id)}">Pause</button>`}<button data-job-action="cancel" data-job-id="${escapeHtml(job.id)}">Cancel</button></div></article>`).join("")}</div>`;
}

export function runJobAction(action, call) {
  const command = { pause: "job_pause", resume: "job_resume", cancel: "job_cancel" }[action.action];
  if (!command) throw new Error(`unknown job action: ${action.action}`);
  return call(command, { jobId: action.jobId });
}
