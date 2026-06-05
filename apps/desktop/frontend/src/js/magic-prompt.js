// F-18 Magic Prompt: natural-language → Flow. Opens a drawer, calls the
// `generate_flow` command (shared lumo_ai::copilot core, same as `lumo copilot`),
// then saves the generated YAML as a new flow and opens it in the editor.

import { $, html, toast } from "./dom.js";
import { call, errorMessage } from "./api.js";
import { refreshFlows, loadFlow } from "./flows.js";
import { state } from "./state.js";
import { refreshActiveProviderPill } from "./providers.js";

export async function openMagicPrompt() {
  if (!state.providers) {
    try {
      state.providers = await call("provider_status");
    } catch (_) {
      state.providers = null;
    }
  }

  const overlay = $("magicPromptOverlay");
  overlay.hidden = false;
  overlay.innerHTML = `
    <div class="ai-drawer">
      <header>
        <strong>✨ Magic Prompt · 自然语言生成流程</strong>
        <button class="icon" id="mpClose">×</button>
      </header>
      <div class="ai-drawer-body">
        <div class="prop-field">
          <label>描述你想要的流程</label>
          <textarea id="mpPrompt" rows="5" placeholder="例如：每天早上 9 点抓取某网页的标题，写入 result.xlsx"></textarea>
        </div>
        <div class="prop-field">
          <label>模型</label>
          ${renderModelSelect()}
        </div>
        <span class="hint">
          由配置的 LLM 生成 lumo/v1 流程并自动校验（最多重试 2 次），成功后另存为新流程并打开。
          需先在「模型」配置 LLM；网络未开启时可在模型页或底部状态栏启用本次会话网络。
        </span>
        <div id="mpStatus" class="hint"></div>
        <pre id="mpError" class="error-detail" hidden></pre>
      </div>
      <footer>
        <button id="mpCancel">取消</button>
        <button class="primary" id="mpGenerate">生成</button>
      </footer>
    </div>`;

  const close = () => { overlay.hidden = true; overlay.innerHTML = ""; };
  $("mpClose").addEventListener("click", close);
  $("mpCancel").addEventListener("click", close);
  overlay.addEventListener("click", (e) => { if (e.target === overlay) close(); });

  $("mpGenerate").addEventListener("click", async () => {
    const prompt = $("mpPrompt").value.trim();
    if (!prompt) {
      toast("请输入描述", "Magic Prompt 需要一段自然语言描述", "bad");
      return;
    }
    const model = $("mpModel").value.trim();
    const btn = $("mpGenerate");
    const status = $("mpStatus");
    btn.disabled = true;
    status.textContent = "正在生成…（调用 LLM，可能需要几秒）";
    try {
      if (!state.providers?.networkEnabled) {
        status.textContent = "正在启用本次会话 LLM 网络…";
        state.providers = await call("enable_llm_network_for_session");
        refreshActiveProviderPill();
      }
      status.textContent = "正在生成…（调用 LLM，可能需要几秒）";
      const yaml = await call("generate_flow", { prompt, model: model || null });
      // Name the new flow after the generated flow's metadata.id (fallback constant).
      const name = yaml.match(/id:\s*([A-Za-z0-9_-]+)/)?.[1] || "magic-flow";
      const path = await call("save_flow_as", { name, source: yaml });
      await refreshFlows();
      await loadFlow(path);
      close();
      toast("已生成流程", `${name} · 已保存并打开`, "ok");
    } catch (e) {
      const detail = errorMessage(e);
      status.textContent = "生成失败，详情如下。";
      const errBox = $("mpError");
      errBox.hidden = false;
      errBox.textContent = detail;
      btn.disabled = false;
      toast("生成失败", detail, "bad");
    }
  });
}

function renderModelSelect() {
  const profiles = state.providers?.profiles || [];
  const active = profiles.find((p) => p.name === state.providers?.active);
  const activeDefault = active?.defaultModel ? modelValue(active, active.defaultModel) : "";
  const defaultLabel = activeDefault
    ? `使用活动默认（${activeDefault}）`
    : "使用活动 provider 默认";

  const groups = profiles
    .map((profile) => {
      const models = modelOptionsForProfile(profile);
      if (!models.length) return "";
      return `<optgroup label="${html(profile.name)}">
        ${models
          .map(
            (model) =>
              `<option value="${html(model.value)}">${html(model.label)}</option>`
          )
          .join("")}
      </optgroup>`;
    })
    .filter(Boolean)
    .join("");

  return `<select id="mpModel">
    <option value="">${html(defaultLabel)}</option>
    ${
      groups ||
      '<option value="" disabled>尚未配置可选模型</option>'
    }
  </select>`;
}

function modelOptionsForProfile(profile) {
  const seen = new Set();
  const options = [];
  const add = (model, suffix = "") => {
    const value = modelValue(profile, model);
    if (!value || seen.has(value)) return;
    seen.add(value);
    const labelModel = value.startsWith(`${profile.name}/`)
      ? value.slice(profile.name.length + 1)
      : value;
    options.push({
      value,
      label: `${labelModel}${suffix}`,
    });
  };
  add(profile.defaultModel, " · 默认");
  (profile.models || []).forEach((model) => add(model));
  return options;
}

function modelValue(profile, model) {
  const raw = String(model || "").trim();
  if (!raw) return "";
  return raw.startsWith(`${profile.name}/`) ? raw : `${profile.name}/${raw}`;
}
