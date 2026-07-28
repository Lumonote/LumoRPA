import test from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";
import { URL } from "node:url";

import { createVoiceProjection, applyVoiceEvent, renderVoiceCapsule } from "../src/js/voice-capsule.js";

test("renders every voice lifecycle state with accessible controls", () => {
  for (const status of ["idle", "listening", "transcribing", "routing", "confirming", "executing", "reporting", "success", "error", "muted"]) {
    const html = renderVoiceCapsule({ ...createVoiceProjection(), status, partial: "打开日报", source: "LOCAL" });
    assert.match(html, new RegExp(`is-${status}`));
    assert.match(html, /aria-live=/);
    assert.match(html, /data-voice-action="cancel"/);
    assert.match(html, /vc-spectrum/);
    assert.match(html, /vc-particles/);
  }
});

test("projects quick command feedback into success and error states", () => {
  let state = applyVoiceEvent(createVoiceProjection(), { kind: "transcript.final", text: "打开任务中心", source: "local" });
  state = applyVoiceEvent(state, { kind: "voice.feedback", ok: true, message: "搞定啦！已打开任务中心" });
  assert.equal(state.status, "success");
  assert.equal(state.message, "搞定啦！已打开任务中心");
  assert.equal(state.transcript, "");
  assert.equal(state.progress, 1);
  const failed = applyVoiceEvent(createVoiceProjection(), { kind: "voice.feedback", ok: false, message: "哎呀，没有找到流程" });
  assert.equal(failed.status, "error");
  assert.equal(failed.message, "哎呀，没有找到流程");
  const html = renderVoiceCapsule(state);
  assert.match(html, /is-success/);
  assert.match(html, /DONE/);
  assert.match(html, /搞定啦！已打开任务中心/);
});

test("follow-up window honors persona message with default fallback", () => {
  const persona = applyVoiceEvent(createVoiceProjection(), { kind: "voice.follow-up", timeoutSeconds: 8, message: "Lumo 还在听，继续吩咐～" });
  assert.equal(persona.status, "listening");
  assert.equal(persona.message, "Lumo 还在听，继续吩咐～");
  const fallback = applyVoiceEvent(createVoiceProjection(), { kind: "voice.follow-up", timeoutSeconds: 12, message: "" });
  assert.match(fallback.message, /12 秒/);
});

test("projects transcript and agent events into capsule state", () => {
  let state = applyVoiceEvent(createVoiceProjection(), { kind: "voice.state", state: "listening" });
  state = applyVoiceEvent(state, { kind: "transcript.partial", text: "打开" });
  state = applyVoiceEvent(state, { kind: "transcript.final", text: "打开日报", source: "cloud" });
  state = applyVoiceEvent(state, { kind: "agent.state", state: "executing", progress: 0.4 });
  state = applyVoiceEvent(state, { kind: "voice.level", level: 0.75 });
  state = applyVoiceEvent(state, { kind: "voice.follow-up", timeoutSeconds: 12 });
  assert.deepEqual({ status: state.status, transcript: state.transcript, source: state.source, progress: state.progress, level: state.level }, { status: "listening", transcript: "打开日报", source: "CLOUD", progress: 0.4, level: 0.75 });
  assert.match(state.message, /12 秒/);
});

test("capsule motion honors reduced motion", () => {
  const css = fs.readFileSync(new URL("../src/styles/voice-capsule.css", import.meta.url), "utf8");
  assert.match(css, /prefers-reduced-motion:\s*reduce/);
  assert.match(css, /vc-spectrum-spin/);
  assert.match(css, /vc-waveform/);
  assert.match(css, /width:\s*min\(296px/);
  assert.match(css, /--cyan:\s*#35f2ff/);
  assert.doesNotMatch(css, /--pink|--coral|--lime/);
});
