const escapeHtml = (value) => String(value ?? "").replaceAll("&", "&amp;").replaceAll("<", "&lt;").replaceAll(">", "&gt;").replaceAll('"', "&quot;").replaceAll("'", "&#39;");
import { jobCanEdit } from "./job-scheduler.js";

export function jobNextRunLabel(job) {
  const nextAt = job.available_at || job.availableAt;
  if (!nextAt || String(job.state) !== "queued") return "";
  const when = new Date(nextAt);
  if (Number.isNaN(when.getTime())) return "";
  const spec = job.schedule_spec || job.scheduleSpec || {};
  const timezone = spec.timezone || (job.schedule_kind === "cron" ? "UTC" : undefined);
  try {
    return ` · 下次 ${when.toLocaleString(undefined, { timeZone: timezone })}${timezone ? ` (${timezone})` : ""}`;
  } catch { return " · 定时时区无效，请编辑任务"; }
}

export function renderJobs(jobs = []) {
  if (!jobs.length) return '<div class="hub-empty"><strong>暂无 Agent 任务</strong><span>定时或一次性任务会显示在这里。</span></div>';
  return `<div class="job-list">${jobs.map((job) => {
    const state = job.state || "unknown";
    const actionable = ["queued", "waiting", "running", "paused"].includes(state);
    const review = state === "waiting_approval" ? "等待审批" : state === "unknown" ? "核验执行结果" : "查看运行";
    return `<article class="hub-row"><div><strong>${escapeHtml(job.payload?.suppliedPlan?.metadata?.flowName || job.id)}</strong><span>${escapeHtml(job.schedule_kind || job.scheduleKind || "one_shot")} · attempt ${Number(job.attempts ?? job.attempt ?? 0)}/${Number(job.max_attempts ?? job.maxAttempts ?? 0)}${escapeHtml(jobNextRunLabel(job))}</span>${job.last_error ? `<small>${escapeHtml(job.last_error)}</small>` : ""}</div><div class="hub-tags"><span class="hub-tag">${escapeHtml(String(state).toUpperCase())}</span>${jobCanEdit(job) ? `<button data-job-action="edit" data-job-id="${escapeHtml(job.id)}">编辑定时任务</button>` : ""}${actionable ? `<button data-job-action="${state === "paused" ? "resume" : "pause"}" data-job-id="${escapeHtml(job.id)}">${state === "paused" ? "Resume" : "Pause"}</button>` : ""}${job.run_id || job.runId || job.schedule_kind === "cron" || state === "unknown" ? `<button data-job-action="review" data-job-id="${escapeHtml(job.id)}">${review}</button>` : ""}${!["completed", "failed"].includes(state) ? `<button data-job-action="cancel" data-job-id="${escapeHtml(job.id)}">Cancel</button>` : ""}</div></article>`;
  }).join("")}</div>`;
}

export async function loadJobReview(job, call) {
  if (!job) throw new Error("任务不存在，请刷新列表");
  if (job.state === "unknown") return { job, nodes: await call("job_unknown_nodes", { jobId: job.id }) };
  const runId = job.run_id || job.runId || await call("job_latest_run", { jobId: job.id });
  if (!runId) throw new Error("任务还没有执行记录");
  let projection = createRunProjection(runId);
  for (const event of await call("agent_events", { runId, afterSeq: 0 })) projection = applyAgentEvent(projection, event);
  return { job, projection };
}

export function renderJobReview({ job, nodes, projection }, results = {}) {
  const body = nodes ? nodes.map((node) => {
    const id = node.nodeId || node.node_id || node.checkpoint?.node_id;
    return `<form data-job-review-node="${escapeHtml(id)}" data-job-id="${escapeHtml(job.id)}"><h4>${escapeHtml(id)} · ${escapeHtml(node.risk || node.checkpoint?.risk)}</h4><p>请先核对外部系统。仅在操作未发生时选择重试；已完成时填写真实输出。</p><label>已完成的输出（JSON）<textarea data-job-node-result rows="4">${escapeHtml(results[id] ?? "{}")}</textarea></label><button type="submit" value="completed">确认已完成</button><button type="submit" value="retry">确认未发生，允许重试</button></form>`;
  }).join("") : (projection.pendingApprovals || []).map((event) => renderConfirmation({ runId: projection.runId, nodeId: event.nodeId || event.node_id, ...event.payload })).join("") || `<p>运行状态：${escapeHtml(projection.status)}</p>${Object.values(projection.nodes || {}).map((node) => `<details><summary>${escapeHtml(node.id)} · ${escapeHtml(node.status)}</summary><pre>${escapeHtml(JSON.stringify(node.payload?.output ?? node.payload, null, 2))}</pre></details>`).join("")}`;
  return `<section class="hub-card" data-job-review><header><h3>任务 ${escapeHtml(job.id)}</h3><button data-job-review-close>关闭详情</button></header>${body}<p data-job-review-error role="status"></p></section>`;
}

export function createJobReviewController(call) {
  let current = null;
  let selection = 0;
  let request = 0;
  let selecting = false;
  const drafts = new Map();
  const snapshot = () => ({ selection, jobId: current?.job.id });
  const isCurrent = (token) => token.selection === selection && token.jobId === current?.job.id;
  const fetch = async (job, invoke, selected) => {
    const sequence = ++request;
    const loaded = await loadJobReview(job, invoke);
    if (selected !== selection || sequence !== request) return false;
    current = loaded;
    return true;
  };
  return {
    get current() { return current; },
    snapshot,
    isCurrent,
    async open(job, invoke = call) {
      const selected = ++selection;
      selecting = true;
      try { return await fetch(job, invoke, selected); }
      finally { if (selected === selection) selecting = false; }
    },
    close() { selection++; request++; current = null; selecting = false; },
    async refresh(jobs, invoke = call) {
      if (!current || selecting) return false;
      const job = jobs.find((item) => item.id === current.job.id);
      if (!job) return false;
      return fetch(job, invoke, selection);
    },
    setResult(nodeId, result) {
      if (!current) return;
      const values = drafts.get(current.job.id) || {};
      values[nodeId] = result;
      drafts.set(current.job.id, values);
    },
    markup() { return current ? renderJobReview(current, drafts.get(current.job.id)) : ""; },
  };
}

export async function resolveUnknownNode({ jobId, nodeId, outcome, result }, call, confirm) {
  if (!["retry", "completed"].includes(outcome)) throw new Error("请选择核验结论");
  const payload = { jobId, nodeId, outcome };
  if (outcome === "completed") payload.result = JSON.parse(result);
  if (!await confirm(outcome === "retry" ? "已核对外部系统，确认本节点未产生副作用，允许重新执行？" : "确认本节点已完成，使用填写的真实输出继续后续节点？")) return false;
  await call("job_resolve_unknown", payload);
  return true;
}

export function runJobAction(action, call) {
  const command = { pause: "job_pause", resume: "job_resume", cancel: "job_cancel" }[action.action];
  if (!command) throw new Error(`unknown job action: ${action.action}`);
  return call(command, { jobId: action.jobId });
}
import { applyAgentEvent, createRunProjection } from "./agent-events.js";
import { renderConfirmation } from "./agent-confirmation.js";
