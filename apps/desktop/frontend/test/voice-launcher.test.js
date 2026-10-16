import test from "node:test";
import assert from "node:assert/strict";

import { activeVoiceShortcut, formatAccelerator, isMacPlatform, recallHint } from "../src/js/voice-shortcut.js";
import { applyVoiceLauncher, isActiveVoiceState, mountVoiceLauncher, voiceIsActive, voiceLauncherView } from "../src/js/voice-launcher.js";

function fakeButton() {
  const classes = new Set();
  const key = { textContent: "" };
  const listeners = {};
  const button = {
    key,
    listeners,
    disabled: false,
    title: "",
    attributes: {},
    classList: {
      toggle: (name, on) => (on ? classes.add(name) : classes.delete(name)),
      contains: (name) => classes.has(name),
    },
    setAttribute: (name, value) => { button.attributes[name] = value; },
    querySelector: (selector) => (selector === "[data-voice-shortcut]" ? key : null),
    addEventListener: (name, handler) => { listeners[name] = handler; },
  };
  return button;
}

test("picks the enabled voice_toggle binding over the legacy shortcut field", () => {
  const status = {
    config: {
      shortcut: "CommandOrControl+Shift+Space",
      shortcuts: [
        { id: "a", action: "push_to_talk", accelerator: "Alt+X", enabled: true },
        { id: "b", action: "voice_toggle", accelerator: "Alt+L", enabled: true },
      ],
    },
  };
  assert.equal(activeVoiceShortcut(status), "Alt+L");
  // 绑定被禁用时退回 config.shortcut，都没有时退回内置默认值。
  status.config.shortcuts[1].enabled = false;
  assert.equal(activeVoiceShortcut(status), "CommandOrControl+Shift+Space");
  assert.equal(activeVoiceShortcut({}), "CommandOrControl+Shift+Space");
});

test("formats accelerators per platform", () => {
  assert.equal(formatAccelerator("CommandOrControl+Shift+Space", true), "⌘⇧Space");
  assert.equal(formatAccelerator("CommandOrControl+Shift+Space", false), "Ctrl+Shift+Space");
  assert.equal(formatAccelerator("Alt+L", true), "⌥L");
  assert.equal(formatAccelerator("", true), "未设置快捷键");
  assert.equal(isMacPlatform("MacIntel"), true);
  assert.equal(isMacPlatform("Win32"), false);
  assert.match(recallHint({}, true), /⌘⇧Space/);
  assert.match(recallHint({}, true), /顶栏「语音」/);
});

test("classifies every active voice state as listening", () => {
  for (const state of ["listening", "transcribing", "routing", "confirming", "executing", "reporting"]) {
    assert.equal(isActiveVoiceState(state), true, state);
  }
  for (const state of ["idle", "success", "error", "muted", "", null]) {
    assert.equal(isActiveVoiceState(state), false, String(state));
  }
  assert.equal(voiceIsActive({ listening: true, state: "idle" }), true);
  assert.equal(voiceIsActive({ state: "executing" }), true);
  assert.equal(voiceIsActive({ state: "idle" }), false);
});

test("launcher tooltip explains how to summon, collapse and reconfigure", () => {
  const idle = voiceLauncherView({ state: "idle", config: { enabled: true, shortcut: "Alt+L" } }, { macos: true });
  assert.equal(idle.active, false);
  assert.equal(idle.shortcut, "⌥L");
  assert.match(idle.title, /唤起/);
  assert.match(idle.title, /可拖动/);
  assert.match(idle.title, /设置 → 桌面语音与隐私/);

  const active = voiceLauncherView({ state: "listening", config: { enabled: true, shortcut: "Alt+L" } }, { macos: true });
  assert.equal(active.active, true);
  assert.match(active.title, /收起/);

  const off = voiceLauncherView({ state: "idle", config: { enabled: false, shortcut: "Alt+L" } }, { macos: true });
  assert.equal(off.enabled, false);
  assert.match(off.title, /已关闭/);
});

test("applies launcher state onto the topbar button", () => {
  const button = fakeButton();
  applyVoiceLauncher(button, { active: true, enabled: true, shortcut: "⌘⇧Space", title: "收起" });
  assert.equal(button.classList.contains("is-active"), true);
  assert.equal(button.classList.contains("is-off"), false);
  assert.equal(button.title, "收起");
  assert.equal(button.key.textContent, "⌘⇧Space");
});

test("clicking the launcher toggles voice and repaints from the returned status", async () => {
  const button = fakeButton();
  const calls = [];
  let listener = null;
  const call = async (command) => {
    calls.push(command);
    if (command === "voice_status") return { state: "idle", config: { enabled: true, shortcut: "Alt+L" } };
    return { state: "listening", config: { enabled: true, shortcut: "Alt+L" } };
  };
  const controller = mountVoiceLauncher({
    button,
    call,
    listen: (name, handler) => { listener = { name, handler }; },
    macos: true,
  });
  await controller.refresh();
  assert.deepEqual(calls, ["voice_status", "voice_status"]);
  assert.equal(button.classList.contains("is-active"), false);
  assert.equal(button.key.textContent, "⌥L");

  await button.listeners.click();
  assert.equal(calls.at(-1), "voice_toggle");
  assert.equal(button.classList.contains("is-active"), true);
  assert.equal(button.disabled, false);

  // 快捷键/托盘触发的状态变化只有事件，没有命令返回值。
  assert.equal(listener.name, "lumo://voice-state");
  listener.handler({ payload: { state: "idle", listening: false } });
  assert.equal(button.classList.contains("is-active"), false);
});

test("launcher survives an unavailable voice backend", async () => {
  const button = fakeButton();
  const errors = [];
  const controller = mountVoiceLauncher({
    button,
    call: async () => { throw new Error("Tauri API unavailable"); },
    listen: undefined,
    macos: false,
    onError: (error) => errors.push(error),
  });
  await controller.refresh();
  // 读状态失败只是保持默认展示，不该冒泡成错误提示。
  assert.deepEqual(errors, []);
  assert.equal(button.key.textContent, "Ctrl+Shift+Space");

  await button.listeners.click();
  // 用户主动点击失败要报出来。
  assert.equal(errors.length, 1);
  assert.equal(button.disabled, false);
});
