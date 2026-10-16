// 语音唤起快捷键的读取与展示。主窗口的顶栏按钮和悬浮胶囊都要告诉用户
// 「收起之后按什么能叫回来」，所以格式化逻辑放在这里共用。

export const DEFAULT_VOICE_SHORTCUT = "CommandOrControl+Shift+Space";

const MAC_KEYS = {
  commandorcontrol: "⌘", cmdorctrl: "⌘", command: "⌘", cmd: "⌘", super: "⌘", meta: "⌘",
  control: "⌃", ctrl: "⌃", alt: "⌥", option: "⌥", shift: "⇧",
};

const PC_KEYS = {
  commandorcontrol: "Ctrl", cmdorctrl: "Ctrl", control: "Ctrl", ctrl: "Ctrl",
  command: "Win", cmd: "Win", super: "Win", meta: "Win",
  alt: "Alt", option: "Alt", shift: "Shift",
};

export function isMacPlatform(platform = globalThis.navigator?.platform ?? "") {
  return /mac/i.test(String(platform));
}

// voice_status 返回 { config } 或直接是 config；取当前生效的 voice_toggle 绑定。
export function activeVoiceShortcut(status = {}) {
  const config = status?.config || status || {};
  const bindings = Array.isArray(config.shortcuts) ? config.shortcuts : [];
  const bound = bindings.find(
    (binding) => binding?.action === "voice_toggle" && binding?.enabled !== false && String(binding?.accelerator || "").trim(),
  );
  const accelerator = String(bound?.accelerator || config.shortcut || "").trim();
  return accelerator || DEFAULT_VOICE_SHORTCUT;
}

// `CommandOrControl+Shift+Space` → macOS 上是 `⌘⇧Space`，其它平台是 `Ctrl+Shift+Space`。
export function formatAccelerator(accelerator, macos = isMacPlatform()) {
  const parts = String(accelerator || "").split("+").map((part) => part.trim()).filter(Boolean);
  if (!parts.length) return "未设置快捷键";
  const table = macos ? MAC_KEYS : PC_KEYS;
  return parts.map((part) => table[part.toLowerCase()] || part).join(macos ? "" : "+");
}

// 语音被停掉时，胶囊会整个消失，这句话负责告诉用户怎么把它请回来。
export function recallHint(status, macos = isMacPlatform()) {
  return `按 ${formatAccelerator(activeVoiceShortcut(status), macos)} 或点击顶栏「语音」重新唤起`;
}
