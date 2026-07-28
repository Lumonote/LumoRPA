import test from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";
import { URL } from "node:url";
import { renderTopology, renderNodeDetail, renderRunHistoryOptions, renderRunMetrics, renderVoiceHistory } from "../src/js/mission-control.js";

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
