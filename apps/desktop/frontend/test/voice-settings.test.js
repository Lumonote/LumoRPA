import test from "node:test";
import assert from "node:assert/strict";

import { renderVoiceSettings, serializeVoiceSettings } from "../src/js/voice-settings.js";

test("renders privacy-first voice defaults and model health", () => {
  const html = renderVoiceSettings({ wakeWordEnabled: true, shortcut: "Option+Space", sttProfile: "local", retainAudio: false, models: [{ id: "kws-zh", status: "checksum_failed" }] });
  assert.match(html, /Option\+Space/);
  assert.match(html, /本地优先/);
  assert.match(html, /默认不保留音频/);
  assert.match(html, /checksum failed/i);
});

test("serializes settings without inventing audio retention", () => {
  const values = serializeVoiceSettings({ wakeWord: { checked: true }, shortcut: { value: "Cmd+Shift+L" }, sttProfile: { value: "cloud" }, quietMode: { checked: true }, retainAudio: { checked: false } });
  assert.deepEqual(values, { wakeWordEnabled: true, shortcut: "Cmd+Shift+L", sttProfile: "cloud", quietMode: true, retainAudio: false });
});

