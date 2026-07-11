const escapeHtml = (value) => String(value ?? "").replaceAll("&", "&amp;").replaceAll("<", "&lt;").replaceAll(">", "&gt;").replaceAll('"', "&quot;").replaceAll("'", "&#39;");
const secretKey = (key) => /(token|secret|password|authorization|cookie|api.?key|vault)/i.test(key);

function redact(value, key = "") {
  if (secretKey(key)) return "••••••••";
  if (Array.isArray(value)) return value.map((item) => redact(item));
  if (value && typeof value === "object") return Object.fromEntries(Object.entries(value).map(([childKey, child]) => [childKey, redact(child, childKey)]));
  return value;
}

const percent = (value) => `${Number(value || 0) >= 0 ? "+" : ""}${(Number(value || 0) * 100).toFixed(1)}%`;
const label = (value) => String(value || "unknown").replaceAll(/[_-]+/g, " ");

export function renderImprovementProposals(proposals = []) {
  if (!proposals.length) return '<div class="hub-empty"><strong>暂无改进提案</strong><span>完成的脱敏执行轨迹将生成可评估、可回滚的提案。</span></div>';
  return `<div class="improvement-grid">${proposals.map((proposal) => {
    const metrics = proposal.metrics || proposal.evaluation || {};
    return `<article class="improvement-card"><header><div><span>${escapeHtml(label(proposal.target))}</span><h3>${escapeHtml(proposal.rationale || proposal.id)}</h3></div><em>${escapeHtml(proposal.status || "draft")}</em></header><pre>${escapeHtml(JSON.stringify(redact(proposal.patch || {}), null, 2))}</pre><div class="improvement-metrics"><span>SUCCESS <strong>${percent(metrics.successDelta)}</strong></span><span>LATENCY <strong>${Number(metrics.latencyDeltaMs || 0)}ms</strong></span><span>RISK <strong>${Number(metrics.riskDelta || 0)}</strong></span></div><small>Base ${escapeHtml(proposal.baseVersionHash || proposal.base_version_hash || "unversioned")} · Patch ${escapeHtml(proposal.patchHash || proposal.patch_hash || "pending")}</small><div class="improvement-actions"><button data-improvement-action="evaluate" data-proposal-id="${escapeHtml(proposal.id)}">沙箱评估</button><button data-improvement-action="reject" data-proposal-id="${escapeHtml(proposal.id)}">拒绝</button><button class="is-primary" data-improvement-action="approve" data-proposal-id="${escapeHtml(proposal.id)}" data-patch-hash="${escapeHtml(proposal.patchHash || proposal.patch_hash)}">批准并创建新版本</button>${proposal.status === "applied" ? `<button data-improvement-action="rollback" data-proposal-id="${escapeHtml(proposal.id)}">回滚</button>` : ""}</div></article>`;
  }).join("")}</div>`;
}

const commands = { evaluate: "evaluate_improvement", approve: "approve_improvement", reject: "reject_improvement", rollback: "rollback_improvement" };

export function runImprovementAction(action, call) {
  const command = commands[action.action];
  if (!command) throw new Error(`unknown improvement action: ${action.action}`);
  return call(command, { proposalId: action.proposalId, patchHash: action.patchHash });
}

