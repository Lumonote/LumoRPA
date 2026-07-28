import { bindShortcutCenter, collectShortcutBindings, normalizeShortcutBindings, renderShortcutCenter } from "./shortcut-center.js";

const escapeHtml = (value) => String(value ?? "").replaceAll("&", "&amp;").replaceAll("<", "&lt;").replaceAll(">", "&gt;").replaceAll('"', "&quot;").replaceAll("'", "&#39;");
const checked = (value) => value ? " checked" : "";

const QUICK_ACTIONS = [
  ["open_view", "打开视图"],
  ["run_flow", "运行流程"],
  ["stop", "停止全部"],
  ["mute", "静音"],
  ["unmute", "恢复拾音"],
  ["status", "状态播报"],
  ["listen", "开始听写"],
];

const VOICE_PACKS = [
  ["default", "默认助手"],
  ["lumo", "Lumo AI · 本地"],
];

export function normalizeQuickCommands(list = []) {
  return (Array.isArray(list) ? list : [])
    .map((item, index) => ({
      id: String(item?.id || `qc-${index + 1}`),
      phrase: String(item?.phrase || ""),
      action: QUICK_ACTIONS.some(([action]) => action === item?.action) ? item.action : "open_view",
      argument: String(item?.argument || ""),
      enabled: item?.enabled !== false,
      confirm: item?.confirm === true,
    }))
    .filter((command) => command.phrase.trim());
}

export function exportQuickCommands(commands = []) {
  return JSON.stringify(normalizeQuickCommands(commands), null, 2);
}

export function mergeQuickCommands(existing = [], imported = []) {
  const key = (command) => command.phrase.replaceAll(/\s+/g, "").toLowerCase();
  const merged = new Map(existing.map((command) => [key(command), command]));
  for (const command of imported) merged.set(key(command), command);
  return [...merged.values()];
}

export function normalizeVoiceSettings(status = {}, models = [], devices = []) {
  const config = status.config || status;
  return {
    enabled: config.enabled !== false,
    wakeWordEnabled: Boolean(config.wakeWordEnabled),
    shortcut: config.shortcut || "CommandOrControl+Shift+Space",
    deviceId: config.deviceId || "default",
    sttProfile: config.sttProfile || "local",
    quietMode: Boolean(config.quietMode),
    retainAudio: Boolean(config.retainAudio),
    followUpEnabled: config.followUpEnabled !== false,
    followUpTimeoutSeconds: Number(config.followUpTimeoutSeconds) || 8,
    voicePack: VOICE_PACKS.some(([pack]) => pack === config.voicePack) ? config.voicePack : "default",
    quickCommands: normalizeQuickCommands(config.quickCommands),
    confirmFlowRun: Boolean(config.confirmFlowRun),
    ttsVoice: String(config.ttsVoice || ""),
    ttsRatePercent: config.ttsRatePercent ?? "",
    shortcuts: normalizeShortcutBindings(config.shortcuts, config.shortcut),
    models,
    devices,
  };
}

export function renderQuickCommandRow(command = {}) {
  const action = QUICK_ACTIONS.some(([value]) => value === command.action) ? command.action : "open_view";
  return `<div class="vqc-row" data-quick-command-row data-quick-id="${escapeHtml(command.id || `qc-${Date.now().toString(36)}`)}"><input data-qc-field="phrase" placeholder="短语，如：开工" value="${escapeHtml(command.phrase || "")}"><select data-qc-field="action">${QUICK_ACTIONS.map(([value, label]) => `<option value="${value}"${value === action ? " selected" : ""}>${label}</option>`).join("")}</select><input data-qc-field="argument" placeholder="参数：视图 ID（如 mission-control）或流程名" value="${escapeHtml(command.argument || "")}"><label class="vqc-enabled"><input data-qc-field="enabled" type="checkbox"${checked(command.enabled !== false)}>启用</label><label class="vqc-enabled"><input data-qc-field="confirm" type="checkbox"${checked(command.confirm === true)}>确认</label><button type="button" data-remove-quick-command aria-label="删除指令">×</button></div>`;
}

export function renderVoiceSettings(settings = {}) {
  const models = settings.models || [];
  const devices = settings.devices || [];
  settings = { ...settings, shortcuts: normalizeShortcutBindings(settings.shortcuts, settings.shortcut) };
  const quickCommands = normalizeQuickCommands(settings.quickCommands);
  return `<div class="voice-settings"><div class="voice-settings-grid"><label><span>语音服务</span><input data-voice-setting="enabled" type="checkbox"${checked(settings.enabled !== false)}></label><label><span>本地唤醒词</span><input data-voice-setting="wakeWord" type="checkbox"${checked(settings.wakeWordEnabled)}></label><label><span>输入设备</span><select data-voice-setting="deviceId">${devices.length ? devices.map((device) => `<option value="${escapeHtml(device.id)}"${device.id === settings.deviceId ? " selected" : ""}>${escapeHtml(device.name || device.id)}</option>`).join("") : '<option value="default">System Default</option>'}</select></label><label><span>语音识别</span><select data-voice-setting="sttProfile"><option value="local"${settings.sttProfile !== "cloud" ? " selected" : ""}>本地优先</option><option value="cloud"${settings.sttProfile === "cloud" ? " selected" : ""}>云端配置</option></select></label><label><span>语音包</span><select data-voice-setting="voicePack">${VOICE_PACKS.map(([value, label]) => `<option value="${value}"${(settings.voicePack || "default") === value ? " selected" : ""}>${label}</option>`).join("")}</select><small>Lumo AI 语音包完全本地运行，音色与反馈话术更亲切</small></label><label><span>播报音色</span><input data-voice-setting="ttsVoice" type="text" placeholder="留空用语音包默认，如 zh-CN" value="${escapeHtml(settings.ttsVoice || "")}"><small>音色标识或语言代码，覆盖语音包默认</small></label><label><span>播报语速</span><select data-voice-setting="ttsRatePercent">${[["", "默认"], ["35", "慢速"], ["50", "标准"], ["65", "轻快"], ["80", "快速"]].map(([value, label]) => `<option value="${value}"${String(settings.ttsRatePercent ?? "") === value ? " selected" : ""}>${label}</option>`).join("")}</select><button type="button" data-voice-tts-preview>试听</button></label><label><span>流程运行确认</span><input data-voice-setting="confirmFlowRun" type="checkbox"${checked(settings.confirmFlowRun)}><small>「运行 X 流程」先语音确认再执行</small></label><label><span>安静模式</span><input data-voice-setting="quietMode" type="checkbox"${checked(settings.quietMode)}></label><label><span>连续对话</span><input data-voice-setting="followUpEnabled" type="checkbox"${checked(settings.followUpEnabled !== false)}><small>任务完成后免唤醒续聊</small></label><label><span>续听窗口</span><select data-voice-setting="followUpTimeoutSeconds">${[5, 8, 12, 20].map((seconds) => `<option value="${seconds}"${Number(settings.followUpTimeoutSeconds || 8) === seconds ? " selected" : ""}>${seconds} 秒</option>`).join("")}</select></label><label><span>保留原始音频</span><input data-voice-setting="retainAudio" type="checkbox"${checked(settings.retainAudio)}><small>默认不保留音频</small></label></div><div class="voice-quick-commands" data-quick-commands><div class="vqc-head"><div><strong>语音快捷指令</strong><span>说出短语立即执行，不经 AI 规划。内置：打开任务中心 · 运行 X 流程 · 停止 · 静音 · 当前状态 · 「然后」连接多条指令</span></div><div class="vqc-tools"><button type="button" data-export-quick-commands>导出</button><button type="button" data-import-quick-commands>导入</button><input data-quick-command-file type="file" accept="application/json" hidden><button type="button" data-add-quick-command>添加指令</button></div></div><div class="vqc-rows" data-quick-command-rows>${quickCommands.length ? quickCommands.map((command) => renderQuickCommandRow(command)).join("") : '<div class="vqc-empty">还没有自定义指令，点「添加指令」把常用操作绑定到一句话。</div>'}</div></div>${renderShortcutCenter(settings.shortcuts)}<div class="voice-save-status" data-voice-save-status aria-live="polite"></div><div class="voice-models">${models.length ? models.map((model) => `<article><div><strong>${escapeHtml(model.id)}</strong><span>${escapeHtml(String(model.kind || "voice model").replaceAll("_", " "))}</span></div><em class="is-${escapeHtml(model.status)}">${escapeHtml(String(model.status || "missing").replaceAll("_", " "))}</em><button data-model-action="${model.status === "installed" ? "remove" : "install"}" data-model-id="${escapeHtml(model.id)}">${model.status === "installed" ? "移除" : "安装 / 修复"}</button></article>`).join("") : '<div class="hub-empty"><strong>未发现本地语音模型</strong><span>可继续使用快捷键与配置的云端 STT。</span></div>'}</div><button class="primary" data-save-voice-settings>保存语音设置</button></div>`;
}

export function collectQuickCommands(rows = []) {
  return [...rows]
    .map((row, index) => ({
      id: String(row?.dataset?.quickId || `qc-${index + 1}`),
      phrase: String(row.querySelector('[data-qc-field="phrase"]')?.value || "").trim(),
      action: String(row.querySelector('[data-qc-field="action"]')?.value || "open_view"),
      argument: String(row.querySelector('[data-qc-field="argument"]')?.value || "").trim(),
      enabled: row.querySelector('[data-qc-field="enabled"]')?.checked !== false,
      confirm: row.querySelector('[data-qc-field="confirm"]')?.checked === true,
    }))
    .filter((command) => command.phrase);
}

export function serializeVoiceSettings(fields, shortcuts = [], quickCommands = []) {
  const shortcut = shortcuts.find((binding) => binding.action === "voice_toggle" && binding.enabled)?.accelerator || "";
  const ratePercent = String(fields.ttsRatePercent?.value ?? "").trim();
  return { enabled: Boolean(fields.enabled?.checked), wakeWordEnabled: Boolean(fields.wakeWord?.checked), shortcut, deviceId: fields.deviceId?.value || "default", sttProfile: fields.sttProfile?.value || "local", quietMode: Boolean(fields.quietMode?.checked), retainAudio: Boolean(fields.retainAudio?.checked), followUpEnabled: fields.followUpEnabled?.checked !== false, followUpTimeoutSeconds: Number(fields.followUpTimeoutSeconds?.value) || 8, voicePack: fields.voicePack?.value || "default", quickCommands, confirmFlowRun: Boolean(fields.confirmFlowRun?.checked), ttsVoice: (fields.ttsVoice?.value || "").trim() || null, ttsRatePercent: ratePercent ? Number(ratePercent) : null, shortcuts };
}

export function bindVoiceSettings(root, call) {
  if (!root || root.dataset.bound) return;
  root.dataset.bound = "true";
  bindShortcutCenter(root);
  root.addEventListener("change", async (event) => {
    const file = event.target.closest("[data-quick-command-file]");
    if (!file || !file.files?.length) return;
    const status = root.querySelector("[data-voice-save-status]");
    try {
      const imported = normalizeQuickCommands(JSON.parse(await file.files[0].text()));
      const merged = mergeQuickCommands(collectQuickCommands(root.querySelectorAll("[data-quick-command-row]")), imported);
      const container = root.querySelector("[data-quick-command-rows]");
      if (container) container.innerHTML = merged.length ? merged.map((command) => renderQuickCommandRow(command)).join("") : '<div class="vqc-empty">还没有自定义指令，点「添加指令」把常用操作绑定到一句话。</div>';
      if (status) { status.className = "voice-save-status is-success"; status.textContent = `已导入并合并为 ${merged.length} 条指令，记得点「保存语音设置」`; }
    } catch (error) {
      if (status) { status.className = "voice-save-status is-error"; status.textContent = `导入失败：${error?.message || error}`; }
    }
    file.value = "";
  });
  root.addEventListener("click", async (event) => {
    if (event.target.closest("[data-voice-tts-preview]")) {
      await call("voice_tts_preview").catch(() => {});
      return;
    }
    if (event.target.closest("[data-export-quick-commands]")) {
      const payload = exportQuickCommands(collectQuickCommands(root.querySelectorAll("[data-quick-command-row]")));
      const link = document.createElement("a");
      link.href = URL.createObjectURL(new Blob([payload], { type: "application/json" }));
      link.download = "lumo-voice-commands.json";
      link.click();
      URL.revokeObjectURL(link.href);
      return;
    }
    if (event.target.closest("[data-import-quick-commands]")) {
      root.querySelector("[data-quick-command-file]")?.click();
      return;
    }
    if (event.target.closest("[data-add-quick-command]")) {
      const container = root.querySelector("[data-quick-command-rows]");
      if (container) {
        container.querySelector(".vqc-empty")?.remove();
        container.insertAdjacentHTML("beforeend", renderQuickCommandRow({ id: `qc-${Date.now().toString(36)}${Math.floor(Math.random() * 1000)}` }));
      }
      return;
    }
    const removeQuick = event.target.closest("[data-remove-quick-command]");
    if (removeQuick) {
      removeQuick.closest("[data-quick-command-row]")?.remove();
      return;
    }
    if (event.target.closest("[data-save-voice-settings]")) {
      const fields = Object.fromEntries([...root.querySelectorAll("[data-voice-setting]")].map((input) => [input.dataset.voiceSetting, input]));
      const status = root.querySelector("[data-voice-save-status]");
      try {
        const quickCommands = collectQuickCommands(root.querySelectorAll("[data-quick-command-row]"));
        await call("voice_configure", { config: serializeVoiceSettings(fields, collectShortcutBindings(root.querySelectorAll("[data-shortcut-id]")), quickCommands) });
        if (status) { status.className = "voice-save-status is-success"; status.textContent = "校验通过，语音设置已生效"; }
      } catch (error) {
        if (status) { status.className = "voice-save-status is-error"; status.textContent = `保存失败：${error?.message || error}`; }
      }
    }
    const model = event.target.closest("[data-model-action]");
    if (model) await call(model.dataset.modelAction === "install" ? "voice_model_install" : "voice_model_remove", { modelId: model.dataset.modelId });
  });
}
