import test from "node:test";
import assert from "node:assert/strict";

import { restoreAndSubscribeAgentEvents } from "../src/js/app-events.js";

test("restores persisted events before subscribing and fills sequence gaps", async () => {
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

  assert.deepEqual(order, ["load:0", "listen", "load:1", "load:3"]);
  assert.equal(projections.at(-1).seq, 5);
  assert.equal(projections.at(-1).status, "completed");
});
