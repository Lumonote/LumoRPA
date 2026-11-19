// Run: node --expose-gc apps/desktop/frontend/benchmarks/run-summaries.mjs
// Logic/V8 benchmark only. It excludes Rust/SQLite, IPC, DOM and WebView work.
import assert from "node:assert/strict";
import { Buffer } from "node:buffer";
import { performance } from "node:perf_hooks";
import process from "node:process";
import { setImmediate } from "node:timers/promises";
import {
  applyGraphRunResponse, createGraphRunContext, getGraphRunIndex, graphRuntimePath, recordGraphRunProgress,
} from "../src/js/editor/graph-run-state.js";
import { getRunInstanceIndex, runInstancePage, runResultIdentity } from "../src/js/run-instances.js";

const COUNTS = [10000, 100000];
const SAMPLES = 5;
const PAGE_SIZE = 200;
const FLOW = "/benchmark/run-summaries.yaml", SOURCE = "loop-summary-benchmark", RUN = "summary-benchmark";
const LEAVES = ["read_brief", "classify", "write_report"];
const AST = { spec: { steps: [{ id: "batch", action: "control.for_each", do: LEAVES.map((id) => ({ id, action: "control.log" })) }] } };
const initialContext = () => createGraphRunContext({ flowPath: FLOW, source: SOURCE, runId: RUN });
const editor = (context) => ({ flowPath: FLOW, source: SOURCE, ast: AST, editorRunContext: context });
const round = (value) => Math.round(value * 1000) / 1000;
const median = (values) => [...values].sort((a, b) => a - b)[Math.floor(values.length / 2)];
const collect = () => { globalThis.gc?.(); };
const heap = async () => {
  // Cross an event-loop boundary so V8 drops temporary stack/JSON references.
  await setImmediate(); collect(); await setImmediate(); collect();
  return process.memoryUsage().heapUsed;
};
const miB = (bytes) => round(bytes / 1024 / 1024);
let sink;

function measure(operation, setup = () => undefined) {
  sink = operation(setup());
  sink = null;
  const samplesMs = [];
  for (let sample = 0; sample < SAMPLES; sample++) {
    const prepared = setup();
    collect();
    const start = performance.now();
    sink = operation(prepared);
    samplesMs.push(performance.now() - start);
    sink = null;
  }
  return { medianMs: round(median(samplesMs)), samplesMs: samplesMs.map(round) };
}

// Exact camelCase StepRunDto summary shape: list_step_summaries selects NULL
// payloads/vars; step_summary_dto sets detailsDeferred=true. No result payload
// is generated here. The row count is step records, not loop iterations.
function makeSummaries(count) {
  return Array.from({ length: count }, (_, index) => {
    const iteration = Math.floor(index / LEAVES.length), leaf = LEAVES[index % LEAVES.length];
    const failed = index % 1000 === 999;
    return {
      detailsDeferred: true, hasOutput: false, seq: index + 1,
      path: `batch[${iteration}]/${leaf}`, parentPath: `batch[${iteration}]`, depth: 1,
      stepId: leaf, idx: index % LEAVES.length, state: failed ? "failed" : "ok", attempt: 1,
      outputJson: null, varsJson: null, error: failed ? "The requested record could not be processed." : null,
      startedAt: "2026-11-20T10:00:00+00:00", finishedAt: "2026-11-20T10:00:01+00:00", durationMs: 12 + index % 1989,
    };
  });
}

const responseFor = (steps) => ({ report: { runId: RUN, success: true }, run: { id: RUN, state: "ok" }, steps });
const identityOnly = (context) => runResultIdentity(context, RUN);

// Original runs.js instance picker algorithm, without markup/DOM. Its map
// allocates one wrapper for every summary, even though only 200 are displayed.
function scanInstancePage(steps, context, selectedIndex) {
  const path = graphRuntimePath(context, steps[selectedIndex].path, AST);
  const instances = steps.map((item, idx) => ({ item, idx }))
    .filter(({ item }) => graphRuntimePath(context, item.path, AST) === path);
  const position = Math.max(0, instances.findIndex(({ idx }) => idx === selectedIndex));
  const start = Math.floor(position / PAGE_SIZE) * PAGE_SIZE;
  return { total: instances.length, page: instances.slice(start, start + PAGE_SIZE) };
}

function indexedInstancePage(steps, index, context, selectedIndex) {
  const path = graphRuntimePath(context, steps[selectedIndex].path, AST);
  const page = runInstancePage(index.get(path) || [], selectedIndex, PAGE_SIZE);
  return { total: page.total, page: page.indices.map((idx) => ({ item: steps[idx], idx })) };
}

async function measureProgressRetention(steps, response) {
  const baseline = await heap();
  const anchors = {};
  const build = () => {
    let progress = initialContext();
    for (const step of steps) {
      progress = recordGraphRunProgress(progress, { runId: RUN, path: step.path, type: "step_started" });
      progress = recordGraphRunProgress(progress, { runId: RUN, path: step.path, type: "step_finished", state: step.state, durationMs: step.durationMs });
    }
    anchors.completed = applyGraphRunResponse(progress, response);
    anchors.activeResultContext = progress;
  };
  build();
  const withFullProgressIdentity = await heap();
  const release = () => { anchors.activeResultContext = identityOnly(anchors.activeResultContext); };
  release();
  const withIdentityOnly = await heap();
  assert.equal(Object.keys(anchors.completed.stepStates).length, steps.length);
  assert.equal(anchors.activeResultContext.runId, RUN);
  return {
    progressEvents: steps.length * 2,
    completedPlusRetainedProgressMiB: miB(withFullProgressIdentity - baseline),
    completedPlusIdentityOnlyMiB: miB(withIdentityOnly - baseline),
    releasedProgressChainMiB: miB(withFullProgressIdentity - withIdentityOnly),
  };
}

function benchmark(count) {
  const wire = JSON.stringify(makeSummaries(count));
  const jsonBytes = Buffer.byteLength(wire);
  const steps = JSON.parse(wire);
  const response = responseFor(steps), identity = identityOnly(initialContext());
  const context = applyGraphRunResponse(initialContext(), response);
  const runIndex = getGraphRunIndex(editor(context));
  const instanceIndex = getRunInstanceIndex(steps, identity, AST);
  assert.equal(runIndex.size, LEAVES.length);
  assert.equal([...runIndex.values()].reduce((sum, value) => sum + value.values.size, 0), count);
  const selectedIndex = count - 1;
  assert.deepEqual(indexedInstancePage(steps, instanceIndex, identity, selectedIndex), scanInstancePage(steps, identity, selectedIndex));
  const serialization = measure(() => JSON.stringify(steps).length);
  const wireForParsing = JSON.stringify(steps);
  const parsing = measure(() => JSON.parse(wireForParsing).length);
  const completion = measure(() => applyGraphRunResponse(initialContext(), response));
  const coldGraphIndex = measure((prepared) => getGraphRunIndex(editor(prepared)), () => applyGraphRunResponse(initialContext(), response));
  const hotGraphIndex = measure(() => { for (let read = 0; read < 1000; read++) sink = getGraphRunIndex(editor(context)); });
  const oldInstancePage = measure(() => scanInstancePage(steps, identity, selectedIndex));
  // A fresh array gives the production WeakMap cache a cold summary owner;
  // setup allocation is outside the timed build.
  const coldInstanceIndex = measure((prepared) => getRunInstanceIndex(prepared, identity, AST), () => steps.slice());
  const hotInstanceIndex = measure(() => { for (let read = 0; read < 1000; read++) sink = getRunInstanceIndex(steps, identity, AST); });
  const indexedPage = measure(() => indexedInstancePage(steps, instanceIndex, identity, selectedIndex));
  const latestSelection = measure(() => steps.findLastIndex((step) => graphRuntimePath(identity, step.path, AST) === "batch/read_brief"));
  const missingSelection = measure(() => steps.findLastIndex((step) => graphRuntimePath(identity, step.path, AST) === "missing"));
  const indexedLatestSelection = measure(() => instanceIndex.get("batch/read_brief")?.at(-1));
  return {
    records: count, canonicalNodes: LEAVES.length, jsonBytes, jsonMiB: miB(jsonBytes), bytesPerRecord: round(jsonBytes / count),
    serialization, parsing, completion, coldGraphIndex, hotGraphIndex1000Reads: hotGraphIndex,
    oldInstancePage, coldInstanceIndex, hotInstanceIndex1000Reads: hotInstanceIndex, indexedInstancePage: indexedPage,
    latestSelectionScan: latestSelection, missingSelectionScan: missingSelection, indexedLatestSelection,
  };
}

async function measureMemory(count) {
  if (!globalThis.gc) return { retainedHeapMiB: null, progressRetention: null };
  const beforeSummaries = await heap(), anchors = {};
  const load = () => { anchors.steps = JSON.parse(JSON.stringify(makeSummaries(count))); };
  load();
  const afterSummaries = await heap();
  anchors.response = responseFor(anchors.steps);
  anchors.context = applyGraphRunResponse(initialContext(), anchors.response);
  const afterContext = await heap();
  anchors.runIndex = getGraphRunIndex(editor(anchors.context));
  const afterGraphIndex = await heap();
  anchors.instanceIndex = getRunInstanceIndex(anchors.steps, identityOnly(initialContext()), AST);
  const afterInstanceIndex = await heap();
  const progressRetention = await measureProgressRetention(anchors.steps, anchors.response);
  assert.equal(anchors.steps.length, count);
  assert.equal(anchors.runIndex.size, LEAVES.length);
  return {
    retainedHeapMiB: {
      parsedSummaries: miB(afterSummaries - beforeSummaries),
      completedContextExtra: miB(afterContext - afterSummaries),
      graphAggregateIndexExtra: miB(afterGraphIndex - afterContext),
      instanceNumberIndexExtra: miB(afterInstanceIndex - afterGraphIndex),
    }, progressRetention,
  };
}

const results = [];
for (const count of COUNTS) {
  const result = benchmark(count);
  Object.assign(result, await measureMemory(count));
  results.push(result);
}
assert.equal(sink, null);
globalThis.console.log(JSON.stringify({
  description: "Pure Node/V8 logic benchmark. No Rust/SQLite, IPC, DOM or WebView measurement. One warmup and five samples; medians reported.",
  environment: { node: process.version, arch: process.arch, platform: process.platform, gcExposed: typeof globalThis.gc === "function" },
  fixture: "Three child nodes under batch control.for_each; 0.1% failed rows; StepRunDto fields including timestamps; deferred outputJson/varsJson are null. Heap fixture is deserialized JSON. Progress paths share summary string references, so released-chain size is a conservative synthetic estimate.",
  results,
}, null, 2));
