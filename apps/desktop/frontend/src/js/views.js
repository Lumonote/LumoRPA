// View tab routing: top views (design/recorder/runs/models/features/settings),
// editor sub-views (steps/graph/tree/code), and the right-rail section tabs.

import { $, $$, reportError } from "./dom.js";
import { state } from "./state.js";
import { renderStepList } from "./editor/steps.js";
import { renderGraph } from "./editor/graph.js";
import { renderTree } from "./editor/tree.js";
import { renderCode } from "./editor/code.js";
import { renderCapabilitiesPanel } from "./capabilities.js";
import { refreshRuns } from "./runs.js";
import { refreshProviders } from "./providers.js";
import { renderFeatures } from "./features.js";
import { refreshSettings } from "./settings.js";
import { refreshRecorder } from "./recorder.js";

export function switchTopView(view) {
  state.currentView = view;
  $$(".tabs .tab").forEach((b) => b.classList.toggle("is-active", b.dataset.view === view));
  const isDesign = view === "design";
  $("designView").style.display = isDesign ? "" : "none";
  $("rightRail").style.display = isDesign ? "" : "none";
  // Left rail visible for design / recorder (because recorder also uses flow context)
  document.querySelector(".left-rail").style.display = isDesign ? "" : "none";

  const map = {
    recorder: "recorderView",
    runs: "runsView",
    models: "modelsView",
    features: "featuresView",
    settings: "settingsView",
  };
  Object.values(map).forEach((id) => ($(id).style.display = "none"));
  if (map[view]) $(map[view]).style.display = "";

  if (view === "runs") refreshRuns().catch(reportError);
  if (view === "models") refreshProviders().catch(reportError);
  if (view === "features") renderFeatures();
  if (view === "settings") refreshSettings().catch(reportError);
  if (view === "recorder") refreshRecorder().catch(() => {});
}

export function switchEditorMode(mode) {
  state.viewMode = mode;
  $$("#viewSwitch button").forEach((b) => b.classList.toggle("is-active", b.dataset.mode === mode));
  $$(".editor-body .view").forEach((v) => v.classList.toggle("is-active", v.id === `view-${mode}`));
  if (mode === "steps") renderStepList();
  if (mode === "graph") renderGraph();
  if (mode === "tree") renderTree();
  if (mode === "code") renderCode();
}

export function switchRightSection(name) {
  state.rightSection = name;
  $$("#rightTabs button").forEach((b) => b.classList.toggle("is-active", b.dataset.section === name));
  ["rsInspector", "rsInputs", "rsOutputs", "rsCapabilities"].forEach((id) => {
    const target = id === `rs${name.charAt(0).toUpperCase()}${name.slice(1)}`;
    $(id).classList.toggle("is-active", target);
  });
  if (name === "capabilities") {
    renderCapabilitiesPanel().catch((e) => console.error("renderCapabilitiesPanel:", e));
  }
}
