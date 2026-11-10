// 顶栏任务助手始终打开文字面板；语音快捷键仍保留原有的开麦行为。

import { activeVoiceShortcut, formatAccelerator, isMacPlatform } from "./voice-shortcut.js";
import { subscribeLocale, t } from "./i18n.js";

export function voiceLauncherView(status = {}, { macos = isMacPlatform(), active = null } = {}) {
  const config = status?.config || status || {};
  const shortcut = formatAccelerator(activeVoiceShortcut(status), macos);
  const isActive = active === null ? status?.expanded === true : Boolean(active);
  const voice = config.enabled === false
    ? t("companion.voice_disabled")
    : status?.blocked
      ? t("companion.microphone_blocked")
      : `${t("companion.voice_shortcut")} ${shortcut}`;
  return { active: isActive, enabled: true, shortcut, title: t("companion.open_hint", { voice }) };
}

export function applyVoiceLauncher(button, view) {
  if (!button) return;
  button.classList.toggle("is-active", view.active);
  button.classList.toggle("is-off", false);
  button.title = view.title;
  button.setAttribute("aria-expanded", view.active ? "true" : "false");
  const key = button.querySelector("[data-voice-shortcut]");
  if (key) {
    key.textContent = view.shortcut;
    key.title = t("companion.voice_shortcut");
  }
}

export function mountVoiceLauncher({ button, call, listen, macos = isMacPlatform(), onError = () => {}, eventTarget = globalThis.window }) {
  if (!button) return null;
  let status = {};
  let disposed = false;
  let unlisten;
  let revision = -1;
  const paint = () => { if (!disposed) applyVoiceLauncher(button, voiceLauncherView(status, { macos })); };
  const accept = (snapshot) => {
    if (disposed || !snapshot || typeof snapshot !== "object") return;
    if (Number.isSafeInteger(snapshot.revision)) {
      if (snapshot.revision < revision) return;
      revision = snapshot.revision;
    }
    status = { ...status, ...snapshot };
    paint();
  };

  const refresh = async () => {
    if (disposed) return;
    try {
      accept(await call("capsule_status"));
    } catch {
      // 读状态失败不阻止用户直接打开文字面板。
    }
    paint();
  };

  const open = async () => {
    if (disposed || button.disabled) return;
    button.disabled = true;
    try {
      accept(await call("capsule_resize", { expanded: true }));
    } catch (error) {
      if (!disposed) onError(new Error(`${t("companion.open_failed")}：${error instanceof Error ? error.message : String(error)}`));
    } finally {
      if (!disposed) button.disabled = false;
    }
  };
  button.addEventListener("click", open);

  const unsubscribeLocale = subscribeLocale(paint);
  const dispose = () => {
    if (disposed) return;
    disposed = true;
    unlisten?.();
    unsubscribeLocale();
    button.removeEventListener?.("click", open);
    eventTarget?.removeEventListener?.("pagehide", dispose);
  };
  eventTarget?.addEventListener?.("pagehide", dispose, { once: true });
  const subscribe = async () => {
    try {
      const stop = await listen?.("lumo://capsule-state", (event) => accept(event?.payload));
      if (disposed) stop?.();
      else unlisten = stop;
    } catch { /* 事件不可用时仍能通过点击打开 */ }
  };
  void subscribe();

  paint();
  void refresh();
  return { refresh, dispose };
}
