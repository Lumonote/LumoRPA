import test from "node:test";
import assert from "node:assert/strict";

const storage = new Map();
globalThis.localStorage = { getItem: (key) => storage.get(key) ?? null, setItem: (key, value) => storage.set(key, String(value)), removeItem: (key) => storage.delete(key) };
globalThis.window = {};
globalThis.CustomEvent = class { constructor(type, options = {}) { this.type = type; this.detail = options.detail; } };
const { state, graph } = await import("../src/js/state.js");
const { parseYaml } = await import("../src/js/yaml.js");
const { renderGraph, applyGraphTransform, updateGraphGeometry } = await import("../src/js/editor/graph.js");
const { switchEditorMode } = await import("../src/js/views.js");
const { flushScheduledGraphLayouts } = await import("../src/js/editor/graph-layout-store.js");
const { bindGraphInteractions } = await import("../src/js/editor/graph-interactions.js");

// Keep model construction, navigation, view routing and layout persistence real.
// This host supplies SVG attributes/children and the small editor DOM boundary.
function mount(context) {
  context.mock.method(globalThis, "setTimeout", () => 0);
  const ids = new Map(), listeners = new Map();
  class Element {
    constructor(tag = "div") {
      this.tagName = tag; this.children = []; this.dataset = {}; this.style = {}; this.attributes = new Map(); this.fields = new Map();
      this.hidden = true; this.value = ""; this.textContent = ""; this.listeners = new Map();
      const classes = new Set();
      this.classList = {
        toggle: (name, on) => { const enabled = on === undefined ? !classes.has(name) : on; if (enabled) classes.add(name); else classes.delete(name); return enabled; },
        add: (name) => classes.add(name), remove: (name) => classes.delete(name), contains: (name) => classes.has(name),
      };
    }
    set id(value) { this._id = value; ids.set(value, this); }
    get id() { return this._id; }
    setAttribute(name, value) {
      this.attributes.set(name, String(value));
      if (name === "id") this.id = String(value);
      if (name.startsWith("data-")) this.dataset[name.slice(5).replace(/-([a-z])/g, (_, letter) => letter.toUpperCase())] = String(value);
    }
    getAttribute(name) { return this.attributes.get(name) ?? null; }
    appendChild(child) { this.children.push(child); child.parent = this; return child; }
    replaceChildren(...children) { this.children = []; for (const child of children) if (child.tagName === "#fragment") for (const nested of child.children) this.appendChild(nested); else this.appendChild(child); }
    addEventListener(type, handler) { if (!this.listeners.has(type)) this.listeners.set(type, new Set()); this.listeners.get(type).add(handler); }
    removeEventListener(type, handler) { this.listeners.get(type)?.delete(handler); }
    dispatchEvent(event) { for (const handler of this.listeners.get(event.type) || []) handler(event); }
    setPointerCapture() {}
    hasPointerCapture() { return false; }
    matches(selector) { return selector.startsWith(".") ? (this.className || "").split(" ").includes(selector.slice(1)) : this.tagName.toLowerCase() === selector.toLowerCase(); }
    closest(selector) { return this.matches(selector) ? this : this.parent?.closest(selector) || null; }
    querySelectorAll(selector) { return this.children.flatMap((child) => [...(child.matches(selector) ? [child] : []), ...child.querySelectorAll(selector)]); }
    querySelector(selector) {
      const found = this.querySelectorAll(selector)[0];
      if (found || !selector.startsWith("[")) return found || null;
      if (!this.fields.has(selector)) this.fields.set(selector, new Element());
      return this.fields.get(selector);
    }
    getBoundingClientRect() { return { x: 0, y: 0, left: 0, top: 0, width: 1000, height: 800 }; }
    focus() { globalThis.document.activeElement = this; }
    remove() {}
  }
  const get = (id) => { if (!ids.has(id)) { const element = new Element(); element.id = id; } return ids.get(id); };
  const wrap = new Element(); wrap.className = "graph-wrap";
  const toolbar = new Element(); toolbar.className = "graph-studio-actions"; wrap.appendChild(toolbar);
  wrap.appendChild(get("graphSvg")); wrap.appendChild(get("graphRoot"));
  globalThis.document = {
    activeElement: null,
    getElementById: get,
    createElement: (tag) => new Element(tag),
    createElementNS: (_namespace, tag) => new Element(tag),
    createDocumentFragment: () => new Element("#fragment"),
    querySelectorAll: () => [], querySelector: () => null,
    addEventListener: (type, handler) => { if (!listeners.has(type)) listeners.set(type, []); listeners.get(type).push(handler); },
    dispatchEvent: (event) => { for (const handler of listeners.get(event.type) || []) handler(event); return true; },
  };
  const windowEvents = new Element();
  globalThis.window.addEventListener = windowEvents.addEventListener.bind(windowEvents);
  globalThis.window.removeEventListener = windowEvents.removeEventListener.bind(windowEvents);
  Object.assign(state, {
    currentView: "design", viewMode: "graph", graphLayouts: {}, selectedStepPath: null,
    selectedStepPaths: new Set(), selectedStepId: null, editorRunContext: null,
    schemaCache: new Map([["control.log", { properties: {} }]]), breakpoints: new Set(),
  });
  return { get, windowEvents };
}

function setFlow(path, ids, layout = {}) {
  state.flowPath = path; state.flow = { id: path, path };
  state.source = `spec:\n  steps:\n${ids.map((id) => `    - { id: ${id}, action: control.log }`).join("\n")}`;
  state.savedSource = state.source; state.ast = parseYaml(state.source);
  state.graphLayouts[path] = layout;
  state.selectedStepPath = null; state.selectedStepId = null;
}

test("result navigation from code mode rebuilds the current flow before locating its node", async (context) => {
  const ui = mount(context);
  globalThis.window.matchMedia = () => ({ matches: true });
  setFlow("/flows/isolation-a.yaml", ["aStart", "aTarget"]);
  renderGraph();
  assert.equal(graph.allNodePositions.get("1").step.id, "aTarget");
  switchEditorMode("code");
  setFlow("/flows/isolation-b.yaml", ["bStart", "bTarget"]);
  assert.equal(graph.allNodePositions.get("1").step.id, "aTarget", "the dormant canvas still has A's model");
  globalThis.document.dispatchEvent(new globalThis.CustomEvent("lumo:focus-node", { detail: { path: "bTarget" } }));
  assert.equal(state.viewMode, "graph");
  assert.equal(graph.modelFlowKey, state.flowPath);
  assert.equal(graph.modelSource, state.source);
  assert.equal(graph.allNodePositions.get("1").step.id, "bTarget");
  assert.deepEqual(state.selectedStepPath, [1]);
  assert.equal(state.selectedStepId, "bTarget");
  assert.equal(ui.get("rightRail").classList.contains("is-graph-open"), false);
  assert.equal(ui.get("graphInspectorToggle").getAttribute("aria-expanded"), "false");
  await flushScheduledGraphLayouts();
});

test("a dormant canvas transform cannot persist another flow's viewport", async (context) => {
  mount(context);
  const a = "/flows/transform-a.yaml", b = "/flows/transform-b.yaml";
  setFlow(a, ["a"]); renderGraph();
  await flushScheduledGraphLayouts();
  const aLayout = globalThis.structuredClone(state.graphLayouts[a]);
  const aSaved = storage.get(`lumo.graphLayout.v1:${a}`);
  switchEditorMode("code");
  setFlow(b, ["b"], { "id:b": { x: 300, y: -80 } });
  const bLayout = globalThis.structuredClone(state.graphLayouts[b]);
  Object.assign(graph, { tx: 765, ty: -345, scale: 0.25 });
  applyGraphTransform();
  await flushScheduledGraphLayouts();
  assert.deepEqual(state.graphLayouts[a], aLayout);
  assert.deepEqual(state.graphLayouts[b], bLayout);
  assert.equal(storage.get(`lumo.graphLayout.v1:${a}`), aSaved);
  assert.equal(storage.has(`lumo.graphLayout.v1:${b}`), false);
  state.viewMode = "graph"; // A's canvas remains stale until a render occurs.
  applyGraphTransform();
  await flushScheduledGraphLayouts();
  assert.deepEqual(state.graphLayouts[a], aLayout);
  assert.deepEqual(state.graphLayouts[b], bLayout);
  assert.equal(storage.has(`lumo.graphLayout.v1:${b}`), false);
});

test("dragged group members update the visible frame and its fit bounds", async (context) => {
  mount(context);
  setFlow("/flows/frame-bounds.yaml", ["first", "second"], {
    "id:first": { x: 0, y: 0 }, "id:second": { x: 400, y: 250 },
    __studio: { version: 1, groups: [{ id: "g", title: "Steps", members: ["id:first", "id:second"], collapsed: false }] },
  });
  renderGraph();
  const frame = graph.groupFrames[0];
  const previous = { x: frame.x, y: frame.y, width: frame.width, height: frame.height };
  const moved = graph.nodePositions.get("1"); moved.x = 1000; moved.y = 600;
  updateGraphGeometry(new Set(["1"]));
  assert.equal(frame.x, -22); assert.equal(frame.y, -22);
  assert.equal(frame.width, moved.x + moved.w + 44);
  assert.equal(frame.height, moved.y + moved.h + 44);
  assert.ok(frame.width > previous.width && frame.height > previous.height);
  const rect = frame.element.querySelector("rect"), header = frame.element.querySelector("foreignObject");
  assert.equal(Number(rect.getAttribute("width")), frame.width);
  assert.equal(Number(rect.getAttribute("height")), frame.height + 30);
  assert.equal(Number(rect.getAttribute("y")), frame.y - 30);
  assert.equal(Number(header.getAttribute("width")), frame.width);
  assert.equal(Number(header.getAttribute("y")), frame.y - 30);
  await flushScheduledGraphLayouts();
});

test("cancelling a route drag after switching flows restores only the original layout", async (context) => {
  const ui = mount(context), first = "/flows/route-a.yaml", second = "/flows/route-b.yaml";
  const frames = new Map(); let nextFrame = 0;
  const flushFrames = () => { for (const [id, callback] of [...frames]) { frames.delete(id); callback(); } };
  const oldRaf = globalThis.requestAnimationFrame, oldCancel = globalThis.cancelAnimationFrame;
  globalThis.requestAnimationFrame = (callback) => { const id = ++nextFrame; frames.set(id, callback); return id; };
  globalThis.cancelAnimationFrame = (id) => { frames.delete(id); };
  context.after(() => { flushFrames(); globalThis.requestAnimationFrame = oldRaf; globalThis.cancelAnimationFrame = oldCancel; });
  setFlow(first, ["first", "second"]); renderGraph();
  const edge = graph.edges.find((item) => item.kind === "sequence");
  const routes = state.graphLayouts[first].__studio.routes;
  routes[edge.routeKey] = { x: 90, y: 120 };
  renderGraph();
  const dispose = bindGraphInteractions();
  const route = { dataset: { routeKey: edge.routeKey }, closest: () => ({ dataset: { edgeId: edge.id } }) };
  const target = { closest: (selector) => selector === "[data-route-key]" ? route : null };
  const pointer = { target, button: 0, pointerId: 7, clientX: 200, clientY: 220, preventDefault() {} };
  ui.get("graphSvg").dispatchEvent({ ...pointer, type: "pointerdown" });
  ui.windowEvents.dispatchEvent({ ...pointer, type: "pointermove", clientX: 280, clientY: 300 });
  flushFrames();
  assert.notDeepEqual(routes[edge.routeKey], { x: 90, y: 120 });

  setFlow(second, ["first", "second"], { __studio: { version: 1, groups: [], routes: { [edge.routeKey]: { x: 700, y: 800 } } } });
  renderGraph();
  ui.windowEvents.dispatchEvent({ ...pointer, type: "pointerup", clientX: 280, clientY: 300 });

  assert.deepEqual(routes[edge.routeKey], { x: 90, y: 120 }, "cancelled drag restores A's route");
  assert.deepEqual(state.graphLayouts[second].__studio.routes[edge.routeKey], { x: 700, y: 800 }, "B's route is untouched");
  assert.deepEqual(JSON.parse(storage.get(`lumo.graphLayout.v1:${first}`)).__studio.routes[edge.routeKey], { x: 90, y: 120 });
  dispose();
  await flushScheduledGraphLayouts();
});
