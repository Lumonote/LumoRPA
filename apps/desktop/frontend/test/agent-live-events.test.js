import test from "node:test";
import assert from "node:assert/strict";

import { restoreAndSubscribeAgentEvents } from "../src/js/app-events.js";

test("subscribes before restoring persisted events and fills sequence gaps", async () => {
  const order = [];
  const projections = [];
  let liveHandler;
  const call = async (_cmd, args) => {
    order.push(`load:${args.afterSeq}`);
    if (args.afterSeq === 0) return [{ seq: 1, kind: "run.started" }];
    return [{ seq: 2, kind: "node.started", nodeId: "n1" }];
  };
  const listen = async (_event, handler) => {
    order.push("listen");
    liveHandler = handler;
    return () => {};
  };

  await restoreAndSubscribeAgentEvents({ runId: "r1", call, listen, onProjection: (p) => projections.push(p), schedule: (fn) => fn() });
  await liveHandler({ payload: { seq: 3, kind: "node.completed", nodeId: "n1" } });
  await liveHandler({ payload: { seq: 5, kind: "run.completed" } });

  assert.deepEqual(order, ["listen", "load:0", "load:1", "load:3"]);
  assert.equal(projections.at(-1).seq, 5);
  assert.equal(projections.at(-1).status, "completed");
});

test("latest follows real run ids and restores the backend's nested plan payload", async () => {
  let liveHandler;
  let latest = "r1";
  const history = (id) => [
    { run_id: id, seq: 1, kind: "session.started" },
    { run_id: id, seq: 2, kind: "plan.created", payload: { plan: { nodes: [{ id: "read", capability_id: "file.read", depends_on: [] }] } } },
  ];
  const controller = await restoreAndSubscribeAgentEvents({
    runId: "latest",
    call: async () => history(latest),
    listen: async (_event, handler) => { liveHandler = handler; return () => {}; },
    onProjection: () => {}, schedule: (fn) => fn(),
  });
  assert.equal(controller.getProjection().runId, "r1");
  assert.equal(controller.getProjection().nodes.read.capabilityId, "file.read");
  await liveHandler({ payload: { runId: "r1", seq: 3, kind: "node.completed", nodeId: "read" } });
  assert.equal(controller.getProjection().nodes.read.status, "completed");

  latest = "r2";
  await liveHandler({ payload: { runId: "r2", seq: 3, kind: "node.started", nodeId: "read" } });
  assert.equal(controller.getProjection().runId, "r2");
  assert.equal(controller.getProjection().nodes.read.status, "running");
  await liveHandler({ payload: { runId: "r1", seq: 4, kind: "run.completed" } });
  assert.equal(controller.getProjection().runId, "r2");
  assert.equal(controller.getProjection().status, "planned");
});

test("buffers events during restore without switching back to an older run", async () => {
  let liveHandler;
  let finishRestore;
  let loads = 0;
  const snapshot = [{ run_id: "new", seq: 1, kind: "run.started" }];
  const mounting = restoreAndSubscribeAgentEvents({
    runId: "latest",
    call: async () => {
      loads += 1;
      if (loads === 1) return new Promise((resolve) => { finishRestore = resolve; });
      return snapshot;
    },
    listen: async (_event, handler) => { liveHandler = handler; return () => {}; },
    onProjection: () => {}, schedule: (fn) => fn(),
  });
  await Promise.resolve();
  liveHandler({ payload: { runId: "old", seq: 9, kind: "run.completed" } });
  liveHandler({ payload: { runId: "new", seq: 2, kind: "run.completed" } });
  finishRestore(snapshot);
  const controller = await mounting;
  assert.equal(controller.getProjection().runId, "new");
  assert.equal(controller.getProjection().status, "completed");
  assert.equal(controller.getProjection().seq, 2);
});

test("fixed run subscriptions ignore other runs and stop publishing after disposal", async () => {
  let liveHandler;
  let stopped = false;
  const projections = [];
  const controller = await restoreAndSubscribeAgentEvents({
    runId: "r1", call: async () => [],
    listen: async (_event, handler) => { liveHandler = handler; return () => { stopped = true; }; },
    onProjection: (projection) => projections.push(projection), schedule: (fn) => fn(),
  });
  await liveHandler({ payload: { runId: "other", seq: 1, kind: "run.started" } });
  assert.equal(controller.getProjection().seq, 0);
  controller.unlisten();
  await liveHandler({ payload: { runId: "r1", seq: 1, kind: "run.started" } });
  assert.equal(stopped, true);
  assert.equal(projections.length, 1);
});
