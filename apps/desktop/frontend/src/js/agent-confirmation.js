const escapeHtml = (value) => String(value ?? "").replaceAll("&", "&amp;").replaceAll("<", "&lt;").replaceAll(">", "&gt;").replaceAll('"', "&quot;").replaceAll("'", "&#39;");
const secretKey = (key) => /(token|secret|password|authorization|cookie|api.?key)/i.test(key);

function redact(value, key = "") {
  if (secretKey(key)) return "••••••••";
  if (Array.isArray(value)) return value.map((item) => redact(item));
  if (value && typeof value === "object") return Object.fromEntries(Object.entries(value).map(([childKey, child]) => [childKey, redact(child, childKey)]));
  return value;
}

export function renderConfirmation(request) {
  if (!request) return "";
  const strengthened = request.risk === "L3";
  return `<section class="mc-confirmation is-${escapeHtml(request.risk || "L2")}" data-plan-hash="${escapeHtml(request.planHash)}"><span>${strengthened ? "高风险 · L3 双重确认" : "敏感操作 · L2 确认"}</span><h3>${escapeHtml(request.capabilityId)}</h3><p>${escapeHtml(request.reason || "此能力需要人工授权")}</p><pre>${escapeHtml(JSON.stringify(redact(request.arguments || {}), null, 2))}</pre><small>Plan hash · ${escapeHtml(request.planHash)}</small><div class="mc-confirm-actions"><button data-agent-action="reject" data-run-id="${escapeHtml(request.runId)}" data-node-id="${escapeHtml(request.nodeId)}" data-plan-hash="${escapeHtml(request.planHash)}">拒绝</button><button class="is-primary" data-agent-action="approve" data-run-id="${escapeHtml(request.runId)}" data-node-id="${escapeHtml(request.nodeId)}" data-plan-hash="${escapeHtml(request.planHash)}">批准本次执行</button></div></section>`;
}

const commands = { approve: "agent_approve", reject: "agent_approve", pause: "agent_pause", resume: "agent_resume", cancel: "agent_cancel", skip: "agent_cancel" };

export async function runAgentControl(control, call, currentPlanHash = () => control.planHash) {
  if (control.action === "approve" && currentPlanHash() !== control.planHash) throw new Error("stale approval: plan hash changed");
  const command = commands[control.action];
  if (!command) throw new Error(`unknown agent action: ${control.action}`);
  return call(command, { runId: control.runId, nodeId: control.nodeId, planHash: control.planHash, approved: control.action === "approve", resolution: control.action });
}

export function renderRunControls(projection) {
  const action = projection.status === "paused" ? "resume" : "pause";
  return `<div class="mc-run-controls"><button data-agent-action="${action}" data-run-id="${escapeHtml(projection.runId)}">${action === "pause" ? "暂停" : "继续"}</button><button data-agent-action="cancel" data-run-id="${escapeHtml(projection.runId)}">终止</button></div>`;
}

