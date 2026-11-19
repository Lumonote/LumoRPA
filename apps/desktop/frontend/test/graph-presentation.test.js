import test from "node:test";
import assert from "node:assert/strict";
import { buildGraphModel } from "../src/js/editor/graph-model.js";
import { fitGraphBounds, zoomGraphAt, screenToWorld } from "../src/js/editor/graph-viewport.js";
import { projectGraphPresentation, searchGraphNodes, alignedPositions, snappedDelta, graphRouteKey, normalizeGraphLayouts, isValidGraphView } from "../src/js/editor/graph-presentation.js";

const step = (id, extra = {}) => ({ id, action: "control.log", ...extra });

test("fit includes all endpoints of 80, 200 and 1000 nodes and negative coordinates", () => {
  for (const count of [80, 200, 1000]) {
    const model = buildGraphModel(Array.from({ length: count }, (_, i) => step(`n${i}`)), { layouts: { "id:n0": { x: -1000, y: -200 } } });
    const view = fitGraphBounds(model.bounds, 1000, 900);
    assert.ok(model.bounds.x * view.scale + view.tx >= 0);
    assert.ok((model.bounds.x + model.bounds.width) * view.scale + view.tx <= 1000);
    const zoomed = zoomGraphAt(view, 500, 450, 1.2);
    assert.ok(Math.abs(zoomed.scale - view.scale * 1.2) < 1e-9, "overview zoom must not jump to a manual floor");
    const before = screenToWorld(view, 500, 450), after = screenToWorld(zoomed, 500, 450);
    assert.ok(Math.abs(before.x - after.x) < 1e-7 && Math.abs(before.y - after.y) < 1e-7);
  }
});

test("folded control hides descendants, preserves boundary edges and source identity", () => {
  const steps = [step("gate", { action: "control.if", do: [step("child"), step("nested", { do: [step("leaf")] })] }), step("use", { with: { value: "{{ steps.child.result }}" } })];
  const before = JSON.stringify(steps), model = buildGraphModel(steps);
  const view = projectGraphPresentation(model, { "id:gate": { collapsed: true } });
  assert.deepEqual([...view.nodes.keys()], ["0", "1"]);
  assert.equal(view.nodes.get("0").members.length, 3);
  const data = view.edges.find((edge) => edge.kind === "data");
  assert.equal(data.from, "0"); assert.equal(data.originalFrom, "0/:do/0"); assert.equal(data.boundary, true);
  assert.equal(JSON.stringify(steps), before);
  assert.equal(model.nodes.size, 5);
});

test("collapsed group retains all external links and expands without losing saved coordinates", () => {
  const layouts = { "id:b": { x: 700, y: 200 }, __studio: { version: 1, groups: [{ id: "g", title: "Group", members: ["id:b", "id:c"], collapsed: true }] } };
  const model = buildGraphModel([step("a"), step("b"), step("c"), step("d")], { layouts });
  const view = projectGraphPresentation(model, layouts);
  assert.deepEqual([...view.nodes.keys()], ["0", "1", "3"]);
  assert.deepEqual(view.edges.map((edge) => [edge.from, edge.to]), [["0", "1"], ["1", "3"]]);
  assert.equal(view.nodes.get("1").x, 700);
  layouts.__studio.groups[0].collapsed = false;
  const expanded = projectGraphPresentation(model, layouts);
  assert.equal(expanded.nodes.size, 4); assert.equal(expanded.frames.length, 1);
});

test("search reaches hidden nodes and recursive parameter values safely", () => {
  const value = {}; value.self = value;
  const nodes = buildGraphModel([step("Parent", { do: [step("Needle", { with: { message: "content" } })] }), step("recursive", { with: value })]).nodes;
  assert.equal(searchGraphNodes(nodes, "needle")[0].step.id, "Needle");
  assert.equal(searchGraphNodes(nodes, "content")[0].step.id, "Needle");
  assert.equal(searchGraphNodes(nodes, "recursive").length, 1);
});

test("alignment and snapping preserve relative group spacing", () => {
  const nodes = [{ key: "a", x: 3, y: 8, w: 260, h: 140 }, { key: "b", x: 400, y: 90, w: 260, h: 140 }];
  assert.deepEqual(alignedPositions(nodes).get("b"), { x: 400, y: 8 });
  assert.deepEqual(alignedPositions(nodes, "left").get("b"), { x: 3, y: 90 });
  assert.deepEqual(snappedDelta(nodes[0], 20, 20, 2), { x: 9, y: 16 });
  assert.deepEqual(snappedDelta(nodes[0], 20, 20, 2, false), { x: 10, y: 10 });
});

test("route identity survives node insertion and source reorder", () => {
  const a = buildGraphModel([step("read"), step("write", { with: { content: "{{ steps.read.result }}" } })]);
  const b = buildGraphModel([step("new"), step("read"), step("write", { with: { content: "{{ steps.read.result }}" } })]);
  assert.equal(graphRouteKey(a.edges.find((e) => e.kind === "data"), a.nodes), graphRouteKey(b.edges.find((e) => e.kind === "data"), b.nodes));
});

test("layout normalization preserves valid coordinates, folded state, routes and object references", () => {
  const position = { x: -120, y: 0, collapsed: true };
  const members = ["id:a", "id:b"];
  const group = { id: "g", title: "Steps", color: "blue", members, collapsed: true };
  const groups = [group], route = { x: -35, y: 450 }, routes = { "legacy-route-key": route };
  const view = { tx: -100, ty: 15, scale: 1e-12 };
  const studio = { version: 1, groups, routes, view, snap: false };
  const layouts = { "id:a": position, "id:b": { collapsed: false }, __studio: studio };
  const expected = globalThis.structuredClone(layouts);
  assert.equal(normalizeGraphLayouts(layouts), layouts);
  assert.equal(normalizeGraphLayouts(layouts), layouts);
  assert.deepEqual(layouts, expected);
  assert.equal(layouts["id:a"], position);
  assert.equal(layouts.__studio, studio);
  assert.equal(studio.groups, groups);
  assert.equal(studio.groups[0], group);
  assert.equal(group.members, members);
  assert.equal(studio.routes, routes);
  assert.equal(routes["legacy-route-key"], route);
  assert.equal(studio.view, view);
});

test("layout normalization rejects malformed groups and nonfinite geometry without losing usable data", () => {
  const group = { id: "g", title: null, color: "invalid", members: [null, "id:a", "", "id:a", 1, "id:b"], collapsed: "false" };
  const layouts = {
    "id:a": { x: Infinity, y: 2, collapsed: true },
    "id:b": { x: 1, y: NaN, collapsed: "false" },
    "id:c": null, "id:d": [], "id:e": { x: 3 },
    __studio: { version: 1, groups: [null, 1, {}, { id: "bad", members: "id:a" }, group, { id: "g", members: ["id:e"] }], routes: { good: { x: 0, y: -3 }, nil: null, array: [], bad: { x: NaN, y: 0 }, partial: { x: 1 } }, view: { tx: 0, ty: 0, scale: 0 }, snap: "false" },
  };
  normalizeGraphLayouts(layouts);
  assert.deepEqual(layouts["id:a"], { collapsed: true });
  assert.deepEqual(layouts["id:b"], {});
  assert.deepEqual(layouts["id:e"], {});
  assert.equal(Object.hasOwn(layouts, "id:c"), false);
  assert.equal(Object.hasOwn(layouts, "id:d"), false);
  assert.deepEqual(layouts.__studio.groups, [group]);
  assert.deepEqual(group, { id: "g", title: "分组", color: "sage", members: ["id:a", "id:b"], collapsed: false });
  assert.deepEqual(layouts.__studio.routes, { good: { x: 0, y: -3 } });
  assert.equal(Object.hasOwn(layouts.__studio, "view"), false);
  assert.equal(layouts.__studio.snap, true);
});

test("malformed persisted layout shapes cannot break graph projection", () => {
  const model = buildGraphModel([step("a"), step("b")]);
  for (const layouts of [null, [], "invalid", { __studio: [] }, { __studio: { version: 1, groups: {} } }, { __studio: { version: 1, groups: [null, {}, { id: "g", members: null }] } }]) {
    assert.equal(projectGraphPresentation(model, layouts).nodes.size, 2);
  }
});

test("saved viewport requires finite translation and a positive finite scale", () => {
  for (const view of [null, [], {}, { tx: 0, ty: 0, scale: 0 }, { tx: 0, ty: 0, scale: -1 }, { tx: 0, ty: 0, scale: Infinity }, { tx: NaN, ty: 0, scale: 1 }, { tx: 0, ty: "0", scale: 1 }]) {
    assert.equal(isValidGraphView(view), false);
    const layouts = { __studio: { version: 1, view } };
    normalizeGraphLayouts(layouts);
    assert.equal(Object.hasOwn(layouts.__studio, "view"), false);
  }
  assert.equal(isValidGraphView({ tx: 0, ty: -20, scale: 0.00001 }), true);
});
