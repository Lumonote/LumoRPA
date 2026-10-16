// Settings view: language switch + environment info + discovered Skills list.

import { $, $$, html, kv, toast } from "./dom.js";
import { call } from "./api.js";
import { state } from "./state.js";
import { bindOnce } from "./bind-once.js";
import { getLocale, setLocale, t } from "./i18n.js";
import { bindVoiceSettings, normalizeVoiceSettings, renderVoiceSettings } from "./voice-settings.js";
import { bindUpdateCenter, renderUpdateCenter } from "./update-center.js";

// #localeRow 是长驻节点（不随 innerHTML 重建），必须 bind-once 防止每次
// refreshSettings 叠加一层监听。
export function renderLocaleRow() {
  const row = $("localeRow");
  if (!row) return;
  bindOnce(row, "locale-switch", (el) => el.addEventListener("click", (event) => {
    const button = event.target.closest("[data-locale]");
    if (button) setLocale(button.dataset.locale);
  }));
  $$("#localeRow [data-locale]").forEach((button) => button.classList.toggle("is-active", button.dataset.locale === getLocale()));
}

export async function refreshSettings() {
  renderLocaleRow();
  const [info, skills, voiceStatus, voiceModels, voiceDevices, updateStatus, autostart] = await Promise.all([call("app_info"), call("list_skills"), call("voice_status").catch(() => ({})), call("voice_model_list").catch(() => []), call("voice_devices").catch(() => []), call("desktop_update_status").catch(() => ({ currentVersion: "unknown", channel: "stable", configured: false })), call("autostart_status").catch(() => null)]);
  const voice = normalizeVoiceSettings(voiceStatus, voiceModels, voiceDevices);
  state.app = info;
  $("environmentBox").innerHTML = [
    kv("版本", info.version),
    kv("平台", `${info.platform} ${info.arch}`),
    kv("应用数据", info.dataDir),
    kv("Providers", info.providersPath),
    kv("Skills 根", info.skillsPath),
    kv("Examples", info.examplesDir || "-"),
    kv("LLM 网络", info.networkEnabled ? "已开启 (本次会话或环境变量)" : "未开启"),
    `<div class="kv"><span>开机自启</span><strong><label style="display:inline-flex;align-items:center;gap:6px;cursor:pointer"><input type="checkbox" id="autostartToggle"${autostart ? " checked" : ""}${autostart === null ? " disabled" : ""}/>${autostart === null ? "不可用" : "登录时自动启动 LumoRPA"}</label></strong></div>`,
  ].join("");
  $("autostartToggle")?.addEventListener("change", async (e) => {
    const box = e.target;
    try {
      const enabled = await call("autostart_set", { enabled: box.checked });
      box.checked = enabled;
      toast(t("settings.autostart"), enabled ? t("settings.autostart_on") : t("settings.autostart_off"), "ok");
    } catch (err) {
      box.checked = !box.checked;
      toast("开机自启设置失败", String(err), "bad");
    }
  });
  $("skillsBox").innerHTML = skills.length
    ? skills.map((s) => kv(s.name, s.description || s.source)).join("")
    : `<div class="kv"><span>暂无</span><strong>把 SKILL.md 放到 ${html(info.skillsPath)}</strong></div>`;
  $("voiceSettingsBox").innerHTML = renderVoiceSettings(voice);
  bindVoiceSettings($("voiceSettingsBox"), call);
  if ($("updateCenterBox")) {
    $("updateCenterBox").innerHTML = renderUpdateCenter(updateStatus);
    bindUpdateCenter($("updateCenterBox"), call);
  }
  $("appMeta").textContent = `${info.platform} ${info.arch} · v${info.version}`;
  $("versionPill").lastChild.textContent = `v${info.version}`;
}
