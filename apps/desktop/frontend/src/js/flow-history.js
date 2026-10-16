// Flow save-history panel (批次 C): list the rolling snapshots the backend
// keeps for the current flow, preview a line diff against the editor buffer,
// and restore a snapshot (restore itself re-archives the current disk state,
// and the editor buffer swap goes through undo history).

import { $, html, toast } from "./dom.js";
import { call } from "./api.js";
import { state } from "./state.js";
import { parseYaml } from "./yaml.js";
import { diffLines } from "./diff.js";
import { renderActiveView } from "./editor/render.js";
import { renderInspector } from "./editor/inspector.js";
import { snapshotSource } from "./editor/history.js";

function closeFlowHistory() {
  const overlay = $("flowHistoryOverlay");
  overlay.hidden = true;
  overlay.innerHTML = "";
}

export async function openFlowHistory() {
  if (!state.flowPath) {
    toast("先选择一个流程", "历史版本按流程文件记录", "warn");
    return;
  }
  const overlay = $("flowHistoryOverlay");
  overlay.hidden = false;
  overlay.innerHTML = `
    <div class="flow-history-panel">
      <header class="flow-history-head">
        <div>
          <strong>🕘 保存历史</strong>
          <span class="flow-history-sub">${html(state.flowPath)}</span>
        </div>
        <button id="flowHistoryCloseBtn" class="ghost">✕ 关闭</button>
      </header>
      <div class="flow-history-body">
        <div class="flow-history-list" id="flowHistoryList">加载中…</div>
        <div class="flow-history-preview" id="flowHistoryPreview">
          <em class="flow-history-hint">左侧选择一个版本查看与当前编辑内容的差异</em>
        </div>
      </div>
    </div>`;
  overlay.onclick = (e) => {
    if (e.target === overlay) closeFlowHistory();
  };
  $("flowHistoryCloseBtn").addEventListener("click", closeFlowHistory);
  try {
    const entries = await call("flow_history_list", { path: state.flowPath });
    renderHistoryList(entries);
  } catch (e) {
    $("flowHistoryList").innerHTML = `<em class="flow-history-hint">加载失败: ${html(String(e))}</em>`;
  }
}

function renderHistoryList(entries) {
  const list = $("flowHistoryList");
  if (!entries.length) {
    list.innerHTML = `<em class="flow-history-hint">还没有历史版本 — 每次 💾 保存且内容有变化时会自动存档（保留最近 20 版）</em>`;
    return;
  }
  list.innerHTML = entries
    .map((entry) => {
      const when = new Date(entry.savedAtMs);
      const size = entry.bytes > 1024 ? `${(entry.bytes / 1024).toFixed(1)} KB` : `${entry.bytes} B`;
      return `<button class="flow-history-row" data-ms="${entry.savedAtMs}">
        <span>${html(when.toLocaleString())}</span><span class="flow-history-size">${size}</span>
      </button>`;
    })
    .join("");
  list.querySelectorAll(".flow-history-row").forEach((row) => {
    row.addEventListener("click", () => {
      list.querySelectorAll(".flow-history-row").forEach((r) => r.classList.remove("is-active"));
      row.classList.add("is-active");
      previewHistoryEntry(Number(row.dataset.ms)).catch((e) => toast("读取历史失败", String(e), "bad"));
    });
  });
}

async function previewHistoryEntry(savedAtMs) {
  const preview = $("flowHistoryPreview");
  preview.innerHTML = `<em class="flow-history-hint">载入中…</em>`;
  const snapshot = await call("flow_history_read", { path: state.flowPath, savedAtMs });
  // 差异方向 = 「回滚会发生什么」：add 行是快照独有（回滚后加回），del 行是当前独有（回滚后移除）。
  const { rows, added, removed } = diffLines(state.source || "", snapshot);
  const body = rows
    .map((r) => `<div class="diff-line diff-${r.type}">${html(r.text) || "&nbsp;"}</div>`)
    .join("");
  preview.innerHTML = `
    <div class="flow-history-preview-head">
      <span>回滚到此版本将 <strong class="diff-count-add">+${added}</strong> / <strong class="diff-count-del">−${removed}</strong> 行</span>
      <button class="primary" id="flowHistoryRestoreBtn">↩︎ 回滚到此版本</button>
    </div>
    <div class="flow-history-diff">${body}</div>`;
  $("flowHistoryRestoreBtn").addEventListener("click", async () => {
    if (!confirm(`回滚到 ${new Date(savedAtMs).toLocaleString()} 的版本？\n当前磁盘内容会先自动存档，可再次回滚。`)) return;
    try {
      const restored = await call("flow_history_restore", { path: state.flowPath, savedAtMs });
      snapshotSource("history-restore");
      state.source = restored;
      state.ast = parseYaml(state.source);
      renderActiveView();
      renderInspector();
      toast("已回滚", "编辑器与磁盘均已恢复到所选版本（可 Ctrl/Cmd+Z 撤销缓冲区）", "ok");
      closeFlowHistory();
    } catch (e) {
      toast("回滚失败", String(e), "bad");
    }
  });
}
