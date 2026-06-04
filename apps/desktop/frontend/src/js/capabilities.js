// Capabilities panel (right rail): per-flow network / fs / llm / mcp whitelist.

import { $, $$, html, toast } from "./dom.js";
import { call } from "./api.js";
import { state } from "./state.js";
import { CAP_KINDS } from "./constants.js";
import { parseYaml } from "./yaml.js";
import { renderActiveView } from "./editor/render.js";

export async function renderCapabilitiesPanel() {
  const body = $("capabilitiesBody");
  if (!state.flowPath) {
    body.innerHTML = `<div class="prop-empty">先选中一个流文件。</div>`;
    return;
  }
  let snap;
  try {
    snap = await call("get_flow_capabilities", { path: state.flowPath });
  } catch (e) {
    body.innerHTML = `<div class="prop-empty">读取失败: ${html(String(e))}</div>`;
    return;
  }
  const cards = CAP_KINDS.map((k) => {
    const list = snap[k.key === "fs.read" ? "fs.read" : k.key === "fs.write" ? "fs.write" : k.key] || [];
    const chips = list.length
      ? list
          .map(
            (g) =>
              `<span class="cap-chip">${html(g)}</span>`,
          )
          .join("")
      : `<span class="prop-empty" style="font-size:11px">未声明</span>`;
    return `
      <div class="prop-field" data-cap-kind="${html(k.key)}">
        <label>${html(k.label)}</label>
        <div class="cap-chip-row">${chips}</div>
        <div class="cap-add-row">
          <input type="text" class="cap-input" placeholder="${html(k.hint)}" />
          <button class="ghost cap-add-btn" data-kind="${html(k.key)}">+ 加白名单</button>
        </div>
      </div>`;
  }).join("");
  body.innerHTML = `
    <div class="prop-form">
      <div class="prop-field"><label>当前流</label><div style="font-size:11px;color:var(--muted)">${html(state.flowPath)}</div></div>
      ${cards}
    </div>`;
  $$("#capabilitiesBody .cap-add-btn").forEach((btn) => {
    btn.addEventListener("click", async () => {
      const field = btn.closest(".prop-field");
      const input = field?.querySelector(".cap-input");
      const grant = input?.value?.trim();
      const kind = btn.dataset.kind;
      if (!grant) {
        toast("权限", "请填写要添加的 grant", "warn");
        return;
      }
      try {
        const added = await call("add_capability_grant", { path: state.flowPath, kind, grant });
        toast("权限", added ? `已添加 ${kind} → ${grant}` : `${grant} 已存在,无需重复`, added ? "ok" : "warn");
        if (added) {
          // Re-read the YAML so the editor view stays in sync with disk.
          state.source = await call("read_flow_source", { path: state.flowPath });
          parseYaml(state.source);
          renderActiveView();
        }
        renderCapabilitiesPanel();
      } catch (e) {
        toast("权限", String(e), "bad");
      }
    });
  });
}
