import test from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";
import { URL } from "node:url";
import { mountMissionControl, receiveMissionControlAgentEvent, selectMissionControlRun, updateMissionControl, renderTopology, renderNodeDetail, renderRunHistoryOptions, renderRunMetrics, renderVoiceHistory } from "../src/js/mission-control.js";
import { bindCapsuleRunNavigation, restoreAndSubscribeAgentEvents } from "../src/js/app-events.js";
import { createRunProjection, applyAgentEvent } from "../src/js/agent-events.js";

const projection = {
  status: "running",
  activeNodeId: "n1",
  startedAt: 10,
  nodes: { n1: { id: "n1", capabilityId: "mcp:erp/query", status: "running", payload: { token: "secret", progress: 0.5 } } },
  edges: [], events: [], replans: [], seq: 2,
};

test("renders topology and detail without leaking secret-like fields", () => {
  const topology = { lanes: [[projection.nodes.n1]], edges: [], activeNodeId: "n1" };
  assert.match(renderTopology(topology), /mcp:erp\/query/);
  const detail = renderNodeDetail(projection.nodes.n1);
  assert.doesNotMatch(detail, /secret/);
  assert.match(detail, /••••••••/);
});

test("escapes untrusted labels and renders metrics", () => {
  const node = { id: "<img onerror=1>", capabilityId: "flow:<bad>", status: "failed", payload: {} };
  const html = renderTopology({ lanes: [[node]], edges: [], activeNodeId: null });
  assert.doesNotMatch(html, /<img/);
  assert.match(html, /&lt;img/);
  assert.match(renderRunMetrics(projection), /RUNNING/);
});

test("mission control CSS honors reduced motion", () => {
  const css = fs.readFileSync(new URL("../src/styles/mission-control.css", import.meta.url), "utf8");
  assert.match(css, /prefers-reduced-motion:\s*reduce/);
});

test("renders selectable Agent run history", () => {
  const html = renderRunHistoryOptions([{ id: "r1", utterance: "整理日报", state: "completed" }], "r1");
  assert.match(html, /整理日报/);
  assert.match(html, /completed/);
  assert.match(html, /selected/);
});

test("renders recent voice command history with ok/error states", () => {
  assert.match(renderVoiceHistory([]), /还没有语音指令记录/);
  const html = renderVoiceHistory([
    { atMs: 1753400000000, utterance: "打开任务中心", intent: "打开任务中心", ok: true, message: "搞定啦！已打开任务中心" },
    { atMs: 1753400001000, utterance: "运行月报", intent: "运行流程「月报」", ok: false, message: "没有找到名为「月报」的流程" },
  ]);
  assert.match(html, /mc-voice-list/);
  assert.match(html, /is-ok/);
  assert.match(html, /is-bad/);
  assert.match(html, /打开任务中心/);
  assert.match(html, /没有找到名为「月报」的流程/);
});

test("clicking a node after a live update uses the current projection", (context) => {
  const previousDocument = globalThis.document;
  context.after(() => { globalThis.document = previousDocument; });
  const handlers = {};
  const boxes = Object.fromEntries(["[data-mc-topology]", "[data-mc-detail]", "[data-mc-metrics]", "[data-mc-log]"].map((name) => [name, { innerHTML: "" }]));
  const root = {
    dataset: {}, classList: { toggle() {} },
    querySelector: (name) => boxes[name] || null,
    addEventListener: (name, handler) => { handlers[name] = handler; },
  };
  globalThis.document = { createElement: () => ({ dataset: {}, setAttribute() {}, addEventListener() {} }) };
  mountMissionControl(root, { ...projection, runId: "r1" });
  const completed = { ...projection, runId: "r1", seq: 3, status: "completed", nodes: { n1: { ...projection.nodes.n1, status: "completed", payload: { result: "fresh result" } } } };
  updateMissionControl(root, completed);
  handlers.click({ target: { closest: (selector) => selector === "[data-node-id]" ? { dataset: { nodeId: "n1" } } : null } });
  assert.match(boxes["[data-mc-detail]"].innerHTML, /fresh result/);
  assert.match(boxes["[data-mc-metrics]"].innerHTML, /COMPLETED/);
  assert.doesNotMatch(boxes["[data-mc-topology]"].innerHTML, /is-running/);
});

function missionRoot(context) {
  const previousDocument = globalThis.document;
  context.after(() => { globalThis.document = previousDocument; });
  const handlers = {};
  const boxes = Object.fromEntries(["[data-mc-topology]", "[data-mc-detail]", "[data-mc-metrics]", "[data-mc-log]", "[data-mc-control-error]"].map((name) => [name, { innerHTML: "", textContent: "" }]));
  const root = {
    dataset: {}, classList: { toggle() {} },
    querySelector: (name) => boxes[name] || null,
    addEventListener: (name, handler) => { handlers[name] = handler; },
  };
  const elements = [];
  boxes[".mc-head"] = { append(element) {
    elements.push(element);
    if ("mcHistoryError" in element.dataset) boxes["[data-mc-history-error]"] = element;
  } };
  globalThis.document = { createElement: (tagName) => {
    const element = { tagName, dataset: {}, value: "", innerHTML: "", textContent: "", handlers: {}, attributes: {},
      setAttribute(name, value) { this.attributes[name] = value; },
      addEventListener(name, handler) { this.handlers[name] = handler; },
    };
    return element;
  } };
  return { root, boxes, handlers, elements, select: (value) => {
    const selector = elements.find((element) => element.tagName === "select");
    selector.value = value;
    return selector.handlers.change();
  } };
}

function deferred() {
  let resolve, reject;
  const promise = new Promise((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
}

const flush = async () => { await new Promise((resolve) => globalThis.setImmediate(resolve)); };
const completedEvents = (label, seq = 2) => [
  { seq: 1, kind: "node.completed", node_id: "n1", payload: { result: label } },
  { seq, kind: "run.completed" },
];

test("history keeps an explicit follow-latest option even when no runs exist", () => {
  assert.match(renderRunHistoryOptions([]), /<option value="" selected>.*跟随最新/);
  assert.match(renderRunHistoryOptions([{ id: "old", state: "completed" }], "old"), /<option value="">.*跟随最新/);
});

test("programmatic selection opens an exact run missing from recent history", async (context) => {
  const ui = missionRoot(context);
  const requests = [];
  mountMissionControl(ui.root, { ...projection, runId: "live" }, async (command, args) => {
    if (command !== "agent_events") return [];
    requests.push(args);
    return completedEvents("requested result", 6);
  });
  await selectMissionControlRun(ui.root, "older-run");
  const selector = ui.elements.find((element) => element.tagName === "select");
  assert.deepEqual(requests, [{ runId: "older-run", afterSeq: 0 }]);
  assert.equal(ui.root.dataset.selectedRun, "older-run");
  assert.equal(selector.value, "older-run");
  assert.match(selector.innerHTML, /value="older-run" selected/);
  assert.match(ui.boxes["[data-mc-metrics]"].innerHTML, /COMPLETED/);
  assert.match(ui.boxes["[data-mc-metrics]"].innerHTML, /seq 6/);
  assert.equal(selector.attributes["aria-busy"], "false");
});

test("rapid capsule navigation keeps the last requested run when responses arrive late", async (context) => {
  const ui = missionRoot(context);
  const requests = new Map();
  mountMissionControl(ui.root, { ...projection, runId: "live" }, async (command, args) => {
    if (command !== "agent_events") return [];
    const request = deferred(); requests.set(args.runId, request); return request.promise;
  });
  let receive;
  const errors = [];
  await bindCapsuleRunNavigation({
    listen: async (_name, handler) => { receive = handler; return () => {}; },
    openRun: (runId) => selectMissionControlRun(ui.root, runId),
    onError: (error) => errors.push(error),
  });
  const loadingA = receive({ payload: { runId: "A" } });
  const loadingB = receive({ payload: { runId: "B" } });
  const selector = ui.elements.find((element) => element.tagName === "select");
  await flush();
  requests.get("A").reject(new Error("stale request failed"));
  await loadingA;
  assert.equal(selector.attributes["aria-busy"], "true");
  assert.equal(ui.boxes["[data-mc-history-error]"].textContent, "");
  requests.get("B").resolve(completedEvents("B result", 9));
  await loadingB;
  assert.equal(ui.root.dataset.selectedRun, "B");
  assert.equal(selector.value, "B");
  assert.equal(selector.attributes["aria-busy"], "false");
  assert.match(ui.boxes["[data-mc-metrics]"].innerHTML, /seq 9/);
  assert.deepEqual(errors, []);
});

test("repeated programmatic selection of one run discards earlier snapshots", async (context) => {
  const ui = missionRoot(context);
  const requests = [];
  mountMissionControl(ui.root, { ...projection, runId: "live" }, async (command) => {
    if (command !== "agent_events") return [];
    const request = deferred(); requests.push(request); return request.promise;
  });
  const first = selectMissionControlRun(ui.root, "same-run");
  const second = selectMissionControlRun(ui.root, "same-run");
  await flush();
  requests[1].resolve(completedEvents("new snapshot", 12));
  await second;
  requests[0].resolve(completedEvents("old snapshot", 3));
  await first;
  assert.equal(ui.root.dataset.selectedRun, "same-run");
  assert.match(ui.boxes["[data-mc-metrics]"].innerHTML, /seq 12/);
  await selectMissionControlRun(ui.root, "");
  assert.match(ui.boxes["[data-mc-metrics]"].innerHTML, /RUNNING/);
  assert.match(ui.boxes["[data-mc-metrics]"].innerHTML, /seq 2/);
});

test("one native subscription keeps a selected companion run live after another run becomes latest", async (context) => {
  const ui = missionRoot(context);
  const events = {
    A: [
      { runId: "A", seq: 1, kind: "session.started" },
      { runId: "A", seq: 2, kind: "plan.created", payload: { nodes: [{ id: "report", capabilityId: "flow:report" }] } },
      { runId: "A", seq: 3, kind: "permission.requested", nodeId: "report", payload: { planHash: "hash", nodeIds: ["report"], risk: "L1" } },
    ],
    B: [{ runId: "B", seq: 1, kind: "run.started" }],
  };
  let latest = "A", receive, subscriptions = 0;
  const requests = [];
  const call = async (command, args) => {
    if (command !== "agent_events") return [];
    requests.push(args);
    return events[args.runId === "latest" ? latest : args.runId].filter((event) => event.seq > args.afterSeq);
  };
  mountMissionControl(ui.root, createRunProjection("A"), call);
  const stream = await restoreAndSubscribeAgentEvents({
    runId: "latest", call,
    listen: async (name, handler) => { subscriptions++; assert.equal(name, "lumo://agent-event"); receive = handler; return () => {}; },
    schedule: (render) => render(),
    onProjection: (next) => updateMissionControl(ui.root, next),
    onEvent: (event) => { void receiveMissionControlAgentEvent(ui.root, event); },
  });
  context.after(() => stream.unlisten());
  await selectMissionControlRun(ui.root, "A");
  assert.match(ui.boxes["[data-mc-detail]"].innerHTML, /data-agent-action="approve"/);
  latest = "B";
  await receive({ payload: events.B[0] });
  assert.equal(stream.getProjection().runId, "B");
  assert.equal(ui.root.dataset.selectedRun, "A");
  assert.match(ui.boxes["[data-mc-metrics]"].innerHTML, /WAITING/);
  events.A.push({ runId: "A", seq: 4, kind: "permission.resolved", payload: { planHash: "hash", nodeIds: ["report"], approved: true } });
  await receive({ payload: events.A[3] });
  await flush();
  assert.doesNotMatch(ui.boxes["[data-mc-detail]"].innerHTML, /mc-confirmation/);
  assert.match(ui.boxes["[data-mc-metrics]"].innerHTML, /RUNNING/);
  events.A.push({ runId: "A", seq: 5, kind: "node.completed", nodeId: "report", payload: { result: "日报已生成" } }, { runId: "A", seq: 6, kind: "run.completed" });
  await receive({ payload: events.A[5] });
  await flush();
  assert.ok(requests.some((args) => args.runId === "A" && args.afterSeq === 4));
  assert.match(ui.boxes["[data-mc-metrics]"].innerHTML, /COMPLETED/);
  assert.match(ui.boxes["[data-mc-metrics]"].innerHTML, /1\/1/);
  assert.match(ui.boxes["[data-mc-metrics]"].innerHTML, /seq 6/);
  await receive({ payload: { runId: "A", seq: 7, kind: "permission.requested", payload: { planHash: "late", nodeIds: ["report"] } } });
  await flush();
  assert.doesNotMatch(ui.boxes["[data-mc-detail]"].innerHTML, /data-agent-action/);
  assert.match(ui.boxes["[data-mc-metrics]"].innerHTML, /COMPLETED/);
  assert.equal(stream.getProjection().runId, "B");
  await selectMissionControlRun(ui.root, "");
  assert.match(ui.boxes["[data-mc-metrics]"].innerHTML, /RUNNING/);
  assert.match(ui.boxes["[data-mc-metrics]"].innerHTML, /seq 1/);
  assert.equal(subscriptions, 1);
});

test("selected-run restoration buffers live events and ignores late gap replies after switching", async (context) => {
  const ui = missionRoot(context);
  const initial = deferred(), gap = deferred();
  const requests = [];
  mountMissionControl(ui.root, { ...projection, runId: "live" }, async (command, args) => {
    if (command !== "agent_events") return [];
    requests.push(args);
    if (args.runId === "A") return args.afterSeq === 0 ? initial.promise : gap.promise;
    return [{ runId: "B", seq: 1, kind: "run.started" }];
  });
  const loading = selectMissionControlRun(ui.root, "A");
  await receiveMissionControlAgentEvent(ui.root, { runId: "A", seq: 2, kind: "run.paused" });
  initial.resolve([{ runId: "A", seq: 1, kind: "run.started" }]);
  await loading;
  assert.match(ui.boxes["[data-mc-metrics]"].innerHTML, /PAUSED/);
  const missing = receiveMissionControlAgentEvent(ui.root, { runId: "A", seq: 5, kind: "run.completed" });
  await flush();
  assert.deepEqual(requests.at(-1), { runId: "A", afterSeq: 2 });
  await selectMissionControlRun(ui.root, "B");
  gap.resolve([{ runId: "A", seq: 3, kind: "run.resumed" }, { runId: "A", seq: 4, kind: "node.completed", nodeId: "report" }, { runId: "A", seq: 5, kind: "run.completed" }]);
  await missing;
  await receiveMissionControlAgentEvent(ui.root, { runId: "A", seq: 6, kind: "run.failed" });
  assert.equal(ui.root.dataset.selectedRun, "B");
  assert.match(ui.boxes["[data-mc-metrics]"].innerHTML, /RUNNING/);
  assert.match(ui.boxes["[data-mc-metrics]"].innerHTML, /seq 1/);
});

test("selected-run gap errors are visible and a later event can recover in order", async (context) => {
  const ui = missionRoot(context);
  let unavailable = true;
  mountMissionControl(ui.root, { ...projection, runId: "live" }, async (command, args) => {
    if (command !== "agent_events") return [];
    if (args.afterSeq === 0) return [{ runId: "A", seq: 1, kind: "run.started" }];
    if (unavailable) throw new Error("sync unavailable");
    return [{ runId: "A", seq: 2, kind: "node.completed", nodeId: "report" }, { runId: "A", seq: 3, kind: "run.completed" }];
  });
  await selectMissionControlRun(ui.root, "A");
  await receiveMissionControlAgentEvent(ui.root, { runId: "A", seq: 3, kind: "run.completed" });
  assert.match(ui.boxes["[data-mc-history-error]"].textContent, /sync unavailable/);
  unavailable = false;
  await receiveMissionControlAgentEvent(ui.root, { runId: "A", seq: 3, kind: "run.completed" });
  assert.equal(ui.boxes["[data-mc-history-error]"].textContent, "");
  assert.match(ui.boxes["[data-mc-metrics]"].innerHTML, /COMPLETED/);
  assert.match(ui.boxes["[data-mc-metrics]"].innerHTML, /seq 3/);
});

test("a pinned history run retains live updates and returning to latest renders immediately", async (context) => {
  const ui = missionRoot(context);
  mountMissionControl(ui.root, { ...projection, runId: "live" }, async (command) => command === "agent_events" ? completedEvents("historical") : []);
  await ui.select("old");
  assert.match(ui.boxes["[data-mc-metrics]"].innerHTML, /COMPLETED/);
  ui.root.dataset.selectedNode = "old-node";
  ui.root.dataset.startLane = "8";
  updateMissionControl(ui.root, { ...projection, runId: "new-live", seq: 5, status: "failed" });
  assert.match(ui.boxes["[data-mc-metrics]"].innerHTML, /COMPLETED/);
  await ui.select("");
  assert.match(ui.boxes["[data-mc-metrics]"].innerHTML, /FAILED/);
  assert.match(ui.boxes["[data-mc-metrics]"].innerHTML, /seq 5/);
  assert.equal(ui.root.dataset.selectedNode, undefined);
  assert.equal(ui.root.dataset.startLane, undefined);
});

test("out-of-order history responses cannot replace a newer selection or follow-latest", async (context) => {
  const ui = missionRoot(context);
  const requests = new Map();
  mountMissionControl(ui.root, { ...projection, runId: "live" }, async (command, args) => {
    if (command !== "agent_events") return [];
    const request = deferred(); requests.set(args.runId, request); return request.promise;
  });
  const loadingA = ui.select("A");
  const loadingB = ui.select("B");
  await flush();
  requests.get("B").resolve(completedEvents("B result", 9));
  await loadingB;
  requests.get("A").resolve(completedEvents("A result", 3));
  await loadingA;
  assert.match(ui.boxes["[data-mc-metrics]"].innerHTML, /seq 9/);
  const loadingC = ui.select("C");
  await flush();
  updateMissionControl(ui.root, { ...projection, runId: "live", seq: 7, status: "failed" });
  await ui.select("");
  requests.get("C").resolve(completedEvents("C result", 20));
  await loadingC;
  assert.match(ui.boxes["[data-mc-metrics]"].innerHTML, /FAILED/);
  assert.match(ui.boxes["[data-mc-metrics]"].innerHTML, /seq 7/);
});

test("live progress cannot move backward while a selected live history request resolves", async (context) => {
  const ui = missionRoot(context);
  const request = deferred();
  mountMissionControl(ui.root, { ...projection, runId: "live" }, async (command) => command === "agent_events" ? request.promise : []);
  const loading = ui.select("live");
  updateMissionControl(ui.root, { ...projection, runId: "live", seq: 8, status: "completed" });
  request.resolve(completedEvents("older", 3));
  await loading;
  updateMissionControl(ui.root, { ...projection, runId: "live", seq: 4, status: "running" });
  assert.match(ui.boxes["[data-mc-metrics]"].innerHTML, /COMPLETED/);
  assert.match(ui.boxes["[data-mc-metrics]"].innerHTML, /seq 8/);
});

test("a fresher history snapshot of the latest run survives returning to live", async (context) => {
  const ui = missionRoot(context);
  mountMissionControl(ui.root, { ...projection, runId: "live" }, async (command) => command === "agent_events" ? completedEvents("fresh", 9) : []);
  await ui.select("live");
  updateMissionControl(ui.root, { ...projection, runId: "live", seq: 4 });
  await ui.select("");
  assert.match(ui.boxes["[data-mc-metrics]"].innerHTML, /COMPLETED/);
  assert.match(ui.boxes["[data-mc-metrics]"].innerHTML, /seq 9/);
});

test("a lagging live event switches latest to its fresher cached history run", async (context) => {
  const ui = missionRoot(context);
  mountMissionControl(ui.root, { ...projection, runId: "old-live" }, async (command) => command === "agent_events" ? completedEvents("newer", 9) : []);
  await ui.select("new-live");
  updateMissionControl(ui.root, { ...projection, runId: "new-live", seq: 4 });
  await ui.select("");
  assert.match(ui.boxes["[data-mc-metrics]"].innerHTML, /COMPLETED/);
  assert.match(ui.boxes["[data-mc-metrics]"].innerHTML, /seq 9/);
});

test("new runs and terminal transitions coalesce history refresh and preserve a missing pinned option", async (context) => {
  const ui = missionRoot(context);
  const lists = [];
  mountMissionControl(ui.root, { ...projection, runId: "live" }, async (command) => {
    if (command === "agent_events") return completedEvents("old result");
    if (command === "agent_run_list") { const request = deferred(); lists.push(request); return request.promise; }
    return [];
  });
  await ui.select("old");
  updateMissionControl(ui.root, { ...projection, runId: "new", seq: 1 });
  updateMissionControl(ui.root, { ...projection, runId: "new", seq: 2, status: "completed" });
  updateMissionControl(ui.root, { ...projection, runId: "new", seq: 3, status: "completed" });
  assert.equal(lists.length, 1);
  lists[0].resolve([{ id: "live", state: "running" }]);
  await flush();
  assert.equal(lists.length, 2);
  lists[1].resolve([{ id: "new", state: "completed" }]);
  await flush();
  const selector = ui.elements.find((element) => element.tagName === "select");
  assert.match(selector.innerHTML, /value="old" selected/);
  assert.match(selector.innerHTML, /value="new"/);
  assert.equal(selector.value, "old");
  assert.match(ui.boxes["[data-mc-metrics]"].innerHTML, /COMPLETED/);
});

test("history IPC failures are visible without preventing a return to live", async (context) => {
  const ui = missionRoot(context);
  mountMissionControl(ui.root, { ...projection, runId: "live" }, async (command) => {
    throw new Error(command === "agent_events" ? "history unavailable" : "list unavailable");
  });
  await flush();
  assert.match(ui.boxes["[data-mc-history-error]"].textContent, /list unavailable/);
  await ui.select("old");
  assert.match(ui.boxes["[data-mc-history-error]"].textContent, /history unavailable/);
  assert.doesNotMatch(ui.boxes["[data-mc-detail]"].innerHTML, /data-agent-action/);
  await ui.select("");
  assert.match(ui.boxes["[data-mc-metrics]"].innerHTML, /RUNNING/);
});

test("resolved permissions and terminal runs do not leave actionable confirmation cards", (context) => {
  const { root, boxes } = missionRoot(context);
  let p = createRunProjection("r1");
  p = applyAgentEvent(p, { seq: 1, kind: "permission.requested", node_id: "send", payload: { planHash: "hash", nodeIds: ["send"], capabilityId: "mcp:mail/send", risk: "L2", arguments: { body: "report" } } });
  updateMissionControl(root, p);
  assert.match(boxes["[data-mc-detail]"].innerHTML, /data-agent-action="approve"/);
  p = applyAgentEvent(p, { seq: 2, kind: "permission.resolved", payload: { planHash: "hash", nodeIds: ["send"], approved: true } });
  updateMissionControl(root, p);
  assert.doesNotMatch(boxes["[data-mc-detail]"].innerHTML, /mc-confirmation/);
  p = applyAgentEvent(p, { seq: 3, kind: "run.completed" });
  updateMissionControl(root, p);
  assert.doesNotMatch(boxes["[data-mc-detail]"].innerHTML, /data-agent-action/);
});

test("failed run controls restore the button and show the command error", async (context) => {
  const { root, boxes, handlers } = missionRoot(context);
  const control = { dataset: { agentAction: "pause", runId: "r1" }, disabled: false };
  mountMissionControl(root, { ...projection, runId: "r1" }, async (command) => {
    if (command === "agent_pause") throw new Error("connection unavailable");
    return [];
  });
  await handlers.click({ target: { closest: (selector) => selector === "[data-agent-action]" ? control : null } });
  assert.equal(control.disabled, false);
  assert.match(boxes["[data-mc-control-error]"].textContent, /connection unavailable/);
});

test("a command error remains visible when progress rerenders the detail during IPC", async (context) => {
  const { root, boxes, handlers } = missionRoot(context);
  let rejectCommand;
  const control = { dataset: { agentAction: "pause", runId: "r1" }, disabled: false };
  mountMissionControl(root, { ...projection, runId: "r1" }, async (command) => {
    if (command === "agent_pause") return new Promise((_resolve, reject) => { rejectCommand = reject; });
    return [];
  });
  const request = handlers.click({ target: { closest: (selector) => selector === "[data-agent-action]" ? control : null } });
  boxes["[data-mc-control-error]"] = { textContent: "" };
  rejectCommand(new Error("pause unavailable"));
  await request;
  assert.match(boxes["[data-mc-control-error]"].textContent, /pause unavailable/);
});
