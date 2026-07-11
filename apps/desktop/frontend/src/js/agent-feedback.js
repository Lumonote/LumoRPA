const secretPattern = /(token|secret|password|authorization|cookie|api.?key)(\s*[=:]\s*)[^\s,;]+/gi;
const secretKey = (key) => /(token|secret|password|authorization|cookie|api.?key)/i.test(key);
const escapeHtml = (value) => String(value ?? "").replaceAll("&", "&amp;").replaceAll("<", "&lt;").replaceAll(">", "&gt;").replaceAll('"', "&quot;").replaceAll("'", "&#39;");

export function redactDiagnostic(value) {
  return String(value ?? "").replace(secretPattern, "$1$2••••••••");
}

function redactPayload(value, key = "") {
  if (secretKey(key)) return "••••••••";
  if (Array.isArray(value)) return value.map((item) => redactPayload(item));
  if (value && typeof value === "object") return Object.fromEntries(Object.entries(value).map(([childKey, child]) => [childKey, redactPayload(child, childKey)]));
  return value;
}

export function selectAgentFeedback(event, { quiet = false } = {}) {
  if (quiet || !event) return null;
  if (event.status === "completed") return String(event.result || "").length > 240 ? "任务已完成，详细结果已写入 Mission Control。" : `任务已完成${event.result ? `：${event.result}` : "。"}`;
  if (event.status === "failed") return `执行失败：${redactDiagnostic(event.error || "未知错误")}`;
  return null;
}

export function renderAgentLog(events = [], limit = 80) {
  const visible = events.slice(-limit).reverse();
  const dropped = Math.max(0, events.length - visible.length);
  const notice = dropped ? `<div class="mc-log-dropped">${dropped} earlier events dropped</div>` : "";
  const rows = visible.map((event) => `<div><span>${escapeHtml(event.seq)}</span><strong>${escapeHtml(event.kind)}</strong><em>${escapeHtml(event.nodeId || event.node_id || "run")}</em><code>${escapeHtml(JSON.stringify(redactPayload(event.payload || {})))}</code></div>`).join("");
  return notice + rows || '<div class="mc-empty">暂无事件</div>';
}
