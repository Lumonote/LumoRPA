// Run from any directory: node apps/desktop/frontend/benchmarks/graph-performance.mjs
// Measures pure state projection only; it excludes DOM, WebView and IPC work.
import assert from "node:assert/strict";
import { performance } from "node:perf_hooks";
import {
  createGraphRunContext, getGraphRunIndex, getGraphRunState, recordGraphRunProgress,
} from "../src/js/editor/graph-run-state.js";

const FLOW = "/benchmark.yaml";
const SOURCE = "benchmark-source";
const RUN = "benchmark-run";
const SAMPLES = 5;
const median = (values) => [...values].sort((a, b) => a - b)[Math.floor(values.length / 2)];
const initialContext = () => createGraphRunContext({ flowPath: FLOW, source: SOURCE, runId: RUN });
const editor = (context, ast) => ({ flowPath: FLOW, source: SOURCE, ast, editorRunContext: context });
const finished = (path) => ({ runId: RUN, path, type: "step_finished", state: "ok", durationMs: 1 });
const measure = (operation) => {
  operation(); // One independent warmup; each measured operation uses a cold context.
  return Array.from({ length: SAMPLES }, () => operation());
};

const coldContexts = [80, 300, 1000].map((count) => {
  const ast = { spec: { steps: Array.from({ length: count }, (_, index) => ({ id: `step${index}` })) } };
  const nodes = ast.spec.steps.map((step) => ({ runtimePath: step.id }));
  const samplesMs = measure(() => {
    let context = initialContext();
    for (const node of nodes) context = recordGraphRunProgress(context, finished(node.runtimePath));
    const state = editor(context, ast);
    const start = performance.now();
    const index = getGraphRunIndex(state);
    let complete = 0;
    for (const node of nodes) if (getGraphRunState(node, state, index).state === "ok") complete++;
    const elapsed = performance.now() - start;
    assert.equal(complete, count);
    return elapsed;
  });
  return { nodes: count, medianMs: median(samplesMs), samplesMs };
});

const iterations = 10000;
const loopAst = { spec: { steps: [{ id: "loop", action: "control.for_each", do: [{ id: "read" }] }] } };
const incrementalSamplesMs = measure(() => {
  let context = initialContext();
  let index;
  const start = performance.now();
  for (let item = 0; item < iterations; item++) {
    context = recordGraphRunProgress(context, finished(`loop[${item}]/read`));
    index = getGraphRunIndex(editor(context, loopAst));
  }
  const elapsed = performance.now() - start;
  assert.equal(index.get("loop/read").durationMs, iterations);
  assert.equal(index.get("loop/read").values.size, iterations);
  return elapsed;
});

globalThis.console.log(JSON.stringify({
  description: "Pure JavaScript projection; one warmup and five cold-context samples. No DOM/WebView/IPC.",
  coldContexts,
  incrementalLoop: { iterations, medianMs: median(incrementalSamplesMs), samplesMs: incrementalSamplesMs },
}, null, 2));
