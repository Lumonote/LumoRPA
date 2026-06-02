// Entry point: wire all DOM events, then boot — load flows/actions/providers/
// settings/feature-map and select an initial flow.

import { $, $$, toast, setStatus, pretty, reportError } from "./dom.js";
import { call } from "./api.js";
import { state } from "./state.js";
import { parseYaml } from "./yaml.js";
import { applyTheme, applyWindowAlpha, applyPanelAlpha, applyPreset } from "./theme.js";
import { switchTopView, switchEditorMode, switchRightSection } from "./views.js";
import {
  refreshFlows, createNewFlow, saveCurrentFlowAs, loadFlow, defaultInputs, saveFlowSource,
  renderFlowList,
} from "./flows.js";
import { refreshActions, renderActions } from "./actions.js";
import { setElTab, renderElementLibrary, elementById } from "./elements.js";
import { appendStepToSource, appendStepWithSelector } from "./editor/mutations.js";
import { syncGutter } from "./editor/code.js";
import { runSelectedFlow, runStep, refreshRuns } from "./runs.js";
import {
  refreshProviders, openProviderEditor, saveProvider, testProvider, renderProviderList,
  refreshActiveProviderPill,
} from "./providers.js";
import {
  ensureRecorderListener, startRecording, stopRecording,
} from "./recorder.js";
import { refreshSettings } from "./settings.js";
import { loadFeatureMap } from "./features.js";
import { bindGraphPan } from "./editor/graph.js";

function bindEvents() {
  // Top tabs
  $$(".tabs .tab").forEach((b) => b.addEventListener("click", () => switchTopView(b.dataset.view)));
  // Editor mode switch
  $$("#viewSwitch button").forEach((b) => b.addEventListener("click", () => switchEditorMode(b.dataset.mode)));
  // Right tabs
  $$("#rightTabs button").forEach((b) => b.addEventListener("click", () => switchRightSection(b.dataset.section)));
  // Theme + opacity
  $("themeToggleBtn").addEventListener("click", () => applyTheme(state.theme === "dark" ? "light" : "dark"));
  $$("[data-theme]").forEach((b) => b.addEventListener("click", () => applyTheme(b.dataset.theme)));
  ["windowAlphaTop", "windowAlphaSlider"].forEach((id) =>
    $(id).addEventListener("input", (e) => applyWindowAlpha(e.target.value))
  );
  ["panelAlphaTop", "panelAlphaSlider"].forEach((id) =>
    $(id).addEventListener("input", (e) => applyPanelAlpha(e.target.value))
  );
  $$("[data-preset]").forEach((b) => b.addEventListener("click", () => applyPreset(b.dataset.preset)));

  // Flow list
  $("flowList").addEventListener("click", async (e) => {
    // Fold toggle
    const head = e.target.closest("[data-toggle]");
    if (head) {
      const kind = head.dataset.toggle;
      state.flowSectionFolded ||= {};
      state.flowSectionFolded[kind] = !head.parentElement.classList.contains("is-folded")
        ? true
        : false;
      renderFlowList();
      return;
    }
    // Row icon actions (delete / duplicate)
    const act = e.target.closest("[data-act]");
    if (act) {
      e.stopPropagation();
      const path = act.dataset.path;
      try {
        if (act.dataset.act === "del") {
          if (!confirm(`确认删除流程文件：\n${path}`)) return;
          await call("delete_flow", { path });
          if (state.flowPath === path) state.flowPath = null;
          await refreshFlows();
          toast("已删除", path, "ok");
        } else if (act.dataset.act === "dup") {
          const newPath = await call("duplicate_flow", { path });
          await refreshFlows();
          await loadFlow(newPath);
          toast("已复制", newPath, "ok");
        }
      } catch (err) {
        toast("操作失败", String(err), "bad");
      }
      return;
    }
    const item = e.target.closest("[data-path]");
    if (item) loadFlow(item.dataset.path).catch(reportError);
  });
  $("flowPath").addEventListener("keydown", (e) => { if (e.key === "Enter") loadFlow().catch(reportError); });
  $("refreshFlowsBtn").addEventListener("click", () => refreshFlows().catch(reportError));
  $("newFlowBtn").addEventListener("click", () => createNewFlow().catch(reportError));
  $("saveFlowAsBtn").addEventListener("click", () => saveCurrentFlowAs().catch(reportError));

  // Action library: search + collapse + drag
  $("actionSearch").addEventListener("input", renderActions);
  $("refreshActionsBtn").addEventListener("click", () => refreshActions().catch(reportError));
  $("actionLibrary").addEventListener("click", (e) => {
    const head = e.target.closest(".action-family-head");
    if (head) head.parentElement.classList.toggle("is-collapsed");
    const item = e.target.closest("[data-action]");
    if (item && !head) appendStepToSource(item.dataset.action);
  });
  $("actionLibrary").addEventListener("dragstart", (e) => {
    const item = e.target.closest("[data-action]");
    if (item) {
      e.dataTransfer.setData("text/x-lumo-action", item.dataset.action);
      e.dataTransfer.effectAllowed = "copy";
      document.body.classList.add("is-dragging-action");
      $("stepList")?.classList.add("is-drop-active");
    }
  });
  $("actionLibrary").addEventListener("dragend", () => {
    document.body.classList.remove("is-dragging-action");
    $("stepList")?.classList.remove("is-drop-active");
    document.querySelectorAll(".flow-connector.is-drop-hover, .flow-dropzone.is-drop-hover")
      .forEach((n) => n.classList.remove("is-drop-hover"));
  });

  // Element / Image library
  $("elTabs")?.addEventListener("click", (e) => {
    const btn = e.target.closest("button[data-el-tab]");
    if (btn) setElTab(btn.dataset.elTab);
  });
  $("elSearch")?.addEventListener("input", renderElementLibrary);
  $("elCaptureBtn")?.addEventListener("click", () => {
    switchTopView("recorder");
    toast("跳到录制器", "点击 ● 开始录制 后再在页面中圈选元素", "ok");
  });
  $("elClearBtn")?.addEventListener("click", () => {
    const tab = state.elTab;
    if (!confirm(`清空当前分类（${tab}）？`)) return;
    if (tab === "elements") state.elements = [];
    else if (tab === "images") state.images = [];
    else if (tab === "datatables") state.datatables = [];
    renderElementLibrary();
  });
  $("elBody")?.addEventListener("click", async (e) => {
    const click = e.target.closest("[data-el-use-click]");
    const extract = e.target.closest("[data-el-use-extract]");
    const copy = e.target.closest("[data-el-copy]");
    if (click) {
      const el = elementById(click.dataset.elUseClick);
      if (el) appendStepWithSelector("browser.click", el);
    } else if (extract) {
      const el = elementById(extract.dataset.elUseExtract);
      if (el) appendStepWithSelector("browser.extract", el);
    } else if (copy) {
      const el = elementById(copy.dataset.elCopy);
      if (el?.fingerprints?.css) {
        try { await navigator.clipboard.writeText(el.fingerprints.css); toast("已复制", el.fingerprints.css, "ok"); }
        catch { toast("复制失败", "请手动选中文本复制", "warn"); }
      }
    }
  });
  $("elBody")?.addEventListener("dragstart", (e) => {
    const card = e.target.closest("[data-element-id]");
    if (card) {
      e.dataTransfer.setData("text/x-lumo-element", card.dataset.elementId);
      e.dataTransfer.setData("text/x-lumo-action", "browser.click");
      e.dataTransfer.effectAllowed = "copy";
      document.body.classList.add("is-dragging-action");
      $("stepList")?.classList.add("is-drop-active");
    }
  });
  $("elBody")?.addEventListener("dragend", () => {
    document.body.classList.remove("is-dragging-action");
    $("stepList")?.classList.remove("is-drop-active");
  });

  // Editor toolbar
  $("loadFlowBtn").addEventListener("click", () => loadFlow().catch(reportError));
  $("validateBtn").addEventListener("click", async () => {
    if (!state.flowPath) return;
    try {
      const r = await call("validate_flow", { path: state.flowPath });
      toast("校验通过", `${r.id} · ${r.stepCount} 步`, "ok");
      setStatus("校验通过");
    } catch (e) { toast("校验失败", String(e), "bad"); setStatus("校验失败", "bad"); }
  });
  $("runBtn").addEventListener("click", runSelectedFlow);
  $("runStepBtn").addEventListener("click", () => {
    if (state.selectedStepId) runStep(state.selectedStepId);
    else toast("先在图/树视图选中一个节点", "", "warn");
  });
  $("saveFlowBtn").addEventListener("click", () => saveFlowSource().catch(reportError));
  $("resetInputBtn").addEventListener("click", () => {
    if (state.flow) $("inputsJson").value = pretty(defaultInputs(state.flow));
  });

  // Code editor
  $("codeEditor").addEventListener("input", (e) => {
    state.source = e.target.value;
    state.ast = parseYaml(state.source);
    syncGutter();
  });
  $("codeEditor").addEventListener("scroll", () => {
    $("codeGutter").scrollTop = $("codeEditor").scrollTop;
  });

  // Runs view
  $("refreshRunsBtn").addEventListener("click", () => refreshRuns().catch(reportError));

  // Models view
  $("newProviderBtn").addEventListener("click", () => openProviderEditor(null));
  $("saveProviderBtn").addEventListener("click", saveProvider);
  $("testProviderBtn").addEventListener("click", testProvider);
  $("modelsInitBtn").addEventListener("click", async () => {
    if (!confirm("将覆盖 providers.toml 为默认四件套 (openai / anthropic / deepseek / ollama)，确认？")) return;
    try {
      state.providers = await call("init_providers", { force: true });
      renderProviderList();
      refreshActiveProviderPill();
      toast("已重置 providers.toml", state.providers.path, "ok");
    } catch (e) { toast("重置失败", String(e), "bad"); }
  });

  // Recorder
  $("recorderStartBtn").addEventListener("click", startRecording);
  $("recorderStopBtn").addEventListener("click", stopRecording);

  // System theme listener
  if (window.matchMedia) {
    window.matchMedia("(prefers-color-scheme: dark)").addEventListener("change", () => {
      if (state.theme === "auto") applyTheme("auto");
    });
  }
}

async function boot() {
  bindEvents();
  applyTheme(state.theme);
  applyWindowAlpha(state.windowAlpha);
  applyPanelAlpha(state.panelAlpha);
  bindGraphPan();
  switchRightSection("inspector");
  renderElementLibrary();
  ensureRecorderListener();

  try {
    await Promise.all([
      refreshFlows().catch(() => {}),
      refreshActions().catch(() => {}),
      refreshProviders().catch(() => {}),
      refreshSettings().catch(() => {}),
      loadFeatureMap().catch(() => {}),
    ]);
    if (state.examples[0]) {
      // Prefer user-saved → recordings → bundled examples so a returning
      // operator lands on the flow they were actually editing.
      const order = { user: 0, recording: 1, example: 2 };
      const first = [...state.examples].sort(
        (a, b) => (order[a.source] ?? 9) - (order[b.source] ?? 9),
      )[0];
      await loadFlow(first.path);
    }
    setStatus("就绪");
  } catch (e) {
    reportError(e);
  }
}

document.addEventListener("DOMContentLoaded", boot);
if (document.readyState !== "loading") boot();
