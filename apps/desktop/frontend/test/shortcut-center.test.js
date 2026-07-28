import test from "node:test";
import assert from "node:assert/strict";
import { acceleratorFromKeyboardEvent, normalizeShortcutBindings, renderShortcutCenter } from "../src/js/shortcut-center.js";

test("migrates legacy voice shortcut into the shortcut center", () => {
  assert.deepEqual(normalizeShortcutBindings([], "CommandOrControl+Shift+L"), [{ id: "voice-toggle", action: "voice_toggle", accelerator: "CommandOrControl+Shift+L", enabled: true }]);
});

test("captures desktop accelerator combinations", () => {
  assert.equal(acceleratorFromKeyboardEvent({ key: " ", metaKey: true, ctrlKey: false, altKey: false, shiftKey: true }), "CommandOrControl+Shift+Space");
  assert.equal(acceleratorFromKeyboardEvent({ key: "m", metaKey: false, ctrlKey: true, altKey: true, shiftKey: false }), "CommandOrControl+Alt+M");
  assert.equal(acceleratorFromKeyboardEvent({ key: "m", metaKey: false, ctrlKey: false, altKey: false, shiftKey: false }), null);
});

test("renders action choices and recording controls", () => {
  const html = renderShortcutCenter([{ id: "mission", action: "mission_control", accelerator: "CommandOrControl+Shift+M", enabled: true }]);
  assert.match(html, /桌面快捷键中心/);
  assert.match(html, /Mission Control/);
  assert.match(html, /CommandOrControl\+Shift\+M/);
  assert.match(html, /data-shortcut-record/);
});
