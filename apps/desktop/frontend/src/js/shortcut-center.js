const escapeHtml = (value) => String(value ?? "").replaceAll("&", "&amp;").replaceAll("<", "&lt;").replaceAll(">", "&gt;").replaceAll('"', "&quot;").replaceAll("'", "&#39;");

export const SHORTCUT_ACTIONS = [
  ["voice_toggle", "唤起 / 收起语音交互", "像 Siri 一样快速开始对话"],
  ["push_to_talk", "按住说话", "按下开始、松开结束"],
  ["mission_control", "打开 Mission Control", "查看并行与串行执行拓扑"],
  ["capability_hub", "打开 Capability Hub", "管理 Skill、MCP 与权限"],
  ["stop_interaction", "停止语音交互", "立即停止当前收音"],
];

export function normalizeShortcutBindings(bindings = [], legacyShortcut = "CommandOrControl+Shift+Space") {
  return bindings.length ? bindings : [{ id: "voice-toggle", action: "voice_toggle", accelerator: legacyShortcut, enabled: true }];
}

export function renderShortcutCenter(bindings = []) {
  const rows = normalizeShortcutBindings(bindings).map((binding) => {
    const definition = SHORTCUT_ACTIONS.find(([action]) => action === binding.action) || [binding.action, binding.action, "Custom action"];
    return `<article class="shortcut-binding" data-shortcut-id="${escapeHtml(binding.id)}"><button class="shortcut-power ${binding.enabled ? "is-on" : ""}" data-shortcut-toggle aria-label="${binding.enabled ? "禁用" : "启用"}${escapeHtml(definition[1])}"><i></i></button><div class="shortcut-copy"><strong>${escapeHtml(definition[1])}</strong><span>${escapeHtml(definition[2])}</span></div><select data-shortcut-action>${SHORTCUT_ACTIONS.map(([action, label]) => `<option value="${action}"${action === binding.action ? " selected" : ""}>${escapeHtml(label)}</option>`).join("")}</select><button class="shortcut-key" data-shortcut-record title="点击后按下组合键"><kbd>${escapeHtml(binding.accelerator || "点击录入")}</kbd><span>重新录入</span></button><button class="shortcut-remove" data-shortcut-remove aria-label="删除快捷键">×</button><input type="hidden" data-shortcut-enabled value="${binding.enabled ? "true" : "false"}"><input type="hidden" data-shortcut-accelerator value="${escapeHtml(binding.accelerator || "")}"></article>`;
  }).join("");
  return `<section class="shortcut-center"><header><div><span>QUICK INVOCATION MATRIX</span><h3>桌面快捷键中心</h3><p>为常用智能体动作绑定系统级快捷键。录入时至少包含 Command、Control、Option 或 Shift。</p></div><button data-shortcut-add>＋ 添加快捷键</button></header><div data-shortcut-list>${rows}</div></section>`;
}

export function acceleratorFromKeyboardEvent(event) {
  const key = String(event.key || "");
  if (["Meta", "Control", "Alt", "Shift"].includes(key)) return null;
  const modifiers = [];
  if (event.metaKey || event.ctrlKey) modifiers.push("CommandOrControl");
  if (event.altKey) modifiers.push("Alt");
  if (event.shiftKey) modifiers.push("Shift");
  if (!modifiers.length) return null;
  const normalizedKey = key === " " ? "Space" : key.length === 1 ? key.toUpperCase() : key;
  return [...modifiers, normalizedKey].join("+");
}

export function collectShortcutBindings(rows = []) {
  return [...rows].map((row) => ({
    id: row.dataset.shortcutId,
    action: row.querySelector("[data-shortcut-action]").value,
    accelerator: row.querySelector("[data-shortcut-accelerator]").value,
    enabled: row.querySelector("[data-shortcut-enabled]").value === "true",
  }));
}

export function bindShortcutCenter(root) {
  let recording = null;
  root.addEventListener("click", (event) => {
    const row = event.target.closest("[data-shortcut-id]");
    if (event.target.closest("[data-shortcut-add]")) {
      const list = root.querySelector("[data-shortcut-list]");
      const id = `shortcut-${Date.now().toString(36)}`;
      list.insertAdjacentHTML("beforeend", renderShortcutCenter([{ id, action: "voice_toggle", accelerator: "", enabled: false }]).match(/<article[\s\S]*<\/article>/)?.[0] || "");
    }
    if (!row) return;
    if (event.target.closest("[data-shortcut-toggle]")) {
      const input = row.querySelector("[data-shortcut-enabled]");
      input.value = input.value === "true" ? "false" : "true";
      event.target.closest("[data-shortcut-toggle]").classList.toggle("is-on", input.value === "true");
    }
    if (event.target.closest("[data-shortcut-remove]")) row.remove();
    if (event.target.closest("[data-shortcut-record]")) {
      recording = row;
      row.querySelector("[data-shortcut-record]").classList.add("is-recording");
      row.querySelector("[data-shortcut-record] span").textContent = "请按组合键…";
    }
  });
  root.addEventListener("keydown", (event) => {
    if (!recording) return;
    event.preventDefault();
    const accelerator = acceleratorFromKeyboardEvent(event);
    if (!accelerator) return;
    recording.querySelector("[data-shortcut-accelerator]").value = accelerator;
    recording.querySelector("kbd").textContent = accelerator;
    recording.querySelector("[data-shortcut-enabled]").value = "true";
    recording.querySelector("[data-shortcut-toggle]").classList.add("is-on");
    recording.querySelector("[data-shortcut-record]").classList.remove("is-recording");
    recording.querySelector("[data-shortcut-record] span").textContent = "重新录入";
    recording = null;
  }, true);
}
