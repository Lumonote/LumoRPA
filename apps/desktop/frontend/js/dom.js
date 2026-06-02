// Small DOM + formatting helpers shared across every panel.

export const $ = (id) => document.getElementById(id);
export const $$ = (sel, root = document) => Array.from(root.querySelectorAll(sel));

export function html(value) {
  return String(value ?? "")
    .replaceAll("&", "&amp;")
    .replaceAll("<", "&lt;")
    .replaceAll(">", "&gt;")
    .replaceAll('"', "&quot;");
}

export function pretty(value) {
  if (value === undefined || value === null || value === "") return "";
  try {
    return JSON.stringify(value, null, 2);
  } catch {
    return String(value);
  }
}

export function toast(title, body = "", kind = "ok") {
  const stack = $("toastStack");
  const node = document.createElement("div");
  node.className = `toast ${kind}`;
  node.innerHTML = `<div class="toast-title">${html(title)}</div>${body ? `<div class="toast-body">${html(body)}</div>` : ""}`;
  stack.appendChild(node);
  setTimeout(() => {
    node.style.opacity = "0";
    node.style.transition = "opacity 260ms";
    setTimeout(() => node.remove(), 280);
  }, kind === "bad" ? 5500 : 3200);
}

export function setStatus(text, tone = "ok") {
  const t = $("statusText");
  t.textContent = text;
  t.style.color = tone === "bad" ? "var(--bad)" : tone === "warn" ? "var(--warn)" : "var(--muted)";
}

export function kv(label, value) {
  return `<div class="kv"><span>${html(label)}</span><strong title="${html(value)}">${html(value)}</strong></div>`;
}

export function reportError(error) {
  setStatus("操作失败", "bad");
  toast("操作失败", String(error), "bad");
}
