import test from "node:test";
import assert from "node:assert/strict";

import { activeVoiceShortcut, formatAccelerator, isMacPlatform, recallHint } from "../src/js/voice-shortcut.js";
import { applyVoiceLauncher, mountVoiceLauncher, voiceLauncherView } from "../src/js/voice-launcher.js";
import { setLocale } from "../src/js/i18n.js";

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
    removeEventListener: (name, handler) => { if (listeners[name] === handler) delete listeners[name]; },
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
  assert.match(recallHint({}, true), /顶栏「任务助手」打开文字面板/);
  assert.match(recallHint({}, true), /语音输入可按/);
});

test("launcher describes text input and marks the displayed shortcut as voice input", () => {
  const idle = voiceLauncherView({ state: "idle", config: { enabled: true, shortcut: "Alt+L" } }, { macos: true });
  assert.equal(idle.active, false);
  assert.equal(idle.shortcut, "⌥L");
  assert.match(idle.title, /打开 Lumo 任务助手/);
  assert.match(idle.title, /文字输入无需麦克风/);
  assert.match(idle.title, /语音输入快捷键 ⌥L/);
  assert.match(idle.title, /可拖动/);

  const active = voiceLauncherView({ state: "idle", expanded: true }, { macos: true });
  assert.equal(active.active, true);
  assert.match(active.title, /打开/);
  assert.doesNotMatch(active.title, /收起/);
  assert.equal(voiceLauncherView({ state: "listening", expanded: false }).active, false);

  const off = voiceLauncherView({ state: "idle", config: { enabled: false, shortcut: "Alt+L" } }, { macos: true });
  assert.equal(off.enabled, true);
  assert.match(off.title, /语音已关闭，文字仍可用/);
  const blocked = voiceLauncherView({ blocked: true, config: { enabled: true } });
  assert.equal(blocked.enabled, true);
  assert.match(blocked.title, /麦克风不可用，文字仍可用/);
});

test("applies launcher state onto the topbar button", () => {
  const button = fakeButton();
  applyVoiceLauncher(button, { active: true, enabled: true, shortcut: "⌘⇧Space", title: "打开任务助手" });
  assert.equal(button.classList.contains("is-active"), true);
  assert.equal(button.classList.contains("is-off"), false);
  assert.equal(button.title, "打开任务助手");
  assert.equal(button.key.textContent, "⌘⇧Space");
  assert.equal(button.key.title, "语音输入快捷键");
  assert.equal(button.attributes["aria-expanded"], "true");
});

test("clicking the launcher opens the text panel and follows capsule expansion events", async (context) => {
  const button = fakeButton();
  const calls = [];
  let listener = null;
  const call = async (command, args) => {
    calls.push({ command, args });
    if (command === "capsule_status") return { state: "idle", expanded: false, config: { enabled: true, shortcut: "Alt+L" } };
    assert.equal(command, "capsule_resize");
    assert.deepEqual(args, { expanded: true });
    return { state: "idle", expanded: true, config: { enabled: true, shortcut: "Alt+L" } };
  };
  const controller = mountVoiceLauncher({
    button,
    call,
    listen: (name, handler) => { listener = { name, handler }; },
    macos: true,
  });
  context.after(controller.dispose);
  await controller.refresh();
  assert.deepEqual(calls.map((entry) => entry.command), ["capsule_status", "capsule_status"]);
  assert.equal(button.classList.contains("is-active"), false);
  assert.equal(button.key.textContent, "⌥L");

  await button.listeners.click();
  assert.equal(calls.at(-1).command, "capsule_resize");
  assert.equal(button.classList.contains("is-active"), true);
  assert.equal(button.disabled, false);

  assert.equal(listener.name, "lumo://capsule-state");
  listener.handler({ payload: { state: "listening", expanded: false } });
  assert.equal(button.classList.contains("is-active"), false);
  assert.equal(button.attributes["aria-expanded"], "false");
});

for (const snapshot of [
  { config: { enabled: false }, state: "idle" },
  { config: { enabled: true }, state: "error", blocked: true },
  { config: { enabled: false }, state: "muted", muted: true, blocked: true },
]) {
  test(`text launcher works with voice ${snapshot.state} and enabled=${snapshot.config.enabled}`, async (context) => {
    const button = fakeButton();
    const calls = [];
    const controller = mountVoiceLauncher({
      button,
      call: async (command, args) => {
        calls.push({ command, args });
        if (command === "capsule_status") return { ...snapshot, expanded: false };
        assert.equal(command, "capsule_resize");
        assert.deepEqual(args, { expanded: true });
        return { ...snapshot, expanded: true, listening: false };
      },
      macos: false,
    });
    context.after(controller.dispose);
    await controller.refresh();
    assert.equal(button.disabled, false);
    assert.equal(button.classList.contains("is-off"), false);
    await button.listeners.click();
    assert.equal(button.attributes["aria-expanded"], "true");
    assert.equal(button.disabled, false);
    assert.ok(calls.every(({ command }) => ["capsule_status", "capsule_resize"].includes(command)));
  });
}

test("failed status reads leave the text launcher usable without voice commands", async (context) => {
  const button = fakeButton();
  const calls = [];
  const errors = [];
  const controller = mountVoiceLauncher({
    button,
    call: async (command, args) => {
      calls.push(command);
      if (command === "capsule_status") throw new Error("status unavailable");
      assert.equal(command, "capsule_resize");
      assert.deepEqual(args, { expanded: true });
      return { expanded: true };
    },
    onError: (error) => errors.push(error),
  });
  context.after(controller.dispose);
  await controller.refresh();
  await button.listeners.click();
  assert.deepEqual(calls, ["capsule_status", "capsule_status", "capsule_resize"]);
  assert.deepEqual(errors, []);
  assert.equal(button.attributes["aria-expanded"], "true");
});

test("launcher reports an opening failure with task-assistant context", async (context) => {
  const button = fakeButton();
  const errors = [];
  const controller = mountVoiceLauncher({
    button,
    call: async () => { throw new Error("Tauri API unavailable"); },
    listen: undefined,
    macos: false,
    onError: (error) => errors.push(error),
  });
  context.after(controller.dispose);
  await controller.refresh();
  // 读状态失败只是保持默认展示，不该冒泡成错误提示。
  assert.deepEqual(errors, []);
  assert.equal(button.key.textContent, "Ctrl+Shift+Space");

  await button.listeners.click();
  // 用户主动点击失败要报出来。
  assert.equal(errors.length, 1);
  assert.match(errors[0].message, /无法打开任务助手：Tauri API unavailable/);
  assert.equal(button.disabled, false);
});

test("launcher suppresses repeated clicks and stale status snapshots", async (context) => {
  const button = fakeButton();
  let resolveStatus, resolveOpen, receive;
  const calls = [];
  const controller = mountVoiceLauncher({
    button,
    call: (command) => {
      calls.push(command);
      if (command === "capsule_status") return new Promise((resolve) => { resolveStatus = resolve; });
      return new Promise((resolve) => { resolveOpen = resolve; });
    },
    listen: (_name, handler) => { receive = handler; },
  });
  context.after(controller.dispose);
  const opening = button.listeners.click();
  await button.listeners.click();
  assert.deepEqual(calls, ["capsule_status", "capsule_resize"]);
  receive({ payload: { revision: 3, expanded: true } });
  resolveStatus({ revision: 1, expanded: false });
  await Promise.resolve();
  assert.equal(button.attributes["aria-expanded"], "true");
  resolveOpen({ revision: 2, expanded: false });
  await opening;
  assert.equal(button.attributes["aria-expanded"], "true");
  assert.equal(button.disabled, false);
});

test("pagehide disposes the launcher and releases a late listener subscription once", async () => {
  const button = fakeButton();
  const pageListeners = new Map();
  let resolveSubscription, receive;
  let stopped = 0;
  let calls = 0;
  const controller = mountVoiceLauncher({
    button,
    call: async () => { calls += 1; return { expanded: false }; },
    listen: (_name, handler) => {
      receive = handler;
      return new Promise((resolve) => { resolveSubscription = resolve; });
    },
    eventTarget: {
      addEventListener: (name, handler) => pageListeners.set(name, handler),
      removeEventListener: (name, handler) => { if (pageListeners.get(name) === handler) pageListeners.delete(name); },
    },
  });
  pageListeners.get("pagehide")();
  controller.dispose();
  resolveSubscription(() => { stopped += 1; });
  await Promise.resolve();
  await controller.refresh();
  receive({ payload: { expanded: true } });
  assert.equal(stopped, 1);
  assert.equal(calls, 1);
  assert.equal(button.listeners.click, undefined);
  assert.equal(pageListeners.size, 0);
  assert.equal(button.attributes["aria-expanded"], "false");
});

test("launcher text follows locale updates until disposed", (context) => {
  const button = fakeButton();
  const controller = mountVoiceLauncher({ button, call: async () => ({}) });
  context.after(() => { controller.dispose(); setLocale("zh-CN"); });
  setLocale("en");
  assert.match(button.title, /Open Lumo task assistant/);
  assert.equal(button.key.title, "Voice input shortcut");
  controller.dispose();
  setLocale("zh-CN");
  assert.match(button.title, /Open Lumo task assistant/);
});
