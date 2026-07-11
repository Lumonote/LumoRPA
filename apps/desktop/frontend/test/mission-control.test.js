import test from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";
import { URL } from "node:url";
import { renderTopology, renderNodeDetail, renderRunMetrics } from "../src/js/mission-control.js";

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
