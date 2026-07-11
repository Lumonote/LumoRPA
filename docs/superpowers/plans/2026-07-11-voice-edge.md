# Voice Edge Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add macOS shortcut and local-keyword wake, hybrid streaming STT, system TTS, privacy controls and the floating capsule that feeds transcripts into the Agent Harness.

**Architecture:** Keep audio/provider logic in `lumo-voice` and macOS/Tauri window concerns in the desktop host. A finite state machine owns transitions and cancellation. Only post-wake audio leaves the local ring buffer.

**Tech Stack:** Rust 2021, Tokio, cpal, sherpa-onnx, Tauri 2 global-shortcut, macOS AVSpeechSynthesizer, vanilla ESM/CSS.

---

### Task 1: Scaffold voice contracts and state machine

**Files:**
- Modify: `Cargo.toml`
- Create: `crates/lumo-voice/Cargo.toml`
- Create: `crates/lumo-voice/src/lib.rs`
- Create: `crates/lumo-voice/src/provider.rs`
- Create: `crates/lumo-voice/src/state.rs`
- Create: `crates/lumo-voice/tests/state_machine.rs`

- [ ] Test valid transitions, cancellation from every active state, duplicate wake suppression and return to idle after reporting.
- [ ] Implement `VoiceState`, `VoiceEvent`, `VoiceController` and provider traits `WakeWordProvider`, `SttProvider`, `TtsProvider`, `AudioCapture`.
- [ ] Run `cargo test -p lumo-voice` and commit `feat(voice): add provider contracts and state machine`.

### Task 2: Implement audio capture and privacy ring buffer

**Files:**
- Create: `crates/lumo-voice/src/audio.rs`
- Create: `crates/lumo-voice/tests/audio.rs`

- [ ] Test mono resampling, bounded pre-roll, zeroization on cancel and that no pre-wake frames reach the STT sink before wake.
- [ ] Implement a 16 kHz mono frame stream with a configurable two-second ring buffer and explicit `drain_after_wake()`.
- [ ] Run audio tests and commit `feat(voice): capture private bounded audio frames`.

### Task 3: Add sherpa-onnx wake word and local STT providers

**Files:**
- Modify: `crates/lumo-voice/Cargo.toml`
- Create: `crates/lumo-voice/src/sherpa.rs`
- Create: `crates/lumo-voice/tests/sherpa_provider.rs`
- Create: `crates/lumo-voice/tests/fixtures/wake-hit.wav`
- Create: `crates/lumo-voice/tests/fixtures/wake-miss.wav`

- [ ] Test deterministic wake hit/miss and partial/final transcript events with fixture audio.
- [ ] Implement model loading from an app-data resource directory, checksum verification, streaming KWS/ASR sessions and cancellation.
- [ ] Keep model files outside Git and the application binary; return a typed `ModelMissing` error with required asset IDs.
- [ ] Run provider tests on macOS and commit `feat(voice): add local wake word and speech recognition`.

### Task 4: Add cloud STT routing and system TTS

**Files:**
- Create: `crates/lumo-voice/src/stt_router.rs`
- Create: `crates/lumo-voice/src/macos_tts.rs`
- Create: `crates/lumo-voice/tests/stt_router.rs`

- [ ] Test local-first, user-selected cloud, privacy-denied fallback, provider timeout and cancellation.
- [ ] Implement cloud STT through existing Provider/Vault configuration and emit the same partial/final transcript contract.
- [ ] Implement AVSpeechSynthesizer start/stop, quiet mode and short-result length limits.
- [ ] Run tests and commit `feat(voice): route hybrid STT and macOS TTS`.

### Task 5: Host microphone permission and global shortcut in Tauri

**Files:**
- Modify: `apps/desktop/src-tauri/Cargo.toml`
- Modify: `apps/desktop/src-tauri/tauri.conf.json`
- Modify: `apps/desktop/src-tauri/capabilities/default.json`
- Create: `apps/desktop/src-tauri/src/voice_commands.rs`
- Modify: `apps/desktop/src-tauri/src/state.rs`
- Modify: `apps/desktop/src-tauri/src/lib.rs`
- Create: `apps/desktop/src-tauri/tests/voice_commands.rs`

- [ ] Test shortcut registration conflicts, permission-denied state, enable/disable wake listener, device selection and cancellation.
- [ ] Add `tauri-plugin-global-shortcut`, macOS microphone usage description and commands `voice_status`, `voice_configure`, `voice_start_listening`, `voice_stop`, `voice_devices`.
- [ ] Emit `lumo://voice-state` and `lumo://transcript`; final transcripts call `agent_start` through the same service boundary as typed prompts.
- [ ] Run desktop Rust tests and commit `feat(desktop): host voice wake and shortcut`.

### Task 6: Build the floating capsule

**Files:**
- Create: `apps/desktop/frontend/src/capsule.html`
- Create: `apps/desktop/frontend/src/js/voice-capsule.js`
- Create: `apps/desktop/frontend/src/styles/voice-capsule.css`
- Create: `apps/desktop/frontend/test/voice-capsule.test.js`
- Modify: `apps/desktop/src-tauri/src/lib.rs`

- [ ] Test rendering for listening, transcribing, routing, confirming, executing, reporting, error and muted states.
- [ ] Create a transparent always-on-top, non-activating capsule window; expand it only for confirmation or details and preserve keyboard accessibility when activated.
- [ ] Bind waveform, partial transcript, source badge, cancel and expand controls to voice/agent events.
- [ ] Run frontend tests/lint and commit `feat(desktop): add floating voice capsule`.

### Task 7: Verify privacy and latency targets

**Files:**
- Create: `crates/lumo-voice/benches/wake_idle.rs`
- Modify: `apps/desktop/README.md`

- [ ] Measure shortcut-to-capsule, event-to-UI and idle wake CPU on Apple Silicon release builds.
- [ ] Verify no raw audio file is created, cloud requests contain only post-wake frames, and mute/disable immediately stops capture.
- [ ] Record results against targets: shortcut UI <150ms, event UI <100ms, idle CPU <5%; commit `test(voice): verify privacy and latency budgets`.

### Task 8: Add voice settings and model resource management

**Files:**
- Create: `apps/desktop/src-tauri/src/voice_models.rs`
- Modify: `apps/desktop/src-tauri/src/voice_commands.rs`
- Modify: `apps/desktop/frontend/src/js/settings.js`
- Create: `apps/desktop/frontend/src/js/voice-settings.js`
- Create: `apps/desktop/frontend/test/voice-settings.test.js`

- [ ] Test wake-word enablement, shortcut conflict feedback, local/cloud STT selection, audio retention defaults, model checksum failure and download cancellation.
- [ ] Implement the model manifest contract:

```rust
pub struct VoiceModelManifest {
    pub id: String,
    pub kind: VoiceModelKind,
    pub url: String,
    pub sha256: String,
    pub unpacked_bytes: u64,
}

pub async fn install_voice_model(manifest: &VoiceModelManifest, target: &Path, cancel: CancellationToken) -> Result<(), VoiceModelError>;
```

Download into a temporary file, verify SHA-256, unpack into a versioned directory, then atomically update the active pointer. Delete partial files on cancellation or checksum failure.

- [ ] Add settings controls for wake word, shortcut, input device, STT profile, TTS voice, quiet mode, transcript retention and installed models.
- [ ] Run desktop/frontend tests and commit `feat(desktop): manage voice privacy and models`.
