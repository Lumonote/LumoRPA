import test from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";
import { URL } from "node:url";

import { createVoiceProjection, applyVoiceEvent, renderVoiceCapsule } from "../src/js/voice-capsule.js";

test("renders every voice lifecycle state with accessible controls", () => {
  for (const status of ["idle", "listening", "transcribing", "routing", "confirming", "executing", "reporting", "error", "muted"]) {
    const html = renderVoiceCapsule({ ...createVoiceProjection(), status, partial: "打开日报", source: "LOCAL" });
    assert.match(html, new RegExp(`is-${status}`));
    assert.match(html, /aria-live=/);
    assert.match(html, /data-voice-action="cancel"/);
  }
});

test("projects transcript and agent events into capsule state", () => {
  let state = applyVoiceEvent(createVoiceProjection(), { kind: "voice.state", state: "listening" });
  state = applyVoiceEvent(state, { kind: "transcript.partial", text: "打开" });
  state = applyVoiceEvent(state, { kind: "transcript.final", text: "打开日报", source: "cloud" });
  state = applyVoiceEvent(state, { kind: "agent.state", state: "executing", progress: 0.4 });
  assert.deepEqual({ status: state.status, transcript: state.transcript, source: state.source, progress: state.progress }, { status: "executing", transcript: "打开日报", source: "CLOUD", progress: 0.4 });
});

test("capsule motion honors reduced motion", () => {
  const css = fs.readFileSync(new URL("../src/styles/voice-capsule.css", import.meta.url), "utf8");
  assert.match(css, /prefers-reduced-motion:\s*reduce/);
});

