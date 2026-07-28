import test from "node:test";
import assert from "node:assert/strict";

import { collectQuickCommands, exportQuickCommands, mergeQuickCommands, normalizeQuickCommands, normalizeVoiceSettings, renderQuickCommandRow, renderVoiceSettings, serializeVoiceSettings } from "../src/js/voice-settings.js";

test("renders privacy-first voice defaults and model health", () => {
  const html = renderVoiceSettings({ wakeWordEnabled: true, shortcut: "Option+Space", sttProfile: "local", retainAudio: false, models: [{ id: "kws-zh", status: "checksum_failed" }] });
  assert.match(html, /Option\+Space/);
  assert.match(html, /本地优先/);
  assert.match(html, /默认不保留音频/);
  assert.match(html, /连续对话/);
  assert.match(html, /data-voice-save-status/);
  assert.match(html, /checksum failed/i);
});

test("renders local lumo voice pack selector and quick command editor", () => {
  const html = renderVoiceSettings({ voicePack: "lumo", ttsVoice: "zh-CN", ttsRatePercent: 65, confirmFlowRun: true, quickCommands: [{ id: "qc-1", phrase: "开工", action: "run_flow", argument: "晨间日报", enabled: true, confirm: true }] });
  assert.match(html, /data-voice-setting="voicePack"/);
  assert.match(html, /value="lumo" selected/);
  assert.match(html, /Lumo AI · 本地/);
  assert.match(html, /完全本地运行/);
  assert.match(html, /data-voice-setting="ttsVoice"/);
  assert.match(html, /value="zh-CN"/);
  assert.match(html, /value="65" selected/);
  assert.match(html, /data-voice-tts-preview/);
  assert.match(html, /data-voice-setting="confirmFlowRun" type="checkbox" checked/);
  assert.match(html, /data-quick-commands/);
  assert.match(html, /data-add-quick-command/);
  assert.match(html, /data-export-quick-commands/);
  assert.match(html, /data-import-quick-commands/);
  assert.match(html, /data-quick-id="qc-1"/);
  assert.match(html, /value="开工"/);
  assert.match(html, /value="run_flow" selected/);
  assert.match(html, /data-qc-field="confirm" type="checkbox" checked/);
  const empty = renderVoiceSettings({});
  assert.match(empty, /value="default" selected/);
  assert.match(empty, /vqc-empty/);
});

test("serializes settings without inventing audio retention", () => {
  const shortcuts = [{ id: "voice", action: "voice_toggle", accelerator: "CommandOrControl+Shift+L", enabled: true }];
  const quickCommands = [{ id: "qc-1", phrase: "开工", action: "run_flow", argument: "晨间日报", enabled: true, confirm: true }];
  const values = serializeVoiceSettings({ wakeWord: { checked: true }, enabled: { checked: true }, deviceId: { value: "mic-1" }, sttProfile: { value: "cloud" }, quietMode: { checked: true }, retainAudio: { checked: false }, followUpEnabled: { checked: true }, followUpTimeoutSeconds: { value: "12" }, voicePack: { value: "lumo" }, confirmFlowRun: { checked: true }, ttsVoice: { value: " zh-CN " }, ttsRatePercent: { value: "65" } }, shortcuts, quickCommands);
  assert.deepEqual(values, { enabled: true, wakeWordEnabled: true, shortcut: "CommandOrControl+Shift+L", deviceId: "mic-1", sttProfile: "cloud", quietMode: true, retainAudio: false, followUpEnabled: true, followUpTimeoutSeconds: 12, voicePack: "lumo", quickCommands, confirmFlowRun: true, ttsVoice: "zh-CN", ttsRatePercent: 65, shortcuts });
  // 缺省回落：语音包 default、覆盖项为 null、指令为空数组
  const defaults = serializeVoiceSettings({}, []);
  assert.equal(defaults.voicePack, "default");
  assert.deepEqual(defaults.quickCommands, []);
  assert.equal(defaults.confirmFlowRun, false);
  assert.equal(defaults.ttsVoice, null);
  assert.equal(defaults.ttsRatePercent, null);
});

test("normalizes nested backend status and separately loaded models and devices", () => {
  const settings = normalizeVoiceSettings(
    { config: { enabled: true, wakeWordEnabled: true, shortcut: "Cmd+Shift+L", deviceId: "mic-1", sttProfile: "cloud", quietMode: true, retainAudio: false, followUpEnabled: true, followUpTimeoutSeconds: 8, voicePack: "lumo", quickCommands: [{ phrase: "开工", action: "run_flow", argument: "日报" }] } },
    [{ id: "sherpa-kws-en-v1", status: "installed" }],
    [{ id: "mic-1", name: "Studio Mic" }],
  );
  assert.equal(settings.shortcut, "Cmd+Shift+L");
  assert.equal(settings.deviceId, "mic-1");
  assert.equal(settings.models.length, 1);
  assert.equal(settings.devices[0].name, "Studio Mic");
  assert.equal(settings.shortcuts[0].action, "voice_toggle");
  assert.equal(settings.followUpTimeoutSeconds, 8);
  assert.equal(settings.voicePack, "lumo");
  assert.deepEqual(settings.quickCommands, [{ id: "qc-1", phrase: "开工", action: "run_flow", argument: "日报", enabled: true, confirm: false }]);
  // 未知语音包与非法动作回落安全默认
  const fallback = normalizeVoiceSettings({ voicePack: "alien", quickCommands: [{ phrase: "x", action: "reboot" }, { phrase: "  " }] });
  assert.equal(fallback.voicePack, "default");
  assert.deepEqual(fallback.quickCommands.map((command) => command.action), ["open_view"]);
});

test("collects quick command rows from editor DOM", () => {
  const row = (quickId, values) => ({
    dataset: { quickId },
    querySelector: (selector) => {
      const field = selector.match(/data-qc-field="(\w+)"/)?.[1];
      return field in values ? values[field] : null;
    },
  });
  const collected = collectQuickCommands([
    row("qc-1", { phrase: { value: " 开工 " }, action: { value: "run_flow" }, argument: { value: "晨间日报" }, enabled: { checked: true }, confirm: { checked: true } }),
    row("qc-2", { phrase: { value: "" }, action: { value: "stop" }, argument: { value: "" }, enabled: { checked: true }, confirm: { checked: false } }),
    row("qc-3", { phrase: { value: "闭麦" }, action: { value: "mute" }, argument: { value: "" }, enabled: { checked: false }, confirm: { checked: false } }),
  ]);
  assert.deepEqual(collected, [
    { id: "qc-1", phrase: "开工", action: "run_flow", argument: "晨间日报", enabled: true, confirm: true },
    { id: "qc-3", phrase: "闭麦", action: "mute", argument: "", enabled: false, confirm: false },
  ]);
  assert.deepEqual(normalizeQuickCommands([{ id: "qc-9", phrase: "开工", action: "run_flow", argument: "日报", enabled: true, confirm: true }]), [{ id: "qc-9", phrase: "开工", action: "run_flow", argument: "日报", enabled: true, confirm: true }]);
  assert.match(renderQuickCommandRow({ id: "qc-1", phrase: "开工", action: "mute" }), /value="mute" selected/);
});

test("exports and merges quick commands with imported phrases winning", () => {
  const existing = [
    { id: "qc-1", phrase: "开工", action: "run_flow", argument: "旧流程", enabled: true, confirm: false },
    { id: "qc-2", phrase: "收工", action: "stop", argument: "", enabled: true, confirm: false },
  ];
  const imported = normalizeQuickCommands(JSON.parse(exportQuickCommands([
    { id: "x-1", phrase: "开 工", action: "run_flow", argument: "新流程", enabled: true, confirm: true },
    { id: "x-2", phrase: "静音一下", action: "mute", argument: "", enabled: true, confirm: false },
  ])));
  const merged = mergeQuickCommands(existing, imported);
  assert.equal(merged.length, 3);
  const replaced = merged.find((command) => command.phrase.replaceAll(/\s+/g, "") === "开工");
  assert.equal(replaced.argument, "新流程");
  assert.equal(replaced.confirm, true);
  assert.ok(merged.some((command) => command.phrase === "收工"));
  assert.ok(merged.some((command) => command.phrase === "静音一下"));
});
