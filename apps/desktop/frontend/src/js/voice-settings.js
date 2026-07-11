const escapeHtml = (value) => String(value ?? "").replaceAll("&", "&amp;").replaceAll("<", "&lt;").replaceAll(">", "&gt;").replaceAll('"', "&quot;").replaceAll("'", "&#39;");
const checked = (value) => value ? " checked" : "";

export function renderVoiceSettings(settings = {}) {
  const models = settings.models || [];
  return `<div class="voice-settings"><div class="voice-settings-grid"><label><span>本地唤醒词</span><input data-voice-setting="wakeWord" type="checkbox"${checked(settings.wakeWordEnabled)}></label><label><span>全局快捷键</span><input data-voice-setting="shortcut" value="${escapeHtml(settings.shortcut || "Option+Space")}" aria-label="全局语音快捷键"></label><label><span>语音识别</span><select data-voice-setting="sttProfile"><option value="local"${settings.sttProfile !== "cloud" ? " selected" : ""}>本地优先</option><option value="cloud"${settings.sttProfile === "cloud" ? " selected" : ""}>云端配置</option></select></label><label><span>安静模式</span><input data-voice-setting="quietMode" type="checkbox"${checked(settings.quietMode)}></label><label><span>保留原始音频</span><input data-voice-setting="retainAudio" type="checkbox"${checked(settings.retainAudio)}><small>默认不保留音频</small></label></div><div class="voice-models">${models.length ? models.map((model) => `<article><div><strong>${escapeHtml(model.id)}</strong><span>${escapeHtml(String(model.kind || "voice model").replaceAll("_", " "))}</span></div><em class="is-${escapeHtml(model.status)}">${escapeHtml(String(model.status || "missing").replaceAll("_", " "))}</em><button data-model-action="${model.status === "installed" ? "remove" : "install"}" data-model-id="${escapeHtml(model.id)}">${model.status === "installed" ? "移除" : "安装 / 修复"}</button></article>`).join("") : '<div class="hub-empty"><strong>未发现本地语音模型</strong><span>可继续使用快捷键与配置的云端 STT。</span></div>'}</div><button class="primary" data-save-voice-settings>保存语音设置</button></div>`;
}

export function serializeVoiceSettings(fields) {
  return { wakeWordEnabled: Boolean(fields.wakeWord?.checked), shortcut: fields.shortcut?.value || "", sttProfile: fields.sttProfile?.value || "local", quietMode: Boolean(fields.quietMode?.checked), retainAudio: Boolean(fields.retainAudio?.checked) };
}

export function bindVoiceSettings(root, call) {
  if (!root || root.dataset.bound) return;
  root.dataset.bound = "true";
  root.addEventListener("click", async (event) => {
    if (event.target.closest("[data-save-voice-settings]")) {
      const fields = Object.fromEntries([...root.querySelectorAll("[data-voice-setting]")].map((input) => [input.dataset.voiceSetting, input]));
      await call("voice_configure", { config: serializeVoiceSettings(fields) });
    }
    const model = event.target.closest("[data-model-action]");
    if (model) await call(model.dataset.modelAction === "install" ? "voice_model_install" : "voice_model_remove", { modelId: model.dataset.modelId });
  });
}

