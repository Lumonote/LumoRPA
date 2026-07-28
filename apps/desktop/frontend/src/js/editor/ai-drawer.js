// Per-step AI drawer (the ✨ button). Edits the step's `ai:` block and,
// optionally, grants `llm: ["*"]` capability.

import { $, html, toast } from "../dom.js";
import { state } from "../state.js";
import {
  extractSteps, findStepByPath, mutateStepInSource, parseYaml, ensureLlmCapability,
} from "../yaml.js";
import { AI_MODES, AI_LABEL } from "../constants.js";
import { renderActiveView } from "./render.js";

export function openAiDrawer(path) {
  const step = findStepByPath(extractSteps(state.ast), path);
  if (!step) return;
  const ai = step.ai || {};
  const mode = (ai.mode || "off").toLowerCase();
  const overlay = $("aiDrawerOverlay");
  overlay.hidden = false;
  overlay.innerHTML = `
    <div class="ai-drawer">
      <header>
        <strong>✨ AI 配置 · 步骤 ${html(step.id || "(no id)")}</strong>
        <button class="icon" id="aiDrawerClose">×</button>
      </header>
      <div class="ai-drawer-body">
        <div class="prop-field">
          <label>模式</label>
          <div class="ai-mode-row">
            ${AI_MODES.map((m) => `
              <label class="ai-mode-pill ${mode === m ? "is-active" : ""}">
                <input type="radio" name="aiMode" value="${m}" ${mode === m ? "checked" : ""}/>
                <span>${html(AI_LABEL[m])}</span>
              </label>`).join("")}
          </div>
          <span class="hint">
            <strong>关</strong>：仅按确定性执行。
            <strong>兜底</strong>：动作失败后让 AI 重试（自愈选择器、视觉抽取、AI 判定）。
            <strong>主导</strong>：直接交给 AI（当前 control.if 走 AI 决策；其余动作回落兜底语义）。
          </span>
        </div>
        <div class="prop-field">
          <label>模型 override（空 = 沿用流程默认）</label>
          <input id="aiDrawerModel" value="${html(ai.model || "")}" placeholder="gpt-4o-mini / claude-opus-4-7" />
        </div>
        <div class="prop-field">
          <label>Prompt / 目标描述</label>
          <textarea id="aiDrawerPrompt" placeholder="自然语言描述目标，例如：点击搜索按钮 / 抽取标题">${html(ai.prompt || "")}</textarea>
        </div>
        <label class="toggle"><input type="checkbox" id="aiDrawerAddCap" checked /> 同时为 spec.capabilities 添加 llm: ["*"]</label>
      </div>
      <footer>
        <button id="aiDrawerCancel">取消</button>
        <button class="primary" id="aiDrawerSave">保存</button>
      </footer>
    </div>`;

  const close = () => { overlay.hidden = true; overlay.innerHTML = ""; };
  $("aiDrawerClose").addEventListener("click", close);
  $("aiDrawerCancel").addEventListener("click", close);
  overlay.addEventListener("click", (e) => { if (e.target === overlay) close(); });
  $("aiDrawerSave").addEventListener("click", () => {
    const newMode = (overlay.querySelector("input[name='aiMode']:checked")?.value || "off").toLowerCase();
    const newModel = $("aiDrawerModel").value.trim();
    const newPrompt = $("aiDrawerPrompt").value.trim();
    const addCap = $("aiDrawerAddCap").checked;
    const newAi = (newMode === "off" && !newModel && !newPrompt) ? null : {
      mode: newMode,
      ...(newModel ? { model: newModel } : {}),
      ...(newPrompt ? { prompt: newPrompt } : {}),
    };
    let updated = mutateStepInSource(state.source, step.id, {
      id: step.id, action: step.action, with: step.with, retry: step.retry, when: step.when, bind: step.bind,
      ai: newAi,
      do: step.do, else: step.else, catch: step.catch, finally: step.finally, branches: step.branches,
    });
    if (addCap && newMode !== "off") updated = ensureLlmCapability(updated);
    state.source = updated;
    state.ast = parseYaml(state.source);
    close();
    toast("已写入 ai 配置", `step ${step.id} · mode=${newMode}`, "ok");
    renderActiveView();
  });
}
