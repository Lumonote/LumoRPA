import test from "node:test";
import assert from "node:assert/strict";
import { setImmediate } from "node:timers";
import { applyCapsuleSnapshot, createVoiceProjection, mountVoiceCapsule, renderVoiceCapsule } from "../src/js/voice-capsule.js";

const snapshot = (revision = 0, extra = {}) => ({ revision, state: "idle", source: "text", message: "准备就绪", runState: "idle", activeRunId: null, ...extra });
const deferred = () => { let resolve; let reject; const promise = new Promise((yes, no) => { resolve = yes; reject = no; }); return { resolve, reject, promise }; };
function eventTarget() {
  const listeners = new Map();
  return { listeners,
    addEventListener(name, handler) { const group = listeners.get(name) || new Set(); group.add(handler); listeners.set(name, group); },
    removeEventListener(name, handler) { listeners.get(name)?.delete(handler); },
    async fire(name, event = {}) { for (const handler of listeners.get(name) || []) await handler(event); },
  };
}
function fixture() {
  const nodes = new Map();
  const focusState = { active: null };
  const node = () => ({ disabled: false, hidden: false, value: "", textContent: "", dataset: {}, attributes: {}, focused: false,
    style: { values: {}, setProperty(name, value) { this.values[name] = value; } },
    classList: { values: new Set(), toggle(name, value) { if (value) this.values.add(name); else this.values.delete(name); } },
    setAttribute(name, value) { this.attributes[name] = value; },
    focus() { if (this.disabled || this.hidden) return; if (focusState.active) focusState.active.focused = false; focusState.active = this; this.focused = true; },
    closest(selector) { return selector === "[data-voice-action]" && this.dataset.voiceAction ? this : selector === "[data-capsule-form]" && this.isForm ? this : null; },
  });
  for (const name of ["input", "form", "label", "source", "summary", "task-state", "task-label", "message", "result", "error", "progress", "progress-fill", "panel", "submit"]) nodes.set(`[data-capsule-${name}]`, node());
  for (const selector of [".voice-capsule", "[data-companion]", "[data-voice-meter]"]) nodes.set(selector, node());
  nodes.get("[data-capsule-form]").isForm = true;
  for (const action of ["panel", "cancel", "listen", "mute", "stop-task", "expand"]) {
    const control = node(); control.dataset.voiceAction = action; nodes.set(`[data-voice-action="${action}"]`, control);
  }
  const root = eventTarget(); root.writes = 0;
  Object.defineProperty(root, "innerHTML", { set(value) { this.markup = value; this.writes++; } });
  root.querySelector = (selector) => nodes.get(selector) || null;
  const events = new Map(); const calls = []; const removed = [];
  const options = {
    call: async (command, args) => { calls.push({ command, args }); return snapshot(); },
    listen: async (name, handler) => { events.set(name, handler); return () => { removed.push(name); events.delete(name); }; },
    hostWindow: eventTarget(), visibilityTarget: { ...eventTarget(), hidden: false },
  };
  const input = nodes.get("[data-capsule-input]");
  const edit = async (value) => { input.value = value; await root.fire("input", { target: input }); };
  const click = (action) => root.fire("click", { target: nodes.get(`[data-voice-action="${action}"]`) });
  const submit = () => root.fire("submit", { target: nodes.get("[data-capsule-form]"), preventDefault() {} });
  return { root, nodes, events, calls, removed, options, input, edit, click, submit, focusState };
}

test("host snapshots restore related task and reject late or malformed revisions", () => {
  let state = applyCapsuleSnapshot(createVoiceProjection(), snapshot(9, { activeRunId: "voice-run", runState: "waiting", progress: 0.4, muted: true, seq: 6 }));
  assert.equal(applyCapsuleSnapshot(state, snapshot(10, { runState: "waitingApproval" })).status, "confirming");
  assert.equal(state.status, "confirming"); assert.equal(state.activeRunId, "voice-run"); assert.equal(state.progress, 0.4);
  assert.equal(applyCapsuleSnapshot(state, snapshot(8, { runState: "completed" })), state);
  assert.equal(applyCapsuleSnapshot(state, snapshot(Number.NaN)), state);
  state = applyCapsuleSnapshot(state, snapshot(10, { runState: "completed", result: "日报已生成", progress: 2 }));
  assert.equal(state.status, "success"); assert.equal(state.result, "日报已生成"); assert.equal(state.progress, 1); assert.equal(state.activeRunId, null);
  assert.equal(applyCapsuleSnapshot(state, snapshot(11, { progress: Number.POSITIVE_INFINITY })).progress, 0);
});

test("snapshot restoration subscribes first and does not overwrite a newer live task", async () => {
  const view = fixture(); const initial = deferred();
  view.options.call = async (command) => { assert.equal(command, "capsule_status"); return initial.promise; };
  const mounting = mountVoiceCapsule(view.root, view.options);
  await new Promise((resolve) => setImmediate(resolve));
  assert.deepEqual([...view.events.keys()], ["lumo://capsule-state", "lumo://voice-level"]);
  assert.equal(view.events.has("lumo://agent-event"), false);
  view.events.get("lumo://capsule-state")({ payload: snapshot(4, { activeRunId: "new", runState: "running", taskLabel: "生成月报" }) });
  initial.resolve(snapshot(2, { activeRunId: "old", runState: "completed" }));
  const controller = await mounting;
  assert.equal(controller.getProjection().activeRunId, "new"); assert.equal(view.nodes.get("[data-capsule-task-label]").textContent, "生成月报"); controller.dispose();
});

test("live state and batched audio preserve input node, focus, draft, and safe result text", async () => {
  const view = fixture(); let nextFrame = null; let schedules = 0;
  view.options.requestFrame = (callback) => { nextFrame = callback; schedules++; return schedules; }; view.options.cancelFrame = () => {};
  const controller = await mountVoiceCapsule(view.root, view.options);
  await view.edit("运行日报"); view.input.focus(); const initialInput = view.input;
  view.events.get("lumo://capsule-state")({ payload: snapshot(1, { state: "listening", listening: true }) });
  for (let index = 0; index < 100; index++) view.events.get("lumo://voice-level")({ payload: { level: index / 100 } });
  assert.equal(schedules, 1); assert.equal(view.root.writes, 1); nextFrame();
  assert.equal(view.nodes.get("[data-voice-meter]").style.values["--voice-level"], "0.99");
  view.events.get("lumo://capsule-state")({ payload: snapshot(2, { runState: "completed", result: '<img src=x onerror="bad">', transcript: "旧指令" }) });
  assert.equal(view.input, initialInput); assert.equal(view.input.focused, true); assert.equal(view.input.value, "运行日报");
  assert.equal(view.nodes.get("[data-capsule-result]").textContent, '<img src=x onerror="bad">'); assert.equal(view.nodes.get("[data-capsule-summary]").textContent, "准备就绪"); controller.dispose();
});

test("task output stays complete in its scroll region while the composer keeps stable nodes", async () => {
  const view = fixture();
  const controller = await mountVoiceCapsule(view.root, view.options);
  await view.edit("下一份日报"); view.input.focus();
  const result = "完整日报结果 <字段> & 校验成功。\n".repeat(100);
  view.events.get("lumo://capsule-state")({ payload: snapshot(1, { expanded: true, taskLabel: "生成今天的日报", runState: "completed", state: "reporting", message: "日报已生成", result }) });
  const html = renderVoiceCapsule(controller.getProjection(), { expanded: true });
  assert.match(html, /data-capsule-content role="region" aria-label="任务进度与结果" tabindex="0"/);
  assert.ok(html.indexOf("</pre></div>") < html.indexOf('class="vc-task-actions"'));
  assert.ok(html.indexOf('class="vc-task-actions"') < html.indexOf("<form data-capsule-form>"));
  assert.match(html, /data-capsule-message aria-live="polite" hidden/);
  assert.equal(view.nodes.get("[data-capsule-result]").textContent, result);
  assert.equal(view.nodes.get("[data-capsule-message]").hidden, true);
  assert.equal(view.input.value, "下一份日报");
  assert.equal(view.focusState.active, view.input);
  view.events.get("lumo://capsule-state")({ payload: snapshot(2, { runState: "failed", state: "error", message: "任务失败", error: "服务不可用，请检查配置后重试" }) });
  assert.equal(view.nodes.get("[data-capsule-result]").hidden, false);
  assert.equal(view.nodes.get("[data-capsule-result]").textContent, "服务不可用，请检查配置后重试");
  assert.equal(view.nodes.get("[data-capsule-message]").hidden, true);
  view.events.get("lumo://capsule-state")({ payload: snapshot(3, { runState: "running", state: "executing", message: "正在重新执行" }) });
  assert.equal(view.nodes.get("[data-capsule-message]").hidden, false);
  assert.equal(view.nodes.get("[data-capsule-message]").textContent, "正在重新执行");
  assert.equal(view.root.writes, 1);
  controller.dispose();
});

test("text submit never opens microphone, prevents duplicates and preserves newer edits", async () => {
  const view = fixture(); const completion = deferred();
  view.options.call = async (command, args) => { view.calls.push({ command, args }); return command === "capsule_submit_text" ? completion.promise : snapshot(); };
  const controller = await mountVoiceCapsule(view.root, view.options);
  await view.edit("  运行日报  "); const sending = view.submit(); await view.submit(); await view.edit("接下来打开月报");
  completion.resolve(snapshot(1, { activeRunId: "task", runState: "running" })); await sending;
  assert.equal(view.calls.filter((call) => call.command === "capsule_submit_text").length, 1);
  assert.deepEqual(view.calls.find((call) => call.command === "capsule_submit_text").args, { text: "运行日报" });
  assert.equal(view.calls.some((call) => call.command === "voice_start_listening"), false); assert.equal(view.input.value, "接下来打开月报"); assert.equal(view.nodes.get("[data-capsule-submit]").disabled, true); controller.dispose();
});

test("failed submissions retain draft, show an error, and allow retry", async () => {
  const view = fixture(); let attempts = 0;
  view.options.call = async (command) => { if (command === "capsule_submit_text" && attempts++ === 0) throw new Error("model unavailable"); return snapshot(attempts); };
  const controller = await mountVoiceCapsule(view.root, view.options);
  await view.edit("运行日报"); await view.submit();
  assert.equal(view.input.value, "运行日报"); assert.match(view.nodes.get("[data-capsule-error]").textContent, /model unavailable/); assert.match(view.nodes.get("[data-capsule-summary]").textContent, /发送失败/); assert.equal(view.nodes.get("[data-capsule-submit]").disabled, false);
  await view.submit(); assert.equal(view.input.value, ""); controller.dispose();
});

test("a completed text task permits the next command while preserving genuine voice input", async () => {
  const view = fixture();
  view.options.call = async (command, args) => {
    view.calls.push({ command, args });
    return command === "capsule_submit_text"
      ? snapshot(2, { activeRunId: "next-run", runState: "running", state: "executing" })
      : snapshot(1, { activeRunId: "finished-run", runState: "completed", state: "reporting", progress: 1 });
  };
  const controller = await mountVoiceCapsule(view.root, view.options);
  await view.edit("运行下一份日报");
  assert.equal(controller.getProjection().status, "success");
  assert.equal(view.nodes.get("[data-capsule-submit]").disabled, false);
  assert.equal(view.nodes.get('[data-voice-action="listen"]').disabled, false);
  await view.submit();
  assert.deepEqual(view.calls.find((call) => call.command === "capsule_submit_text").args, { text: "运行下一份日报" });
  assert.equal(controller.getProjection().activeRunId, "next-run");
  await view.edit("等待语音完成");
  view.events.get("lumo://capsule-state")({ payload: snapshot(3, { activeRunId: "finished-run", runState: "completed", state: "listening", listening: true }) });
  assert.equal(controller.getProjection().status, "listening");
  assert.equal(view.nodes.get("[data-capsule-submit]").disabled, true);
  view.events.get("lumo://capsule-state")({ payload: snapshot(4, { activeRunId: "finished-run", runState: "failed", state: "executing" }) });
  assert.equal(controller.getProjection().status, "error");
  assert.equal(view.nodes.get("[data-capsule-submit]").disabled, false);
  controller.dispose();
});

test("controls bind displayed run, catch stale errors, and expand after successful resize", async () => {
  const view = fixture(); const cancelling = deferred(); const resizing = deferred();
  view.options.call = async (command, args) => { view.calls.push({ command, args }); if (command === "capsule_control") return cancelling.promise; if (command === "capsule_resize") return resizing.promise; return snapshot(1, { activeRunId: "displayed", runState: "running" }); };
  const controller = await mountVoiceCapsule(view.root, view.options);
  const stopping = view.click("stop-task"); view.events.get("lumo://capsule-state")({ payload: snapshot(2, { activeRunId: "replacement", runState: "running" }) });
  cancelling.reject(new Error("stale run")); await stopping;
  assert.deepEqual(view.calls.find((call) => call.command === "capsule_control").args, { action: "cancel", runId: "displayed" }); assert.match(view.nodes.get("[data-capsule-error]").textContent, /stale run/);
  const expanding = view.click("panel"); assert.equal(controller.isExpanded(), false); resizing.resolve(); await expanding;
  assert.equal(controller.isExpanded(), true); assert.equal(view.input.focused, true); assert.equal(view.nodes.get("[data-capsule-panel]").hidden, false); controller.dispose();
});

test("panel keyboard focus moves after resize controls are reenabled and survives failed or disposed actions", async () => {
  const view = fixture();
  const resizing = [];
  view.options.call = async (command) => {
    if (command !== "capsule_resize") return snapshot();
    const request = deferred(); resizing.push(request); return request.promise;
  };
  const controller = await mountVoiceCapsule(view.root, view.options);
  const toggle = view.nodes.get('[data-voice-action="panel"]');
  toggle.focus();
  const opening = view.click("panel");
  assert.equal(toggle.disabled, true);
  resizing[0].resolve(); await opening;
  assert.equal(controller.isExpanded(), true);
  assert.equal(toggle.disabled, false);
  assert.equal(view.focusState.active, view.input);
  let prevented = false;
  const closing = view.root.fire("keydown", { target: view.input, key: "Escape", preventDefault() { prevented = true; } });
  assert.equal(toggle.disabled, true);
  resizing[1].resolve(); await closing;
  assert.equal(prevented, true);
  assert.equal(controller.isExpanded(), false);
  assert.equal(toggle.disabled, false);
  assert.equal(view.focusState.active, toggle);
  const failed = view.click("panel");
  const mute = view.nodes.get('[data-voice-action="mute"]'); mute.focus();
  resizing[2].reject(new Error("window resize unavailable")); await failed;
  assert.equal(controller.isExpanded(), false);
  assert.equal(view.focusState.active, mute);
  const disposed = view.click("panel");
  controller.dispose();
  resizing[3].resolve(); await disposed;
  assert.equal(view.focusState.active, mute);
  assert.equal(view.input.focused, false);
});

test("Escape during IME composition leaves the draft and panel open until ordinary Escape", async () => {
  const view = fixture();
  view.options.call = async (command, args) => {
    view.calls.push({ command, args });
    return snapshot(0, { expanded: true });
  };
  const controller = await mountVoiceCapsule(view.root, view.options);
  await view.edit("正在组词"); view.input.focus();
  let composingPrevented = false;
  await view.root.fire("keydown", { target: view.input, key: "Escape", isComposing: true, preventDefault() { composingPrevented = true; } });
  assert.equal(composingPrevented, false);
  assert.equal(view.calls.some(({ command }) => command === "capsule_resize"), false);
  assert.equal(controller.isExpanded(), true);
  assert.equal(view.nodes.get("[data-capsule-panel]").hidden, false);
  assert.equal(view.input.value, "正在组词");
  assert.equal(view.focusState.active, view.input);
  let ordinaryPrevented = false;
  await view.root.fire("keydown", { target: view.input, key: "Escape", isComposing: false, preventDefault() { ordinaryPrevented = true; } });
  assert.equal(ordinaryPrevented, true);
  assert.deepEqual(view.calls.filter(({ command }) => command === "capsule_resize"), [{ command: "capsule_resize", args: { expanded: false } }]);
  assert.equal(controller.isExpanded(), false);
  assert.equal(view.focusState.active, view.nodes.get('[data-voice-action="panel"]'));
  controller.dispose();
});

test("remount and disposal release subscriptions including a listener still installing", async () => {
  const view = fixture(); const first = await mountVoiceCapsule(view.root, view.options); const second = await mountVoiceCapsule(view.root, view.options);
  assert.equal(view.removed.length, 2); assert.equal(view.root.listeners.get("click").size, 1); first.dispose(); second.dispose(); assert.equal(view.root.listeners.get("click").size, 0); assert.equal(view.removed.length, 4);
  const installing = deferred(); let released = false; view.options.listen = () => installing.promise;
  const pending = mountVoiceCapsule(view.root, view.options); await view.options.hostWindow.fire("pagehide"); installing.resolve(() => { released = true; });
  const disposed = await pending; assert.equal(released, true); disposed.dispose();
});

test("microphone mute still permits text, panel expansion restores, and initial output escapes markup", async () => {
  const view = fixture(); view.options.call = async () => snapshot(3, { muted: true, blocked: true, expanded: true, result: "<script>unsafe</script>" });
  const controller = await mountVoiceCapsule(view.root, view.options); await view.edit("文字任务");
  assert.equal(view.nodes.get('[data-voice-action="listen"]').disabled, true); assert.equal(view.nodes.get("[data-capsule-submit]").disabled, false); assert.equal(controller.isExpanded(), true);
  const html = renderVoiceCapsule(controller.getProjection(), { expanded: true }); assert.doesNotMatch(html, /<script>/); assert.match(html, /&lt;script&gt;/); controller.dispose();
});
