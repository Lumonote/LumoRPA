// P1（人机交互）：human.* 的桌面 UI 侧。全局监听后端 `human-prompt` 事件，
// 弹模态收集回答后调 `human_respond(promptId, value)` 投递回执。
//
// 语义与后端（lib.rs TauriPrompter）对齐：
// - 模态关闭（× / 遮罩点击 / 倒计时归零）= 不回执，让引擎自身的
//   timeout_ms 超时语义生效；
// - 迟到回执后端返回 ok=false，前端只提示“该提示已失效”，不当错误；
// - value 形状：input → string，confirm/approve → bool（decode_human_response
//   裸值直通）。
//
// 同时到达多条 prompt 时排队逐条展示（并行 run 可能同时等人）。

import { $, html, toast } from "./dom.js";
import { call } from "./api.js";
import { normalizeHumanValue, formatCountdown } from "./prompt-utils.js";

const KIND_META = {
  input: { title: "人工输入", icon: "⌨️" },
  confirm: { title: "人工确认", icon: "❓" },
  approve: { title: "人工审批", icon: "🛂" },
};

let listenerBound = false;
const queue = [];
let current = null; // { payload, timer }

export async function ensureHumanPromptListener() {
  if (listenerBound) return;
  const listen = window.__TAURI__?.event?.listen;
  if (!listen) return;
  listenerBound = true;
  try {
    await listen("human-prompt", (event) => {
      if (event?.payload?.promptId) enqueuePrompt(event.payload);
    });
  } catch (err) {
    listenerBound = false;
    console.warn("human-prompt listen failed", err);
  }
}

function enqueuePrompt(payload) {
  queue.push(payload);
  if (!current) showNext();
}

function showNext() {
  const payload = queue.shift();
  if (!payload) { current = null; return; }
  current = { payload, timer: null };
  renderPrompt(payload);
}

// 关闭当前模态。responded=false 时即“不回执”——引擎侧超时/取消自会收场。
function closeCurrent() {
  if (current?.timer) window.clearInterval(current.timer);
  current = null;
  const overlay = $("humanPromptOverlay");
  if (overlay) { overlay.hidden = true; overlay.innerHTML = ""; }
  showNext();
}

async function respond(promptId, value) {
  try {
    const r = await call("human_respond", { promptId, value });
    if (!r?.ok) toast("该提示已失效", "运行可能已超时或被取消", "warn");
  } catch (e) {
    toast("回执失败", String(e), "bad");
  }
}

function renderPrompt(payload) {
  const overlay = $("humanPromptOverlay");
  if (!overlay) return;
  const kind = String(payload.kind || "input");
  const meta = KIND_META[kind] || KIND_META.input;
  const defaultText =
    payload.default === null || payload.default === undefined
      ? ""
      : typeof payload.default === "string"
        ? payload.default
        : JSON.stringify(payload.default);

  const bodyByKind = {
    input: `
      <label class="field">
        <span>请输入内容${defaultText ? `（回车提交，默认：${html(defaultText)}）` : ""}</span>
        <input type="text" id="humanPromptInput" value="${html(defaultText)}" />
      </label>`,
    confirm: "",
    approve: "",
  };
  const footerByKind = {
    input: `
      <button class="ghost" data-hp-close>关闭（不回执）</button>
      <button class="primary" data-hp-submit>提交</button>`,
    confirm: `
      <button data-hp-no>取消</button>
      <button class="primary" data-hp-yes>确认</button>`,
    approve: `
      <button data-hp-no>拒绝</button>
      <button class="primary" data-hp-yes>批准</button>`,
  };

  overlay.innerHTML = `
    <div class="ai-drawer human-prompt-modal" role="dialog" aria-modal="true" aria-label="${html(meta.title)}">
      <header>
        <strong>${meta.icon} ${html(meta.title)}</strong>
        <button class="ghost" data-hp-close title="关闭（不回执，等待引擎超时）">✕</button>
      </header>
      <div class="ai-drawer-body">
        <div class="human-prompt-message">${html(payload.message || "")}</div>
        ${bodyByKind[kind] ?? bodyByKind.input}
        <div class="human-prompt-meta">
          <span>运行 ${html(String(payload.runId || "").slice(0, 14))}… · 节点 ${html(payload.stepPath || "-")}</span>
          <span>剩余 <strong id="humanPromptCountdown">${formatCountdown(payload.timeoutMs)}</strong></span>
        </div>
      </div>
      <footer>${footerByKind[kind] ?? footerByKind.input}</footer>
    </div>`;
  overlay.hidden = false;

  const submit = (raw) => {
    const value = normalizeHumanValue(kind, raw);
    closeCurrent();
    respond(payload.promptId, value);
  };

  overlay.querySelectorAll("[data-hp-close]").forEach((b) =>
    b.addEventListener("click", closeCurrent)
  );
  // 点遮罩本身（不是内容区）= 关闭不回执，与 ai-drawer 交互一致。
  // 用 onclick 赋值而非 addEventListener：overlay 元素跨 prompt 复用，
  // 避免监听器随弹窗次数累积。
  overlay.onclick = (e) => {
    if (e.target === overlay) closeCurrent();
  };
  overlay.querySelector("[data-hp-yes]")?.addEventListener("click", () => submit(true));
  overlay.querySelector("[data-hp-no]")?.addEventListener("click", () => submit(false));
  overlay.querySelector("[data-hp-submit]")?.addEventListener("click", () => {
    submit($("humanPromptInput")?.value ?? "");
  });
  const input = $("humanPromptInput");
  if (input) {
    input.focus();
    input.select();
    input.addEventListener("keydown", (e) => {
      if (e.key === "Enter") submit(input.value);
    });
  }

  // timeoutMs 倒计时：归零自动关闭（不回执），后端同时刻超时收场。
  const deadline = Date.now() + Number(payload.timeoutMs || 0);
  current.timer = window.setInterval(() => {
    const remain = deadline - Date.now();
    const el = $("humanPromptCountdown");
    if (el) el.textContent = formatCountdown(remain);
    if (remain <= 0) {
      toast("等待超时", `${meta.title} · ${payload.stepPath || ""}`, "warn");
      closeCurrent();
    }
  }, 500);
}
