# Mission Control UI Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Render the agent event stream as a polished, accurate serial/parallel execution topology with current-step detail, confirmations, controls, logs and accessible motion.

**Architecture:** Keep a pure event projector separate from DOM rendering. Mission Control never reads executor internals; it reconstructs state from persisted/live events. CSS animations communicate status and data flow and honor reduced-motion.

**Tech Stack:** Vanilla ESM, HTML, CSS, Node test runner, Tauri event API.

---

### Task 1: Build the pure event projector

**Files:**
- Create: `apps/desktop/frontend/src/js/agent-events.js`
- Create: `apps/desktop/frontend/test/agent-events.test.js`

- [ ] Write tests projecting serial nodes, parallel branches, retries, replacement tools, replans and out-of-order duplicate delivery.
- [ ] Implement:

```javascript
export function createRunProjection(runId) { return { runId, seq: 0, nodes: new Map(), edges: [], status: "idle" }; }
export function applyAgentEvent(projection, event) { /* ignore seq <= projection.seq; return new projection */ }
export function readyTopology(projection) { return { lanes: [], edges: [], activeNodeId: null }; }
```

- [ ] Run `cd apps/desktop/frontend && npm test -- --test-name-pattern='agent event'` and commit `feat(desktop): project agent events into topology`.

### Task 2: Add Mission Control markup and topology renderer

**Files:**
- Modify: `apps/desktop/frontend/src/index.html`
- Create: `apps/desktop/frontend/src/js/mission-control.js`
- Create: `apps/desktop/frontend/src/styles/mission-control.css`
- Modify: `apps/desktop/frontend/src/js/state.js`
- Create: `apps/desktop/frontend/test/mission-control.test.js`

- [ ] Test safe HTML rendering, lane assignment and selected-node detail.
- [ ] Implement exported pure renderers `renderTopology`, `renderNodeDetail`, `renderRunMetrics`; use SVG only for edges and normal DOM for accessible nodes.
- [ ] Add status classes `queued`, `running`, `waiting`, `completed`, `failed`, `cancelled`, `unknown` and `replanning`.
- [ ] Run frontend tests/lint and commit `feat(desktop): render mission control execution graph`.

### Task 3: Subscribe to live events and restore persisted runs

**Files:**
- Modify: `apps/desktop/frontend/src/js/app-events.js`
- Modify: `apps/desktop/frontend/src/js/main.js`
- Modify: `apps/desktop/frontend/src/js/mission-control.js`
- Create: `apps/desktop/frontend/test/agent-live-events.test.js`

- [ ] Test that persisted events load before live subscription, duplicate sequences are ignored, and reconnect fills gaps with `agent_events(afterSeq)`.
- [ ] Bind Tauri `lumo://agent-event` to the projector and render at most once per animation frame.
- [ ] Run frontend tests/lint and commit `feat(desktop): stream agent progress into mission control`.

### Task 4: Implement risk confirmation and interruption controls

**Files:**
- Create: `apps/desktop/frontend/src/js/agent-confirmation.js`
- Modify: `apps/desktop/frontend/src/js/mission-control.js`
- Create: `apps/desktop/frontend/test/agent-confirmation.test.js`

- [ ] Test L2/L3 copy, argument redaction, plan-hash binding, approve/reject, pause/resume/cancel/skip and stale approval rejection.
- [ ] Implement confirmation cards that show capability source, affected target, redacted arguments, risk reason and whether approval is once/session/profile.
- [ ] Bind controls to agent commands and disable them while a response is in flight.
- [ ] Run tests and commit `feat(desktop): add agent approvals and controls`.

### Task 5: Add adaptive feedback, logs and motion accessibility

**Files:**
- Create: `apps/desktop/frontend/src/js/agent-feedback.js`
- Modify: `apps/desktop/frontend/src/styles/mission-control.css`
- Create: `apps/desktop/frontend/test/agent-feedback.test.js`

- [ ] Test summary selection, long-result suppression, error redaction and quiet mode.
- [ ] Add data-flow edge animation, active-node breathing, failure rewind and progress shimmer with `@media (prefers-reduced-motion: reduce)` disabling nonessential motion.
- [ ] Render a virtualized/capped log panel with dropped-event indicator and copyable redacted diagnostics.
- [ ] Run `npm test && npm run lint` and commit `feat(desktop): polish mission control feedback and motion`.

### Task 6: Verify desktop visual behavior

**Files:**
- Create: `apps/desktop/frontend/test/fixtures/agent-events.json`
- Modify: `apps/desktop/README.md`

- [ ] Start the desktop app with a fixture-event debug command and capture serial, parallel, failure, replan and reduced-motion states.
- [ ] Verify keyboard focus, screen-reader labels, 1280×720 and 2560×1440 layouts, and no animation-driven layout shifts.
- [ ] Run `cargo test -p lumorpa-desktop`, frontend tests and lint; commit `test(desktop): verify mission control states`.

