// Action library panel: fetch, group by category, search filter, render.

import { $, html } from "./dom.js";
import { call } from "./api.js";
import { state } from "./state.js";
import { FAMILY_LABEL, FAVORITE_IDS, categoryOf, zhAction } from "./constants.js";

export async function refreshActions() {
  state.actions = await call("list_actions");
  state.actionsByFamily.clear();
  for (const a of state.actions) {
    if (!state.actionsByFamily.has(a.family)) state.actionsByFamily.set(a.family, []);
    state.actionsByFamily.get(a.family).push(a);
  }
  renderActions();
}

export function renderActions() {
  const query = ($("actionSearch").value || "").trim().toLowerCase();
  const box = $("actionLibrary");
  const order = ["browser","condition","loop","wait","excel","file","http","ai","skill","flow","data","string","regex","date","math","list","json","csv","hash","util","system","db","control","misc"];
  const byCategory = new Map();
  for (const a of state.actions) {
    const cat = categoryOf(a.id);
    if (!byCategory.has(cat)) byCategory.set(cat, []);
    byCategory.get(cat).push(a);
  }
  const matches = (a) => {
    if (!query) return true;
    const zh = zhAction(a.id);
    return (zh.label || "").toLowerCase().includes(query)
        || (zh.hint  || "").toLowerCase().includes(query)
        || a.id.toLowerCase().includes(query)
        || (a.summary || "").toLowerCase().includes(query);
  };
  const favs = FAVORITE_IDS.map((id) => state.actions.find((a) => a.id === id)).filter(Boolean).filter(matches);
  const sections = [];
  if (favs.length) sections.push(renderActionFamily("favorite", favs, false));
  for (const cat of order) {
    const items = (byCategory.get(cat) || []).filter(matches);
    if (!items.length) continue;
    sections.push(renderActionFamily(cat, items, cat !== "browser"));
  }
  box.innerHTML = sections.join("") || `<div class="prop-empty" style="padding:14px">未匹配到指令</div>`;
}

function renderActionFamily(family, items, collapsed) {
  return `<div class="action-family ${collapsed ? "is-collapsed" : ""}" data-family="${html(family)}">
    <div class="action-family-head"><span>${html(FAMILY_LABEL[family] || family)} · ${items.length}</span><span class="chev">▾</span></div>
    <div class="action-list">
      ${items
        .map((a) => {
          const zh = zhAction(a.id);
          return `<button class="action-item" draggable="true" data-action="${html(a.id)}" title="${html(a.id)} · ${html(a.summary || zh.hint || "")}">
            <div class="id"><span class="zh">${html(zh.label)}</span><span class="en">${html(a.id)}</span></div>
            <div class="meta">${html(zh.hint || a.summary || "")}</div>
          </button>`;
        })
        .join("")}
    </div>
  </div>`;
}
