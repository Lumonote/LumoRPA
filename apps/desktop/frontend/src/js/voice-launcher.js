// 顶栏「语音」按钮：胶囊收起后的固定唤起入口，同时把当前快捷键摆在明面上。
// 胶囊本身是一个无边框的独立窗口，关掉之后主界面必须留下可见的回程。

import { activeVoiceShortcut, formatAccelerator, isMacPlatform } from "./voice-shortcut.js";

const ACTIVE_STATES = new Set(["listening", "transcribing", "routing", "confirming", "executing", "reporting"]);

export function isActiveVoiceState(state) {
  return ACTIVE_STATES.has(String(state ?? "").toLowerCase());
}

// 事件与命令返回的 payload 形状不同，统一按「显式 listening 或活跃状态名」判定。
export function voiceIsActive(payload = {}) {
  return payload?.listening === true || isActiveVoiceState(payload?.state);
}

export function voiceLauncherView(status = {}, { macos = isMacPlatform(), active = null } = {}) {
  const config = status?.config || status || {};
  const enabled = config.enabled !== false;
  const shortcut = formatAccelerator(activeVoiceShortcut(status), macos);
  const isActive = active === null ? voiceIsActive(status) : Boolean(active);
  const title = !enabled
    ? "语音服务已关闭 · 到 设置 → 桌面语音与隐私 开启"
    : isActive
      ? `收起 Lumo 语音悬浮窗（${shortcut}）`
      : `唤起 Lumo 语音悬浮窗（${shortcut}）· 悬浮窗可拖动；快捷键在 设置 → 桌面语音与隐私 修改`;
  return { active: isActive, enabled, shortcut, title };
}

export function applyVoiceLauncher(button, view) {
  if (!button) return;
  button.classList.toggle("is-active", view.active);
  button.classList.toggle("is-off", !view.enabled);
  button.title = view.title;
  button.setAttribute("aria-pressed", view.active ? "true" : "false");
  const key = button.querySelector("[data-voice-shortcut]");
  if (key) key.textContent = view.shortcut;
}

export function mountVoiceLauncher({ button, call, listen, macos = isMacPlatform(), onError = () => {} }) {
  if (!button) return null;
  let status = {};
  let active = false;
  const paint = () => applyVoiceLauncher(button, voiceLauncherView(status, { macos, active }));

  const refresh = async () => {
    try {
      status = await call("voice_status");
      active = voiceIsActive(status);
    } catch {
      // 语音未就绪（或不在 Tauri 里跑）时保留默认展示，按钮仍然可点。
    }
    paint();
  };

  button.addEventListener("click", async () => {
    button.disabled = true;
    try {
      status = await call("voice_toggle");
      active = voiceIsActive(status);
      paint();
    } catch (error) {
      onError(error);
    } finally {
      button.disabled = false;
    }
  });

  Promise.resolve(listen?.("lumo://voice-state", (event) => {
    active = voiceIsActive(event?.payload);
    paint();
  })).catch(() => {});

  paint();
  refresh();
  return { refresh };
}
