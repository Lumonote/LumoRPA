const escapeHtml = (value) => String(value ?? "").replaceAll("&", "&amp;").replaceAll("<", "&lt;").replaceAll(">", "&gt;").replaceAll('"', "&quot;").replaceAll("'", "&#39;");
const secretKey = (key) => /(token|secret|password|authorization|cookie|api.?key|vault)/i.test(key);

function redact(value, key = "") {
  if (secretKey(key)) return "••••••••";
  if (Array.isArray(value)) return value.map((item) => redact(item));
  if (value && typeof value === "object") return Object.fromEntries(Object.entries(value).map(([childKey, child]) => [childKey, redact(child, childKey)]));
  return value;
}

export function renderSecurityCenter({ grants = [], findings = [], revocations = [] } = {}) {
  const grantRows = grants.length ? grants.map((grant) => `<article class="security-grant is-${escapeHtml(grant.risk)}"><header><div><span>${escapeHtml(grant.risk)} · ${escapeHtml(grant.scope || "once")}</span><h3>${escapeHtml(grant.capabilityId)}</h3></div><em>${grant.risk === "L3" ? "BIOMETRIC / 生物认证" : "POLICY APPROVED"}</em></header><pre>${escapeHtml(JSON.stringify(redact(grant.arguments || {}), null, 2))}</pre><small>Expires ${escapeHtml(grant.expiresAt || "session end")}</small><button data-security-action="revoke" data-grant-id="${escapeHtml(grant.id)}">撤销授权</button></article>`).join("") : '<div class="hub-empty"><strong>没有生效中的权限授权</strong><span>L2/L3 操作将在此显示不可变审批记录。</span></div>';
  const findingRows = findings.length ? findings.map((finding) => `<article class="security-finding"><span>${escapeHtml(String(finding.kind).replaceAll("_", " "))}</span><strong>${escapeHtml(finding.source)}</strong><p>${escapeHtml(finding.summary)}</p></article>`).join("") : '<div class="hub-empty"><strong>未发现信任边界事件</strong><span>网页、邮件和 MCP 内容始终作为不可信数据处理。</span></div>';
  return `<div class="security-center"><div class="security-header"><div><span>ZERO-TRUST CONTROL PLANE</span><h2>Security Center</h2></div><div><button data-security-action="biometric">验证生物认证</button><button data-security-action="export">导出脱敏审计</button></div></div><div class="security-matrix"><section><h3>ACTIVE GRANTS</h3>${grantRows}</section><section><h3>TRUST FINDINGS</h3>${findingRows}</section></div><footer>${revocations.length} revoked grants retained immutably</footer></div>`;
}

const commands = { revoke: "security_revoke", export: "security_export_audit", biometric: "security_biometric_challenge" };

export function runSecurityAction(action, call) {
  const command = commands[action.action];
  if (!command) throw new Error(`unknown security action: ${action.action}`);
  return call(command, { grantId: action.grantId });
}
