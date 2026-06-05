// Element / Image / DataTable library. Captured elements carry stable selector
// fingerprints and management metadata for grouping, reuse and cloud sync.

import { $, $$, html } from "./dom.js";
import { call } from "./api.js";
import { state } from "./state.js";

const AUTOMATION_LABEL = {
  web: "网页",
  desktop: "桌面",
};

const SYNC_LABEL = {
  synced: "已同步",
  dirty: "待同步",
  local: "本地",
  readonly: "只读",
};

const STRATEGY_LABEL = {
  id: "ID",
  data_testid: "TestId",
  css: "CSS",
  aria_label: "Aria",
  text_includes: "Text",
  xpath: "XPath",
  a11y: "A11y",
  visual: "Visual",
};

const RUNTIME_SELECTOR_KEYS = ["id", "data_testid", "css", "aria_label", "text_includes", "xpath"];
const DISPLAY_SELECTOR_KEYS = [...RUNTIME_SELECTOR_KEYS, "a11y", "visual"];

export function setElTab(tab) {
  state.elTab = tab;
  $$("#elTabs button").forEach((b) => b.classList.toggle("is-active", b.dataset.elTab === tab));
  renderElementLibrary();
}

export function renderElementLibrary() {
  const els = state.elements || [];
  const imgs = state.images || [];
  const tbls = state.datatables || [];
  const countEl = $("elCountElements"); if (countEl) countEl.textContent = els.length;
  const countIm = $("elCountImages");   if (countIm) countIm.textContent = imgs.length;
  const countTb = $("elCountTables");   if (countTb) countTb.textContent = tbls.length;
  const body = $("elBody"); if (!body) return;
  const query = ($("elSearch")?.value || "").trim().toLowerCase();
  const tab = state.elTab || "elements";

  if (tab === "elements") {
    renderElements(body, els, query);
  } else if (tab === "images") {
    renderImages(body, imgs, query);
  } else if (tab === "datatables") {
    renderDataTables(body, tbls, query);
  }
}

export function elementById(id) {
  return (state.elements || []).find((e) => e.id === id);
}

export async function refreshElementLibrary() {
  const library = await call("load_element_library");
  applyElementLibrary(library);
  renderElementLibrary();
  return library;
}

export async function persistElementLibrary() {
  await call("save_element_library", { library: elementLibrarySnapshot() });
}

export function applyElementLibrary(library) {
  if (!library || typeof library !== "object") return;
  if (Array.isArray(library.elements)) state.elements = library.elements;
  if (Array.isArray(library.images)) state.images = library.images;
  if (Array.isArray(library.datatables)) state.datatables = library.datatables;
}

export function elementLibrarySnapshot() {
  return {
    version: 1,
    elements: state.elements || [],
    images: state.images || [],
    datatables: state.datatables || [],
  };
}

export async function deleteElementById(id) {
  const before = (state.elements || []).length;
  state.elements = (state.elements || []).filter((e) => e.id !== id);
  renderElementLibrary();
  await persistElementLibrary();
  return before - state.elements.length;
}

export async function duplicateElementById(id) {
  const source = elementById(id);
  if (!source) return null;
  const clone = JSON.parse(JSON.stringify(source));
  clone.id = `${source.id}_copy_${Math.floor(Math.random() * 1000)}`;
  clone.label = `${source.label || source.id} 副本`;
  clone.scope = "local";
  clone.syncState = "local";
  clone.usedIn = [];
  clone.cloudGroup = null;
  clone.owner = "本机";
  state.elements = [...(state.elements || []), clone];
  renderElementLibrary();
  await persistElementLibrary();
  return clone;
}

export async function syncElementGroups() {
  let count = 0;
  for (const el of state.elements || []) {
    if (el.scope === "cloud" && el.syncState === "dirty") {
      el.syncState = "synced";
      el.lastValidated = "刚刚";
      count += 1;
    }
  }
  for (const img of state.images || []) {
    if (img.scope === "cloud" && img.syncState === "dirty") {
      img.syncState = "synced";
      count += 1;
    }
  }
  renderElementLibrary();
  await persistElementLibrary();
  return count;
}

export function elementCopyText(id) {
  const el = elementById(id);
  if (!el) return "";
  const payload = selectorPayload(el);
  if (Object.keys(payload.selectors || {}).length) {
    return JSON.stringify(payload, null, 2);
  }
  if (el.bounds) {
    return JSON.stringify({ bounds: el.bounds, a11y: el.fingerprints?.a11y || "" }, null, 2);
  }
  return el.fingerprints?.css || el.fingerprints?.a11y || el.label || "";
}

export function selectorPayload(el) {
  const selectors = {};
  const fp = el?.fingerprints || {};
  for (const key of RUNTIME_SELECTOR_KEYS) {
    if (fp[key]) selectors[key] = fp[key];
  }
  return {
    selectors,
    prompt: elementPrompt(el),
  };
}

function renderElements(body, els, query) {
  const kind = $("elKindFilter")?.value || "all";
  const usage = $("elUsageFilter")?.value || "all";
  const filtered = els
    .filter((el) => matchesElementQuery(el, query))
    .filter((el) => matchesKind(el, kind))
    .filter((el) => matchesUsage(el, usage));

  if (!els.length) {
    body.innerHTML = emptyState("target", "暂无已捕获元素", "点击右上 + 捕获 跳到录制器，圈选页面元素后会生成多套稳定指纹。");
    return;
  }
  if (!filtered.length) {
    body.innerHTML = `
      ${renderElementOverview(els, filtered)}
      ${emptyState("target", "没有匹配的元素", "调整搜索词、范围或状态筛选后再试。")}
    `;
    return;
  }

  body.innerHTML = `
    ${renderElementOverview(els, filtered)}
    <div class="el-group-list">
      ${groupElements(filtered).map(([group, items]) => renderElementGroup(group, items)).join("")}
    </div>
  `;
}

function renderImages(body, imgs, query) {
  const filtered = imgs.filter((img) => matchesAssetQuery(img, query));
  if (!imgs.length) {
    body.innerHTML = emptyState("image", "暂无已捕获图像", "录制器抓取的小图会缓存在此，运行时可由 image.locate 定位后接 desktop.click。");
    return;
  }
  if (!filtered.length) {
    body.innerHTML = `
      ${renderAssetOverview(imgs, filtered, "图像")}
      ${emptyState("image", "没有匹配的图像", "调整搜索词后再试。")}
    `;
    return;
  }
  body.innerHTML = `
    ${renderAssetOverview(imgs, filtered, "图像")}
    <div class="el-grid">${filtered.map(renderImageCard).join("")}</div>
  `;
}

function renderDataTables(body, tbls, query) {
  const filtered = tbls.filter((tbl) => matchesAssetQuery(tbl, query));
  if (!tbls.length) {
    body.innerHTML = emptyState("table", "暂无数据表格", "运行 excel.read_rows 或 browser.extract(all=true) 后，表格结构会自动入库。");
    return;
  }
  if (!filtered.length) {
    body.innerHTML = `
      ${renderAssetOverview(tbls, filtered, "表格")}
      ${emptyState("table", "没有匹配的表格", "调整搜索词后再试。")}
    `;
    return;
  }
  body.innerHTML = `
    ${renderAssetOverview(tbls, filtered, "表格")}
    <div class="el-grid">${filtered.map(renderTableCard).join("")}</div>
  `;
}

function renderElementOverview(all, filtered) {
  const used = all.filter(isElementUsed).length;
  const cloud = all.filter((el) => el.scope === "cloud").length;
  const dirty = all.filter((el) => el.syncState === "dirty").length;
  const desktop = all.filter((el) => elementAutomation(el) === "desktop").length;
  return `
    <div class="el-overview">
      ${stat("当前", filtered.length)}
      ${stat("云元素", cloud)}
      ${stat("桌面", desktop)}
      ${stat("已使用", used)}
      ${stat("待同步", dirty, dirty ? "warn" : "")}
    </div>
  `;
}

function renderAssetOverview(all, filtered, label) {
  const used = all.filter((item) => (item.usedIn || []).length).length;
  const cloud = all.filter((item) => item.scope === "cloud").length;
  return `
    <div class="el-overview">
      ${stat(label, filtered.length)}
      ${stat("云端", cloud)}
      ${stat("已使用", used)}
    </div>
  `;
}

function stat(label, value, tone = "") {
  return `
    <div class="el-stat ${tone}">
      <span>${html(label)}</span>
      <strong>${html(value)}</strong>
    </div>
  `;
}

function groupElements(elements) {
  const groups = new Map();
  for (const el of elements) {
    const key = elementGroup(el);
    if (!groups.has(key)) groups.set(key, []);
    groups.get(key).push(el);
  }
  return [...groups.entries()].sort(([a], [b]) => a.localeCompare(b, "zh-Hans-CN"));
}

function renderElementGroup(group, items) {
  const cloud = items.filter((el) => el.scope === "cloud").length;
  const dirty = items.filter((el) => el.syncState === "dirty").length;
  const readonly = items.filter((el) => el.cloudGroup?.permission === "readonly").length;
  return `
    <section class="el-group">
      <div class="el-group-head">
        <div>
          <span class="el-folder">分组</span>
          <strong>${html(group)}</strong>
          <span>${html(items.length)} 个元素</span>
        </div>
        <div class="el-group-meta">
          ${cloud ? `<span>云 ${html(cloud)}</span>` : ""}
          ${dirty ? `<span class="warn">待同步 ${html(dirty)}</span>` : ""}
          ${readonly ? `<span>只读 ${html(readonly)}</span>` : ""}
        </div>
      </div>
      <div class="el-grid">
        ${items.map(renderElementCard).join("")}
      </div>
    </section>
  `;
}

function renderElementCard(el) {
  const used = isElementUsed(el);
  const automation = elementAutomation(el);
  const syncState = el.syncState || (el.scope === "cloud" ? "synced" : "local");
  const selectorCount = Object.keys(selectorPayload(el).selectors || {}).length;
  const similarCount = el.siblingCount || (el.similar || []).length;
  const permission = el.cloudGroup?.permission || "edit";
  const title = html(el.label || el.id);
  const source = html(el.source || "");
  return `
    <article class="el-card ${used ? "is-used" : "is-unused"} ${syncState === "dirty" ? "is-dirty" : ""}" draggable="true" data-element-id="${html(el.id)}">
      <div class="el-title">
        <span title="${title}">${title}</span>
        <span class="badge">${html(AUTOMATION_LABEL[automation] || automation)}</span>
        <span class="sync-badge ${html(syncState)}">${html(SYNC_LABEL[syncState] || syncState)}</span>
      </div>
      <div class="el-tags">
        <span>${html(el.tag || "element")}</span>
        ${el.role ? `<span>${html(el.role)}</span>` : ""}
        ${used ? `<span>已使用</span>` : `<span class="warn">未使用</span>`}
        ${similarCount ? `<span>相似 ${html(similarCount)}</span>` : ""}
        ${permission === "readonly" ? `<span>只读</span>` : ""}
      </div>
      <div class="el-source" title="${source}">${source}</div>
      <div class="el-mini-meta">
        <span title="${html(elementGroup(el))}">组: ${html(elementGroup(el))}</span>
        <span>策略: ${html(selectorCount || (el.bounds ? 1 : 0))}</span>
        <span>校验: ${html(el.lastValidated || "-")}</span>
      </div>
      <div class="el-fingerprints">
        ${renderFingerprintRows(el)}
      </div>
      <details class="el-detail">
        <summary>详情</summary>
        <pre>${html(JSON.stringify(detailPayload(el), null, 2))}</pre>
      </details>
      <div class="el-actions">
        ${renderElementButtons(el)}
      </div>
    </article>
  `;
}

function renderElementButtons(el) {
  const id = html(el.id);
  if (elementAutomation(el) === "desktop") {
    return `
      <button data-el-use="desktop.click" data-el-id="${id}" title="插入 desktop.click">点击</button>
      <button data-el-use="desktop.type" data-el-id="${id}" title="插入 desktop.type">输入</button>
      <button data-el-copy="${id}" title="复制桌面定位信息">复制</button>
      <button data-el-duplicate="${id}" title="复制成本地元素">副本</button>
      <button data-el-delete="${id}" title="删除元素">删除</button>
    `;
  }
  return `
    <button data-el-use="browser.click" data-el-id="${id}" title="插入 browser.click">点击</button>
    <button data-el-use="browser.type" data-el-id="${id}" title="插入 browser.type">输入</button>
    <button data-el-use="browser.wait" data-el-id="${id}" title="插入 browser.wait">等待</button>
    <button data-el-use="browser.extract" data-el-id="${id}" title="插入 browser.extract">抓取</button>
    <button data-el-copy="${id}" title="复制 selector payload">复制</button>
    <button data-el-duplicate="${id}" title="复制成本地元素">副本</button>
    <button data-el-delete="${id}" title="删除元素">删除</button>
  `;
}

function renderFingerprintRows(el) {
  const fp = el.fingerprints || {};
  const rows = [];
  for (const key of DISPLAY_SELECTOR_KEYS) {
    if (fp[key]) {
      rows.push(`<div class="fp"><span class="k">${html(STRATEGY_LABEL[key] || key)}</span><span class="v" title="${html(fp[key])}">${html(fp[key])}</span></div>`);
    }
  }
  if (el.bounds) {
    rows.push(`<div class="fp"><span class="k">Bounds</span><span class="v" title="${html(JSON.stringify(el.bounds))}">${html(boundsText(el.bounds))}</span></div>`);
  }
  return rows.join("") || `<div class="fp"><span class="k">-</span><span class="v">暂无指纹</span></div>`;
}

function renderImageCard(img) {
  const syncState = img.syncState || (img.scope === "cloud" ? "synced" : "local");
  return `
    <article class="el-card" draggable="true" data-image-id="${html(img.id)}">
      <div class="el-thumbnail">${img.thumbnail ? `<img src="${html(img.thumbnail)}" alt="">` : "缩略图占位"}</div>
      <div class="el-title">
        <span>${html(img.label || img.id)}</span>
        <span class="badge">IMG</span>
        <span class="sync-badge ${html(syncState)}">${html(SYNC_LABEL[syncState] || syncState)}</span>
      </div>
      <div class="el-tags">
        <span>${html(img.group || "未分组")}</span>
        ${(img.usedIn || []).length ? `<span>已使用</span>` : `<span class="warn">未使用</span>`}
      </div>
      <div class="el-source" title="${html(img.source || "")}">${html(img.source || "")}</div>
      <div class="el-fingerprints">
        <div class="fp"><span class="k">phash</span><span class="v">${html(img.hash || "-")}</span></div>
        <div class="fp"><span class="k">at</span><span class="v">${html(img.capturedAt || "-")}</span></div>
      </div>
    </article>
  `;
}

function renderTableCard(tbl) {
  const columns = tbl.columns || [];
  return `
    <article class="el-card">
      <div class="el-title"><span>${html(tbl.label || tbl.id)}</span><span class="badge">TBL</span></div>
      <div class="el-tags">
        <span>${html(tbl.group || "未分组")}</span>
        <span>${html(tbl.rows || 0)} 行</span>
        <span>${html(columns.length)} 列</span>
      </div>
      <div class="el-source" title="${html(tbl.source || "")}">${html(tbl.source || "")}</div>
      <div class="el-fingerprints">
        <div class="fp"><span class="k">Columns</span><span class="v" title="${html(columns.join(", "))}">${html(columns.join(", ") || "-")}</span></div>
        <div class="fp"><span class="k">at</span><span class="v">${html(tbl.capturedAt || "-")}</span></div>
      </div>
    </article>
  `;
}

function matchesElementQuery(el, query) {
  if (!query) return true;
  const fp = el.fingerprints || {};
  const hay = [
    el.id,
    el.label,
    el.group,
    el.source,
    el.tag,
    el.role,
    el.owner,
    el.cloudGroup?.name,
    el.syncState,
    elementAutomation(el),
    ...Object.values(fp),
  ].filter(Boolean).join(" ").toLowerCase();
  return hay.includes(query);
}

function matchesAssetQuery(item, query) {
  if (!query) return true;
  const hay = [
    item.id,
    item.label,
    item.group,
    item.source,
    item.hash,
    ...(item.columns || []),
  ].filter(Boolean).join(" ").toLowerCase();
  return hay.includes(query);
}

function matchesKind(el, kind) {
  if (kind === "all") return true;
  if (kind === "web" || kind === "desktop") return elementAutomation(el) === kind;
  if (kind === "cloud") return el.scope === "cloud";
  if (kind === "local") return el.scope !== "cloud";
  if (kind === "similar") return (el.similar || []).length > 0;
  return true;
}

function matchesUsage(el, usage) {
  if (usage === "all") return true;
  if (usage === "used") return isElementUsed(el);
  if (usage === "unused") return !isElementUsed(el);
  if (usage === "dirty") return el.syncState === "dirty";
  if (usage === "readonly") return el.cloudGroup?.permission === "readonly" || el.syncState === "readonly";
  return true;
}

function isElementUsed(el) {
  if ((el.usedIn || []).length) return true;
  const source = state.source || "";
  return elementReferenceTexts(el).some((text) => text && source.includes(text));
}

function elementReferenceTexts(el) {
  const fp = el.fingerprints || {};
  return [el.id, el.label, ...RUNTIME_SELECTOR_KEYS.map((key) => fp[key])].filter(Boolean);
}

function elementAutomation(el) {
  return el.automation || (el.bounds ? "desktop" : "web");
}

function elementGroup(el) {
  return el.cloudGroup?.name || el.group || el.source || "未分组元素";
}

function elementPrompt(el) {
  const parts = [el.label, el.role, el.tag, el.fingerprints?.visual].filter(Boolean);
  return parts.join(" / ");
}

function boundsText(bounds) {
  return `${bounds.x},${bounds.y} ${bounds.w}x${bounds.h}`;
}

function detailPayload(el) {
  return {
    id: el.id,
    label: el.label,
    automation: elementAutomation(el),
    scope: el.scope || "local",
    syncState: el.syncState || "local",
    group: elementGroup(el),
    source: el.source,
    usedIn: el.usedIn || [],
    selectors: selectorPayload(el).selectors,
    prompt: elementPrompt(el),
    bounds: el.bounds || null,
  };
}

function emptyState(icon, title, message) {
  const glyph = icon === "image" ? "IMG" : icon === "table" ? "TBL" : "EL";
  return `
    <div class="el-empty">
      <div class="el-empty-icon">${html(glyph)}</div>
      <div>${html(title)}</div>
      <div>${html(message)}</div>
    </div>
  `;
}
