// Element / Image / DataTable library (YingDao parity). Captured elements carry
// CSS / XPath / A11y / Visual fingerprints for Self-Healing fallback.

import { $, $$, html } from "./dom.js";
import { state } from "./state.js";

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
    const filtered = els.filter((e) => !query
      || (e.label || "").toLowerCase().includes(query)
      || (e.source || "").toLowerCase().includes(query)
      || JSON.stringify(e.fingerprints || {}).toLowerCase().includes(query));
    if (!filtered.length) {
      body.innerHTML = `
        <div class="el-empty">
          <div style="font-size:30px;margin-bottom:8px">🎯</div>
          <div>暂无已捕获元素</div>
          <div style="margin-top:6px">点击右上 <strong>+ 捕获</strong> 跳到录制器，圈选页面元素即可生成<br>
            <em>CSS · XPath · A11y · Visual</em> 四套指纹，Self-Healing 时按优先级回退。</div>
        </div>`;
      return;
    }
    const groups = new Map();
    for (const el of filtered) {
      const k = el.source || "(未知来源)";
      if (!groups.has(k)) groups.set(k, []);
      groups.get(k).push(el);
    }
    body.innerHTML = [...groups.entries()].map(([src, items]) => `
      <div class="el-group">
        <div class="el-group-head">
          <span>📂 ${html(items.length)} 个元素</span>
          <span class="src" title="${html(src)}">${html(src)}</span>
        </div>
        <div class="el-grid">
          ${items.map((e) => renderElementCard(e)).join("")}
        </div>
      </div>
    `).join("");
  } else if (tab === "images") {
    if (!imgs.length) {
      body.innerHTML = `
        <div class="el-empty">
          <div style="font-size:30px;margin-bottom:8px">🖼</div>
          <div>暂无已捕获图像</div>
          <div style="margin-top:6px"><em>找图 (FindImage)</em> 在 LumoRPA 中由 phash + Vision-LLM 兜底增强，<br>
            录制器抓取的小图缓存在此，<strong>image.find</strong> 指令运行时按相似度匹配。</div>
        </div>`;
      return;
    }
    body.innerHTML = `<div class="el-grid">${imgs.map((i) => `
      <div class="el-card" draggable="true" data-image-id="${html(i.id)}">
        <div class="el-thumbnail">${i.thumbnail ? `<img src="${html(i.thumbnail)}">` : "📷 缩略图占位"}</div>
        <div class="el-title">${html(i.label || i.id)}<span class="badge">IMG</span></div>
        <div class="el-source" title="${html(i.source || "")}">${html(i.source || "")}</div>
        <div class="el-fingerprints">
          <div class="fp"><span class="k">phash</span><span class="v">${html(i.hash || "—")}</span></div>
          <div class="fp"><span class="k">at</span><span class="v">${html(i.capturedAt || "—")}</span></div>
        </div>
      </div>
    `).join("")}</div>`;
  } else if (tab === "datatables") {
    if (!tbls.length) {
      body.innerHTML = `
        <div class="el-empty">
          <div style="font-size:30px;margin-bottom:8px">📊</div>
          <div>暂无数据表格</div>
          <div style="margin-top:6px">在画布加上 <strong>excel.read_rows</strong> / <strong>browser.extract (all=true)</strong>，
            运行后表格结构会自动入库，<br>方便后续 <em>JSON Path</em> / <em>SQL-like</em> 二次处理。</div>
        </div>`;
      return;
    }
    body.innerHTML = `<div class="el-grid">${tbls.map((t) => `
      <div class="el-card"><div class="el-title">${html(t.label || t.id)}<span class="badge">TBL</span></div></div>
    `).join("")}</div>`;
  }
}

function renderElementCard(el) {
  const fp = el.fingerprints || {};
  return `
    <div class="el-card" draggable="true" data-element-id="${html(el.id)}">
      <div class="el-title">${html(el.label || el.id)}<span class="badge">${html((el.tag || "EL").toUpperCase())}</span></div>
      <div class="el-source" title="${html(el.source || "")}">${html(el.source || "")}</div>
      <div class="el-fingerprints">
        ${fp.css    ? `<div class="fp"><span class="k">CSS</span><span class="v" title="${html(fp.css)}">${html(fp.css)}</span></div>` : ""}
        ${fp.xpath  ? `<div class="fp"><span class="k">XPath</span><span class="v" title="${html(fp.xpath)}">${html(fp.xpath)}</span></div>` : ""}
        ${fp.a11y   ? `<div class="fp"><span class="k">A11y</span><span class="v" title="${html(fp.a11y)}">${html(fp.a11y)}</span></div>` : ""}
        ${fp.visual ? `<div class="fp"><span class="k">Visual</span><span class="v" title="${html(fp.visual)}">${html(fp.visual)}</span></div>` : ""}
      </div>
      <div class="el-actions">
        <button data-el-use-click="${html(el.id)}" title="作为 browser.click 的 selector">点击</button>
        <button data-el-use-extract="${html(el.id)}" title="作为 browser.extract 的 selector">抓取</button>
        <button data-el-copy="${html(el.id)}" title="复制 CSS selector">复制</button>
      </div>
    </div>`;
}

export function elementById(id) {
  return (state.elements || []).find((e) => e.id === id);
}
