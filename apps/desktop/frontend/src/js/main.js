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
  renderFlowList, importFlowFile, exportCurrentFlow, exportFlowAtPath, revealCurrentFlowFile,
} from "./flows.js";
import { refreshActions, renderActions } from "./actions.js";
import {
  setElTab, renderElementLibrary, elementById, elementCopyText,
  deleteElementById, duplicateElementById, syncElementGroups,
  refreshElementLibrary, persistElementLibrary,
} from "./elements.js";
import { appendStepToSource, appendStepWithSelector } from "./editor/mutations.js";
import { syncGutter } from "./editor/code.js";
import { runSelectedFlow, runStep, refreshRuns, cancelActiveRun } from "./runs.js";
import { startDebug } from "./debug.js";
import { ensureHumanPromptListener } from "./human-prompt.js";
import {
  refreshProviders, openProviderEditor, saveProvider, testProvider, enableLlmNetworkForSession,
  renderProviderList, refreshActiveProviderPill,
} from "./providers.js";
import {
  ensureRecorderListener, startRecording, stopRecording,
} from "./recorder.js";
import { refreshSettings } from "./settings.js";
import { loadFeatureMap } from "./features.js";
import { bindGraphPan } from "./editor/graph.js";
import { openMagicPrompt } from "./magic-prompt.js";
import { mountCapabilityHub } from "./capability-hub.js";
import { mountMissionControl, refreshVoiceHistory, updateMissionControl } from "./mission-control.js";
import { normalizeToastEvent, reduceRunProgress, restoreAndSubscribeAgentEvents } from "./app-events.js";

let bootStarted = false;

async function bindAppEvents() {
  const listen = window.__TAURI__?.event?.listen;
  if (!listen) return;
  try {
    await listen("lumo://open-view", (event) => {
      const view = String(event?.payload || "");
      if (["design", "recorder", "runs", "models", "features", "settings", "capability-hub", "mission-control"].includes(view)) {
        switchTopView(view);
        document.querySelector(`[data-view="${CSS.escape(view)}"]`)?.click();
      }
    });
    await listen("lumo://toast", (event) => {
      const message = normalizeToastEvent(event?.payload);
      toast(message.title, message.body, message.kind);
    });
    await listen("lumo://voice-history", () => {
      refreshVoiceHistory(document, call).catch(() => {});
    });
    await listen("lumo://run-progress", (event) => {
      state.liveRunProgress = reduceRunProgress(state.liveRunProgress, event?.payload);
      const progress = state.liveRunProgress;
      if (progress.currentStep) setStatus(`运行中 · ${progress.currentStep}`, "warn");
      if (progress.logs.length) {
        const dropped = progress.droppedLogs ? `\n… 已截断 ${progress.droppedLogs} 条较早日志` : "";
        $("outputBox").textContent = `${progress.logs.join("\n")}${dropped}`;
      }
    });
    await listen("lumo://mcp-oauth-open", (event) => {
      const url = String(event?.payload?.url || "");
      if (!/^https?:\/\//i.test(url)) return toast("OAuth 地址无效", url, "bad");
      const opened = window.open(url, "_blank", "noopener,noreferrer");
      if (!opened) toast("请允许打开 OAuth 页面", url, "warn");
    });
    await listen("lumo://mcp-schema-drift", (event) => {
      const payload = event?.payload || {};
      toast("MCP Schema 发生变化", `${payload.serverId || "MCP"} · 请审核后再启用新工具结构`, "warn");
      state.capabilityHubController?.refresh?.();
    });
    await listen("lumo://voice-daemon-error", (event) => toast("语音服务异常", String(event?.payload?.error || "未知错误"), "bad"));
    await listen("lumo://agent-start-failed", (event) => toast("Agent 启动失败", String(event?.payload?.error || "未知错误"), "bad"));
    await listen("lumo://job-worker-error", (event) => toast("后台任务异常", String(event?.payload?.error || "未知错误"), "bad"));
    for (const name of ["lumo://job-updated", "lumo://job-recovered"]) {
      await listen(name, () => state.capabilityHubController?.refresh?.());
    }
  } catch (err) {
    console.warn("app event listen failed", err);
  }
}

function bindEvents() {
  // Top tabs
  $$(".tabs .tab").forEach((b) => b.addEventListener("click", () => {
    switchTopView(b.dataset.view);
    const hub = $("capabilityHubView");
    hub.style.display = b.dataset.view === "capability-hub" ? "" : "none";
    const mission = $("missionControlView");
    mission.style.display = b.dataset.view === "mission-control" ? "" : "none";
    if (b.dataset.view === "capability-hub" && !state.capabilityHubMounted) {
      state.capabilityHubController = mountCapabilityHub({ call, root: hub });
      state.capabilityHubMounted = true;
    }
    if (b.dataset.view === "mission-control" && !state.missionControlMounted) {
      mountMissionControl(mission, state.agentProjection, call);
      state.missionControlMounted = true;
    }
  }));
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
          if (state.flowPath === path) {
            state.flowPath = "";
            state.flow = null;
            $("flowTitle").textContent = "未选择流程";
            $("flowSubtitle").textContent = "从左侧流程库选择，或导入本地流程";
          }
          await refreshFlows();
          toast("已删除", path, "ok");
        } else if (act.dataset.act === "dup") {
          const newPath = await call("duplicate_flow", { path });
          await refreshFlows();
          await loadFlow(newPath);
          toast("已复制", newPath, "ok");
        } else if (act.dataset.act === "export") {
          await exportFlowAtPath(path);
        }
      } catch (err) {
        toast("操作失败", String(err), "bad");
      }
      return;
    }
    const item = e.target.closest("[data-path]");
    if (item) loadFlow(item.dataset.path).catch(reportError);
  });
  $("revealFlowBtn").addEventListener("click", () => revealCurrentFlowFile().catch(reportError));
  $("refreshFlowsBtn").addEventListener("click", () => refreshFlows().catch(reportError));
  $("newFlowBtn").addEventListener("click", () => createNewFlow().catch(reportError));
  $("importFlowBtn").addEventListener("click", () => importFlowFile().catch(reportError));
  $("exportFlowBtn").addEventListener("click", () => exportCurrentFlow().catch(reportError));
  $("magicPromptBtn").addEventListener("click", () => openMagicPrompt());
  $("saveFlowAsBtn").addEventListener("click", () => saveCurrentFlowAs().catch(reportError));

  // Action library: search + collapse + drag
  $("actionSearch").addEventListener("input", renderActions);
  $("refreshActionsBtn").addEventListener("click", () => refreshActions().catch(reportError));
  $("actionLibrary").addEventListener("click", (e) => {
    const head = e.target.closest(".action-family-head");
    if (head) {
      const family = head.parentElement.dataset.family;
      const collapsed = !head.parentElement.classList.contains("is-collapsed");
      head.parentElement.classList.toggle("is-collapsed", collapsed);
      head.setAttribute("aria-expanded", collapsed ? "false" : "true");
      if (family) {
        state.actionFamilyCollapsed[family] = collapsed;
        localStorage.setItem("lumo.actionFamilies", JSON.stringify(state.actionFamilyCollapsed));
      }
      return;
    }
    const item = e.target.closest("[data-action]");
    if (item) appendStepToSource(item.dataset.action);
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
  $("elKindFilter")?.addEventListener("change", renderElementLibrary);
  $("elUsageFilter")?.addEventListener("change", renderElementLibrary);
  $("elSyncBtn")?.addEventListener("click", async () => {
    try {
      const count = await syncElementGroups();
      toast(count ? "同步完成" : "无需同步", count ? `${count} 个云元素已更新` : "当前没有待同步元素", count ? "ok" : "warn");
    } catch (e) {
      toast("同步失败", String(e), "bad");
    }
  });
  $("elCaptureBtn")?.addEventListener("click", () => {
    switchTopView("recorder");
    toast("跳到录制器", "点击 ● 开始录制 后再在页面中圈选元素", "ok");
  });
  $("elClearBtn")?.addEventListener("click", async () => {
    const tab = state.elTab;
    if (!confirm(`清空当前分类（${tab}）？`)) return;
    if (tab === "elements") state.elements = [];
    else if (tab === "images") state.images = [];
    else if (tab === "datatables") state.datatables = [];
    renderElementLibrary();
    try {
      await persistElementLibrary();
    } catch (e) {
      toast("保存元素库失败", String(e), "bad");
    }
  });
  $("elBody")?.addEventListener("click", async (e) => {
    const use = e.target.closest("[data-el-use]");
    const copy = e.target.closest("[data-el-copy]");
    const duplicate = e.target.closest("[data-el-duplicate]");
    const del = e.target.closest("[data-el-delete]");
    if (use) {
      const el = elementById(use.dataset.elId);
      if (el) appendStepWithSelector(use.dataset.elUse, el);
    } else if (copy) {
      const el = elementById(copy.dataset.elCopy);
      const text = elementCopyText(copy.dataset.elCopy);
      if (el && text) {
        try { await navigator.clipboard.writeText(text); toast("已复制", el.label || el.id, "ok"); }
        catch { toast("复制失败", "请手动选中文本复制", "warn"); }
      }
    } else if (duplicate) {
      try {
        const clone = await duplicateElementById(duplicate.dataset.elDuplicate);
        if (clone) toast("已创建副本", clone.label || clone.id, "ok");
      } catch (err) {
        toast("保存元素库失败", String(err), "bad");
      }
    } else if (del) {
      const el = elementById(del.dataset.elDelete);
      if (!el || !confirm(`删除元素：${el.label || el.id}？`)) return;
      try {
        const count = await deleteElementById(del.dataset.elDelete);
        if (count) toast("已删除元素", el.label || el.id, "warn");
      } catch (err) {
        toast("保存元素库失败", String(err), "bad");
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
  $("lintBtn").addEventListener("click", async () => {
    if (!state.flowPath) { toast("先选择一个流程", "", "warn"); return; }
    try {
      const issues = await call("lint_flow", { path: state.flowPath });
      if (!issues.length) {
        toast("Lint 通过", "未发现结构 / 权限 / 变量引用问题", "ok");
        setStatus("Lint 通过", "ok");
        return;
      }
      const errors = issues.filter((i) => i.severity === "error").length;
      const warns = issues.filter((i) => i.severity === "warn").length;
      const infos = issues.length - errors - warns;
      const detail = issues
        .slice(0, 8)
        .map((i) => `[${i.severity}] ${i.step ? i.step + ": " : ""}${i.message}`)
        .join("\n");
      const tone = errors ? "bad" : warns ? "warn" : "ok";
      toast(`Lint: ${errors} 错误 · ${warns} 警告 · ${infos} 提示`, detail, tone);
      setStatus(`Lint: ${errors} 错误 / ${warns} 警告`, tone);
    } catch (e) { toast("Lint 失败", String(e), "bad"); setStatus("Lint 失败", "bad"); }
  });
  $("runBtn").addEventListener("click", runSelectedFlow);
  $("runStepBtn").addEventListener("click", () => {
    if (state.selectedStepId) runStep(state.selectedStepId);
    else toast("先在图/树视图选中一个节点", "", "warn");
  });
  $("debugBtn").addEventListener("click", () => startDebug());
  $("cancelRunBtn").addEventListener("click", () => cancelActiveRun());
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
  $("enableLlmNetworkBtn").addEventListener("click", () => enableLlmNetworkForSession().catch(reportError));
  $("netPill").addEventListener("click", () => {
    if (!state.providers?.networkEnabled) enableLlmNetworkForSession().catch(reportError);
  });
  $("modelsInitBtn").addEventListener("click", async () => {
    if (!confirm("将覆盖 providers.toml 为默认模型源 (openai / anthropic / deepseek / local-ocr / ollama)，确认？")) return;
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
  if (bootStarted) return;
  bootStarted = true;

  bindEvents();
  applyTheme(state.theme);
  applyWindowAlpha(state.windowAlpha);
  applyPanelAlpha(state.panelAlpha);
  bindGraphPan();
  switchRightSection("inspector");
  renderElementLibrary();
  ensureRecorderListener();
  ensureHumanPromptListener();
  bindAppEvents();
  const agentListen = window.__TAURI__?.event?.listen;
  if (agentListen) restoreAndSubscribeAgentEvents({ runId: "latest", call, listen: agentListen, onProjection: (projection) => {
    state.agentProjection = projection;
    if (state.missionControlMounted) updateMissionControl($("missionControlView"), projection);
  } }).catch(() => {});

  try {
    await Promise.all([
      refreshFlows().catch(() => {}),
      refreshActions().catch(() => {}),
      refreshProviders().catch(() => {}),
      refreshSettings().catch(() => {}),
      loadFeatureMap().catch(() => {}),
      refreshElementLibrary().catch(() => {}),
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
