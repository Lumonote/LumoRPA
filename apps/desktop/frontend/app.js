/* eslint-disable */
// LumoRPA Studio frontend — pure-JS, no bundler. Renders the 3-way isomorphic
// editor (Graph/Tree/Code), the provider CRUD panel, the recorder placeholder,
// the time-travel timeline and the feature-map. Talks to the Rust core through
// the Tauri command surface defined in apps/desktop/src-tauri/src/lib.rs.

const invoke = window.__TAURI__?.core?.invoke;

const $ = (id) => document.getElementById(id);
const $$ = (sel, root = document) => Array.from(root.querySelectorAll(sel));

const FAMILY_LABEL = {
  favorite: "⭐ 我收藏的指令",
  browser:  "网页自动化",
  condition:"条件判断",
  loop:     "循环",
  wait:     "等待",
  excel:    "Excel / 数据表格",
  file:     "文件 / 操作系统",
  http:     "网络 / API",
  ai:       "AI / 大模型",
  skill:    "自定义指令 (Skills)",
  flow:     "子流程",
  data:     "数据处理",
  control:  "控制流",
  string:   "字符串",
  regex:    "正则表达式",
  date:     "日期 / 时间",
  math:     "数学 / 计算",
  list:     "列表",
  json:     "JSON",
  csv:      "CSV",
  hash:     "哈希 / 加密",
  util:     "通用工具",
  system:   "系统",
  db:       "数据库",
  misc:     "其它",
};

const FAVORITE_IDS = [
  "browser.open", "browser.type", "browser.click", "browser.extract", "control.log",
];

const ACTION_ZH = {
  "browser.launch":   { label: "启动浏览器",       hint: "启动并连接 Chromium 会话" },
  "browser.open":     { label: "打开网页",         hint: "在浏览器中打开一个 URL" },
  "browser.click":    { label: "点击元素",         hint: "点击 CSS 选择器匹配到的元素" },
  "browser.type":     { label: "填写输入框",       hint: "在 CSS 选择器匹配的输入框中输入文本" },
  "browser.extract":  { label: "批量抓取数据",     hint: "提取 innerText / 属性 / 字段映射" },
  "browser.close":    { label: "关闭浏览器",       hint: "关闭当前浏览器会话" },

  "control.log":      { label: "打印日志",         hint: "向运行台输出一条日志" },
  "control.sleep":    { label: "等待",             hint: "睡眠指定毫秒数" },
  "control.set_var":  { label: "设置变量",         hint: "向当前运行上下文写入变量" },
  "control.fail":     { label: "中断流程",         hint: "主动抛错终止流程" },
  "control.if":       { label: "条件判断",         hint: "if / else 分支" },
  "control.for":      { label: "计数循环",         hint: "按次数循环执行" },
  "control.for_each": { label: "遍历循环",         hint: "对数组 / Range / 迭代器迭代" },
  "control.parallel": { label: "并行执行",         hint: "同时编排多个分支" },
  "control.try":      { label: "异常处理",         hint: "try / catch / finally" },

  "lumo.flow":        { label: "调用子流程",       hint: "调用另一个 LumoFlow YAML" },

  "data.json_format": { label: "JSON 转字符串",    hint: "将 JSON 值序列化为字符串" },
  "data.json_parse":  { label: "字符串转 JSON",    hint: "解析 JSON 文本到对象" },

  "excel.read_rows":  { label: "读取 Excel",       hint: "读取 .xlsx 表格的行" },
  "excel.write_row":  { label: "写入 Excel",       hint: "追加一行到 .xlsx 表格" },

  "file.read":        { label: "读取文件",         hint: "从本地路径读文件" },
  "file.write":       { label: "写入文件",         hint: "把数据写到本地路径" },
  "file.exists":      { label: "文件存在?",        hint: "判断路径是否存在" },

  "http.request":     { label: "HTTP 请求",        hint: "发起 GET / POST / PUT / DELETE 请求" },

  // ── 字符串 ──
  "string.upper":       { label: "字符串大写",       hint: "把字符串转为大写" },
  "string.lower":       { label: "字符串小写",       hint: "把字符串转为小写" },
  "string.trim":        { label: "去除空白",         hint: "去掉字符串首尾空白" },
  "string.length":      { label: "字符长度",         hint: "按字符计数（不按字节）" },
  "string.split":       { label: "字符串切分",       hint: "按分隔符切成数组" },
  "string.join":        { label: "数组拼接成字符串", hint: "用分隔符把数组连成字符串" },
  "string.replace":     { label: "字符串替换",       hint: "把 from 替换为 to（字面量）" },
  "string.contains":    { label: "包含子串?",        hint: "判断字符串是否包含子串" },
  "string.starts_with": { label: "以…开头?",         hint: "判断是否以前缀开头" },
  "string.ends_with":   { label: "以…结尾?",         hint: "判断是否以后缀结尾" },
  "string.substring":   { label: "截取子串",         hint: "按字符位置切片，支持负数" },
  "string.repeat":      { label: "重复字符串",       hint: "把字符串重复 N 次" },
  "string.pad_left":    { label: "左侧补齐",         hint: "把字符串补齐到指定宽度" },
  "string.pad_right":   { label: "右侧补齐",         hint: "把字符串补齐到指定宽度" },
  "string.format":      { label: "模板替换",         hint: "替换 {key} 占位符" },

  // ── 正则 ──
  "regex.match":        { label: "正则匹配?",        hint: "判断文本是否匹配正则" },
  "regex.find_all":     { label: "正则查找全部",     hint: "返回所有匹配的字符串数组" },
  "regex.replace":      { label: "正则替换",         hint: "支持 $1 $2 反向引用" },
  "regex.captures":     { label: "正则捕获组",       hint: "返回第一个匹配的命名/编号分组" },

  // ── 日期 ──
  "date.now":           { label: "当前时间",         hint: "返回 RFC3339 时间字符串" },
  "date.parse":         { label: "解析时间",         hint: "把任意日期字符串规范成 RFC3339" },
  "date.format":        { label: "格式化时间",       hint: "按 strftime 格式输出" },
  "date.add":           { label: "时间偏移",         hint: "按天/小时/分/秒加减" },
  "date.diff":          { label: "时间差",           hint: "返回 a-b 的差值（天/时/分/秒）" },
  "date.weekday":       { label: "星期几",           hint: "返回 1=周一 … 7=周日" },

  // ── 数学 ──
  "math.round":         { label: "四舍五入",         hint: "保留指定位小数" },
  "math.random":        { label: "随机数",           hint: "范围内随机数（整/浮点）" },
  "math.min":           { label: "最小值",           hint: "数组中最小数" },
  "math.max":           { label: "最大值",           hint: "数组中最大数" },
  "math.sum":           { label: "求和",             hint: "对数组求和" },
  "math.avg":           { label: "平均值",           hint: "对数组求算术平均" },
  "math.abs":           { label: "绝对值",           hint: "取数字的绝对值" },

  // ── 列表 ──
  "list.length":        { label: "列表长度",         hint: "返回数组长度" },
  "list.append":        { label: "追加元素",         hint: "在数组末尾追加一项" },
  "list.sort":          { label: "排序",             hint: "升/降序，支持 by 字段" },
  "list.unique":        { label: "去重",             hint: "保留出现顺序的去重" },
  "list.range":         { label: "生成区间",         hint: "[start, end) 整数数组" },
  "list.contains":      { label: "包含某值?",        hint: "数组是否包含某个值" },
  "list.get":           { label: "按索引取值",       hint: "支持负数索引" },
  "list.slice":         { label: "切片",             hint: "数组切片 [start:end]" },
  "list.reverse":       { label: "倒序",             hint: "倒序排列数组" },
  "list.pluck":         { label: "抽取字段",         hint: "从对象数组中取出某字段" },

  // ── JSON ──
  "json.get":           { label: "按路径取值",       hint: "形如 a.b.0.c 的点号路径" },
  "json.set":           { label: "按路径写值",       hint: "在 JSON 中按点号路径写入" },
  "json.merge":         { label: "对象合并",         hint: "浅合并 a + b，b 优先" },
  "json.keys":          { label: "对象键名",         hint: "返回对象的所有键" },
  "json.values":        { label: "对象值",           hint: "返回对象的所有值" },
  "json.delete":        { label: "按路径删除",       hint: "删除 JSON 中的某个字段" },

  // ── CSV ──
  "csv.parse":          { label: "解析 CSV",         hint: "把 CSV 文本转成数组/对象" },
  "csv.stringify":      { label: "生成 CSV",         hint: "把数组转成 CSV 文本" },
  "csv.read":           { label: "读取 CSV 文件",    hint: "从磁盘读 CSV 并解析" },
  "csv.write":          { label: "写出 CSV 文件",    hint: "把数据写成 CSV 文件" },

  // ── 哈希 / 编码 ──
  "hash.sha256":        { label: "SHA-256",          hint: "SHA-256 十六进制" },
  "hash.sha512":        { label: "SHA-512",          hint: "SHA-512 十六进制" },
  "hash.sha1":          { label: "SHA-1（旧）",      hint: "SHA-1 十六进制" },
  "hash.md5":           { label: "MD5（旧）",        hint: "MD5 十六进制" },
  "util.base64_encode": { label: "Base64 编码",      hint: "把字符串编码为 Base64" },
  "util.base64_decode": { label: "Base64 解码",      hint: "把 Base64 解码为字符串" },
  "util.uuid":          { label: "UUID 生成",        hint: "生成随机 UUID v4" },

  // ── 系统 ──
  "system.shell":       { label: "运行 shell",       hint: "需要 LUMO_ALLOW_SHELL=1" },
  "system.env_get":     { label: "读取环境变量",     hint: "按名字读 env" },
  "system.sleep":       { label: "睡眠",             hint: "等待 N 毫秒" },
  "system.platform":    { label: "系统信息",         hint: "返回 OS / arch" },

  // ── 数据库 ──
  "db.sqlite_query":    { label: "SQLite 查询",      hint: "只读 SELECT，返回行" },
  "db.sqlite_exec":     { label: "SQLite 写入",      hint: "执行 INSERT/UPDATE/DDL" },
};

function categoryOf(actionId) {
  if (actionId.startsWith("browser."))                     return "browser";
  if (actionId === "control.if")                           return "condition";
  if (actionId === "control.for" || actionId === "control.for_each") return "loop";
  if (actionId === "control.sleep")                        return "wait";
  if (actionId.startsWith("excel."))                       return "excel";
  if (actionId.startsWith("file."))                        return "file";
  if (actionId.startsWith("http."))                        return "http";
  if (actionId.startsWith("ai."))                          return "ai";
  if (actionId.startsWith("skill."))                       return "skill";
  if (actionId === "lumo.flow")                            return "flow";
  if (actionId.startsWith("string."))                      return "string";
  if (actionId.startsWith("regex."))                       return "regex";
  if (actionId.startsWith("date."))                        return "date";
  if (actionId.startsWith("math."))                        return "math";
  if (actionId.startsWith("list."))                        return "list";
  if (actionId.startsWith("json."))                        return "json";
  if (actionId.startsWith("csv."))                         return "csv";
  if (actionId.startsWith("hash."))                        return "hash";
  if (actionId.startsWith("util."))                        return "util";
  if (actionId.startsWith("system."))                      return "system";
  if (actionId.startsWith("db."))                          return "db";
  if (actionId.startsWith("data.") || actionId === "control.set_var") return "data";
  if (actionId.startsWith("control."))                     return "control";
  return "misc";
}

function zhAction(actionId) {
  return ACTION_ZH[actionId] || { label: actionId || "(未指定)", hint: "" };
}

const PRESETS = {
  glass: { window: 8, panel: 50 },
  frost: { window: 18, panel: 62 },
  solid: { window: 96, panel: 96 },
  invisible: { window: 0, panel: 36 },
};

const state = {
  app: null,
  examples: [],
  actions: [],
  actionsByFamily: new Map(),
  flowPath: "",
  flow: null,            // FlowSummary
  source: "",            // raw YAML
  ast: null,             // parsed AST of source
  selectedStepId: null,
  selectedStepPath: null, // index path like [0, "do", 2]
  runs: [],
  activeRun: null,
  activeRunSteps: [],
  activeStepRun: null,
  activeArtifacts: [],        // X-07: blob artifacts for the active run
  artifactBlobCache: new Map(), // artifactId -> data URL (lazy-loaded)
  providers: null,
  providerDraft: null,
  features: [],
  viewMode: "steps",
  currentView: "design",
  rightSection: "inspector",
  windowAlpha: Number(localStorage.getItem("lumo.win") || 18),
  panelAlpha: Number(localStorage.getItem("lumo.panel") || 62),
  theme: localStorage.getItem("lumo.theme") || "auto",
  recorder: { recording: false, target: null, startedAt: null },
  schemaCache: new Map(),
  elTab: "elements",
  elements: [
    {
      id: "el_login_username",
      label: "用户名输入框",
      source: "https://example.com/login",
      tag: "input",
      fingerprints: {
        css: "form.login input[name='username']",
        xpath: "//form[@class='login']//input[@name='username']",
        a11y: "TextField[name='Username']",
        visual: "anchor:topRight(login-card)",
      },
    },
    {
      id: "el_login_submit",
      label: "登录按钮",
      source: "https://example.com/login",
      tag: "button",
      fingerprints: {
        css: "form.login button[type='submit']",
        xpath: "//form[@class='login']//button[@type='submit']",
        a11y: "Button[name='登录']",
        visual: "anchor:bottomRight(login-card)",
      },
    },
    {
      id: "el_h1",
      label: "页面主标题 H1",
      source: "https://example.com",
      tag: "h1",
      fingerprints: {
        css: "main h1",
        xpath: "//main//h1[1]",
        a11y: "Heading[level=1]",
        visual: "anchor:topCenter",
      },
    },
  ],
  images: [
    {
      id: "img_login_card",
      label: "登录卡片截图",
      source: "https://example.com/login",
      capturedAt: "2025-12-04 14:32",
      thumbnail: null,
      hash: "phash:8b3f2c9a…",
    },
  ],
  datatables: [],
};

// ─── Tauri helpers ─────────────────────────────────────────────────────────

async function call(cmd, args = {}) {
  if (!invoke) throw new Error("Tauri API unavailable (run via `cargo tauri dev`)");
  return invoke(cmd, args);
}

function html(value) {
  return String(value ?? "")
    .replaceAll("&", "&amp;")
    .replaceAll("<", "&lt;")
    .replaceAll(">", "&gt;")
    .replaceAll('"', "&quot;");
}

function pretty(value) {
  if (value === undefined || value === null || value === "") return "";
  try {
    return JSON.stringify(value, null, 2);
  } catch {
    return String(value);
  }
}

function toast(title, body = "", kind = "ok") {
  const stack = $("toastStack");
  const node = document.createElement("div");
  node.className = `toast ${kind}`;
  node.innerHTML = `<div class="toast-title">${html(title)}</div>${body ? `<div class="toast-body">${html(body)}</div>` : ""}`;
  stack.appendChild(node);
  setTimeout(() => {
    node.style.opacity = "0";
    node.style.transition = "opacity 260ms";
    setTimeout(() => node.remove(), 280);
  }, kind === "bad" ? 5500 : 3200);
}

function setStatus(text, tone = "ok") {
  const t = $("statusText");
  t.textContent = text;
  t.style.color = tone === "bad" ? "var(--bad)" : tone === "warn" ? "var(--warn)" : "var(--muted)";
}

// ─── Theme + opacity ───────────────────────────────────────────────────────

function applyTheme(value) {
  state.theme = value;
  localStorage.setItem("lumo.theme", value);
  const root = document.documentElement;
  const dark =
    value === "dark" ||
    (value === "auto" && window.matchMedia("(prefers-color-scheme: dark)").matches);
  root.classList.toggle("theme-dark", dark);
  $$("[data-theme]").forEach((b) => b.classList.toggle("is-active", b.dataset.theme === value));
}

function applyPanelAlpha(percent) {
  const clamped = Math.max(20, Math.min(100, Number(percent)));
  state.panelAlpha = clamped;
  localStorage.setItem("lumo.panel", String(clamped));
  document.documentElement.style.setProperty("--panel-alpha", String(clamped / 100));
  ["panelAlphaTop", "panelAlphaSlider"].forEach((id) => {
    const el = $(id);
    if (el && Number(el.value) !== clamped) el.value = String(clamped);
  });
  const lbl = $("panelAlphaValue");
  if (lbl) lbl.textContent = `${clamped}%`;
}

function applyWindowAlpha(percent) {
  const clamped = Math.max(0, Math.min(100, Number(percent)));
  state.windowAlpha = clamped;
  localStorage.setItem("lumo.win", String(clamped));
  document.documentElement.style.setProperty("--window-alpha", String(clamped / 100));
  ["windowAlphaTop", "windowAlphaSlider"].forEach((id) => {
    const el = $(id);
    if (el && Number(el.value) !== clamped) el.value = String(clamped);
  });
  const lbl = $("windowAlphaValue");
  if (lbl) lbl.textContent = `${clamped}%`;
  // Drive the actual Tauri window background alpha (0..=255) so the OS-level
  // vibrancy is exposed to whatever extent the user wants.
  const alpha = Math.round((clamped / 100) * 255);
  call("set_window_alpha", { options: { alpha } }).catch(() => {});
}

function applyPreset(name) {
  const preset = PRESETS[name];
  if (!preset) return;
  applyWindowAlpha(preset.window);
  applyPanelAlpha(preset.panel);
  $$(".preset-row [data-preset]").forEach((b) => b.classList.toggle("is-active", b.dataset.preset === name));
}

// ─── YAML mini parser ──────────────────────────────────────────────────────
// We only need a "good-enough" parser for LumoFlow YAML so the Graph/Tree
// views can render step structure. The Code view remains the source of truth.

function parseYaml(text) {
  const lines = text.split(/\r?\n/);
  const root = {};
  const stack = [{ indent: -1, value: root, kind: "map" }];
  let i = 0;

  function peek() { return lines[i]; }
  function isBlank(line) { return /^\s*(#|$)/.test(line); }
  function indentOf(line) {
    const m = line.match(/^( *)/);
    return m ? m[1].length : 0;
  }

  while (i < lines.length) {
    const raw = lines[i];
    if (isBlank(raw)) { i++; continue; }
    const ind = indentOf(raw);
    const line = raw.slice(ind);

    while (stack.length > 1 && ind <= stack[stack.length - 1].indent) {
      stack.pop();
    }
    const top = stack[stack.length - 1];

    if (line.startsWith("- ") || line === "-") {
      // List entry. Top must be a list.
      if (top.kind !== "list") {
        // Promote: parent key should hold a list. The previous map entry that
        // pointed here was treated as scalar; rewrite to list.
        // (This happens when key: appears with the list children on next lines.)
        // We assume top is now expecting a list.
        top.kind = "list";
        top.value = [];
        // Re-attach to parent key
        const parent = stack[stack.length - 2];
        if (parent && parent.lastKey) {
          if (parent.kind === "map") parent.value[parent.lastKey] = top.value;
        }
      }
      const after = line.slice(2);
      if (after === "" || /^\s*$/.test(after)) {
        // Empty item: object whose keys come at deeper indent
        const obj = {};
        top.value.push(obj);
        stack.push({ indent: ind, value: obj, kind: "map", lastKey: null });
        i++;
        continue;
      }
      if (after.includes(":") && !after.startsWith("{")) {
        // Inline first key like "- id: foo"
        const obj = {};
        top.value.push(obj);
        const child = { indent: ind, value: obj, kind: "map", lastKey: null };
        stack.push(child);
        // Re-process current line as a map entry at +2 indent
        const fakeLine = " ".repeat(ind + 2) + after;
        lines[i] = fakeLine;
        continue;
      }
      // Scalar list item
      top.value.push(parseScalar(after));
      i++;
      continue;
    }

    // map entry
    const colon = line.indexOf(":");
    if (colon < 0) { i++; continue; }
    const key = line.slice(0, colon).trim();
    const restRaw = line.slice(colon + 1);
    const rest = restRaw.replace(/\s+#.*$/, "").trim();

    if (top.kind !== "map") {
      // promote (rare); skip safety.
      i++; continue;
    }

    if (rest === "" || rest === "|" || rest === ">") {
      if (rest === "|" || rest === ">") {
        // Block scalar — collect until indent <= current.
        const blockLines = [];
        i++;
        const blockIndent = ind + 2;
        while (i < lines.length) {
          const bRaw = lines[i];
          if (bRaw.trim() === "") { blockLines.push(""); i++; continue; }
          if (indentOf(bRaw) < blockIndent) break;
          blockLines.push(bRaw.slice(blockIndent));
          i++;
        }
        top.value[key] = rest === "|" ? blockLines.join("\n") : blockLines.join(" ").trim();
        top.lastKey = key;
        continue;
      }
      // Empty: child container coming.
      const placeholder = {};
      top.value[key] = placeholder;
      top.lastKey = key;
      stack.push({ indent: ind, value: placeholder, kind: "map", lastKey: null });
      i++;
      continue;
    }
    // Inline scalar / flow-style
    top.value[key] = parseScalar(rest);
    top.lastKey = key;
    i++;
  }

  return root;
}

function parseScalar(s) {
  s = s.trim();
  if (s === "") return null;
  // Flow-style inline list
  if (s.startsWith("[") && s.endsWith("]")) {
    const inner = s.slice(1, -1);
    if (!inner.trim()) return [];
    return splitFlow(inner).map((p) => parseScalar(p));
  }
  if (s.startsWith("{") && s.endsWith("}")) {
    const inner = s.slice(1, -1);
    const obj = {};
    for (const part of splitFlow(inner)) {
      const idx = part.indexOf(":");
      if (idx > 0) {
        const k = part.slice(0, idx).trim();
        const v = part.slice(idx + 1).trim();
        obj[k] = parseScalar(v);
      }
    }
    return obj;
  }
  // Strings
  if ((s.startsWith('"') && s.endsWith('"')) || (s.startsWith("'") && s.endsWith("'"))) {
    return s.slice(1, -1);
  }
  if (s === "null" || s === "~") return null;
  if (s === "true") return true;
  if (s === "false") return false;
  if (/^-?\d+$/.test(s)) return Number(s);
  if (/^-?\d+\.\d+$/.test(s)) return Number(s);
  return s;
}

function splitFlow(s) {
  const out = [];
  let buf = "";
  let depth = 0;
  let inStr = null;
  for (const ch of s) {
    if (inStr) {
      buf += ch;
      if (ch === inStr) inStr = null;
      continue;
    }
    if (ch === '"' || ch === "'") { inStr = ch; buf += ch; continue; }
    if (ch === "[" || ch === "{") { depth++; buf += ch; continue; }
    if (ch === "]" || ch === "}") { depth--; buf += ch; continue; }
    if (ch === "," && depth === 0) { out.push(buf.trim()); buf = ""; continue; }
    buf += ch;
  }
  if (buf.trim()) out.push(buf.trim());
  return out;
}

function extractSteps(ast) {
  return ast?.spec?.steps || [];
}

function walkSteps(steps, cb, parent = null, depth = 0) {
  steps.forEach((step, idx) => {
    cb(step, { parent, idx, depth, path: parent ? [...parent, idx] : [idx] });
    ["do", "else", "catch", "finally"].forEach((kind) => {
      if (Array.isArray(step[kind])) {
        walkSteps(step[kind], cb, parent ? [...parent, idx, kind] : [idx, kind], depth + 1);
      }
    });
  });
}

function findStepByPath(steps, path) {
  let cur = steps;
  let parent = null;
  for (let i = 0; i < path.length; i++) {
    const seg = path[i];
    if (typeof seg === "number") {
      parent = cur[seg];
      if (i === path.length - 1) return parent;
      cur = parent;
    } else {
      cur = parent[seg] || [];
    }
  }
  return parent;
}

// ─── View tabs ─────────────────────────────────────────────────────────────

function switchTopView(view) {
  state.currentView = view;
  $$(".tabs .tab").forEach((b) => b.classList.toggle("is-active", b.dataset.view === view));
  const isDesign = view === "design";
  $("designView").style.display = isDesign ? "" : "none";
  $("rightRail").style.display = isDesign ? "" : "none";
  // Left rail visible for design / recorder (because recorder also uses flow context)
  document.querySelector(".left-rail").style.display = isDesign ? "" : "none";

  const map = {
    recorder: "recorderView",
    runs: "runsView",
    models: "modelsView",
    features: "featuresView",
    settings: "settingsView",
  };
  Object.values(map).forEach((id) => ($(id).style.display = "none"));
  if (map[view]) $(map[view]).style.display = "";

  if (view === "runs") refreshRuns().catch(reportError);
  if (view === "models") refreshProviders().catch(reportError);
  if (view === "features") renderFeatures();
  if (view === "settings") refreshSettings().catch(reportError);
  if (view === "recorder") refreshRecorder().catch(() => {});
}

function switchEditorMode(mode) {
  state.viewMode = mode;
  $$("#viewSwitch button").forEach((b) => b.classList.toggle("is-active", b.dataset.mode === mode));
  $$(".editor-body .view").forEach((v) => v.classList.toggle("is-active", v.id === `view-${mode}`));
  if (mode === "steps") renderStepList();
  if (mode === "graph") renderGraph();
  if (mode === "tree") renderTree();
  if (mode === "code") renderCode();
}

function switchRightSection(name) {
  state.rightSection = name;
  $$("#rightTabs button").forEach((b) => b.classList.toggle("is-active", b.dataset.section === name));
  ["rsInspector", "rsInputs", "rsOutputs", "rsCapabilities"].forEach((id) => {
    const target = id === `rs${name.charAt(0).toUpperCase()}${name.slice(1)}`;
    $(id).classList.toggle("is-active", target);
  });
  if (name === "capabilities") {
    renderCapabilitiesPanel().catch((e) => console.error("renderCapabilitiesPanel:", e));
  }
}

const CAP_KINDS = [
  { key: "network", label: "Network", hint: "host glob, e.g. api.github.com or *.example.com" },
  { key: "fs.read", label: "fs.read", hint: "path or glob, e.g. ./inbox/* or ${HOME}/data/**" },
  { key: "fs.write", label: "fs.write", hint: "path or glob" },
  { key: "llm", label: "llm", hint: "provider/model or `*`" },
  { key: "mcp", label: "mcp", hint: "server, server:tool, or server:tool_*" },
];

async function renderCapabilitiesPanel() {
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
        toast("权限", String(e), "error");
      }
    });
  });
}

// ─── Flow list + Actions library ───────────────────────────────────────────

async function refreshFlows() {
  state.examples = await call("list_flow_library");
  renderFlowList();
}

const BLANK_FLOW_TEMPLATE = `apiVersion: lumorpa.io/v1
kind: Flow
metadata:
  id: NAME
  version: 0.1.0
  name: 新流程
spec:
  capabilities:
    network: []
  steps:
    - id: hello
      action: control.log
      with: { message: "hello {{ inputs.name | default('world') }}" }
`;

async function createNewFlow() {
  const proposed = `flow-${new Date().toISOString().slice(0, 10)}`;
  const name = prompt("流程文件名（不带扩展名）：", proposed);
  if (!name) return;
  const yaml = BLANK_FLOW_TEMPLATE.replace("id: NAME", `id: ${name.replace(/[^a-zA-Z0-9_-]/g, "-")}`);
  try {
    const path = await call("save_flow_as", { name, source: yaml });
    await refreshFlows();
    await loadFlow(path);
    toast("已创建", path, "ok");
  } catch (e) {
    toast("新建失败", String(e), "bad");
  }
}

async function saveCurrentFlowAs() {
  const source = (state.source || "").trim();
  if (!source) {
    toast("当前编辑器为空", "先输入流程内容再另存为", "bad");
    return;
  }
  const seed = (state.flowPath || "").split("/").pop()?.replace(/\.lumoflow\.ya?ml$/, "") || "flow-copy";
  const name = prompt("另存为流程名（不带扩展名）：", seed);
  if (!name) return;
  try {
    const path = await call("save_flow_as", { name, source });
    await refreshFlows();
    await loadFlow(path);
    toast("已另存", path, "ok");
  } catch (e) {
    toast("另存失败", String(e), "bad");
  }
}

function renderFlowList() {
  const box = $("flowList");
  if (!state.examples.length) {
    box.innerHTML = `<div class="flow-item"><div class="title">空流程库</div><div class="meta">点击「新建流程」开始，或通过录制保存流程</div></div>`;
    return;
  }
  // Group by source: user-saved → recordings → examples.
  const groups = { user: [], recording: [], example: [] };
  for (const f of state.examples) {
    (groups[f.source] || groups.example).push(f);
  }
  const SECTION_LABELS = {
    user:      { label: "我的流程",   collapsed: false },
    recording: { label: "录制产物",   collapsed: false },
    example:   { label: "内置示例",   collapsed: true  },
  };
  const renderSection = (kind, items) => {
    if (!items.length) return "";
    const cfg = SECTION_LABELS[kind];
    const folded = state.flowSectionFolded?.[kind] ?? cfg.collapsed;
    const rows = items
      .map((f) => `
        <div class="flow-row" data-source="${kind}">
          <button class="flow-item ${f.path === state.flowPath ? "is-active" : ""}" data-path="${html(f.path)}">
            <div class="title">${html(f.name || f.id || f.fileName)}</div>
            <div class="meta">${html(f.valid ? `${f.stepCount} 步 · ${f.fileName}` : (f.error || "解析失败"))}</div>
          </button>
          <div class="flow-row-actions">
            <button class="icon-btn" data-act="dup"  data-path="${html(f.path)}" title="复制到我的流程">⎘</button>
            ${kind !== "example" ? `<button class="icon-btn danger" data-act="del" data-path="${html(f.path)}" title="删除">✕</button>` : ""}
          </div>
        </div>`)
      .join("");
    return `
      <div class="flow-section ${folded ? "is-folded" : ""}" data-section="${kind}">
        <div class="flow-section-head" data-toggle="${kind}">
          <span>${cfg.label} · ${items.length}</span>
          <span class="chev">${folded ? "▸" : "▾"}</span>
        </div>
        <div class="flow-section-body">${rows}</div>
      </div>`;
  };
  box.innerHTML =
    renderSection("user", groups.user)
    + renderSection("recording", groups.recording)
    + renderSection("example", groups.example);
}

async function refreshActions() {
  state.actions = await call("list_actions");
  state.actionsByFamily.clear();
  for (const a of state.actions) {
    if (!state.actionsByFamily.has(a.family)) state.actionsByFamily.set(a.family, []);
    state.actionsByFamily.get(a.family).push(a);
  }
  renderActions();
}

function renderActions() {
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

// ─── Element / Image library (YingDao parity) ─────────────────────────────

function setElTab(tab) {
  state.elTab = tab;
  $$("#elTabs button").forEach((b) => b.classList.toggle("is-active", b.dataset.elTab === tab));
  renderElementLibrary();
}

function renderElementLibrary() {
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
          <div style="margin-top:6px">影刀的 <em>找图 (FindImage)</em> 在 LumoRPA 中由 phash + Vision-LLM 兜底替代，<br>
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

function elementById(id) {
  return (state.elements || []).find((e) => e.id === id);
}

// ─── Flow load / save ──────────────────────────────────────────────────────

async function loadFlow(path = $("flowPath").value.trim()) {
  if (!path) return;
  state.flowPath = path;
  $("flowPath").value = path;
  setStatus("载入中…", "warn");
  try {
    const [flow, source] = await Promise.all([
      call("inspect_flow", { path }),
      call("read_flow_source", { path }).catch((e) => `# ${e}`),
    ]);
    state.flow = flow;
    state.source = source;
    state.ast = parseYaml(source);
    state.selectedStepId = null;
    state.selectedStepPath = null;
    $("flowTitle").textContent = flow.name || flow.id || flow.fileName || "未选择流程";
    $("flowSubtitle").textContent = `${flow.path}  ·  ${flow.stepCount} 步  ·  ${flow.valid ? "校验通过" : "校验异常: " + flow.error}`;
    if (flow.valid) {
      $("inputsJson").value = pretty(defaultInputs(flow));
    }
    renderFlowList();
    renderActiveView();
    renderInspector();
    setStatus(flow.valid ? "已载入" : "校验异常", flow.valid ? "ok" : "bad");
  } catch (error) {
    reportError(error);
  }
}

function defaultInputs(flow) {
  const out = {};
  for (const input of flow.inputs || []) {
    if (input.default !== undefined && input.default !== null) out[input.name] = input.default;
  }
  return out;
}

async function saveFlowSource() {
  if (!state.flowPath) return;
  await call("save_flow_source", { path: state.flowPath, source: state.source });
  state.ast = parseYaml(state.source);
  toast("已保存", state.flowPath, "ok");
  await loadFlow(state.flowPath);
}

// ─── Render dispatch ───────────────────────────────────────────────────────

function renderActiveView() {
  if (state.viewMode === "steps") renderStepList();
  else if (state.viewMode === "graph") renderGraph();
  else if (state.viewMode === "tree") renderTree();
  else renderCode();
}

// ─── Graph view ────────────────────────────────────────────────────────────

const graph = {
  scale: 1,
  tx: 24,
  ty: 24,
};

function renderGraph() {
  const svg = $("graphSvg");
  const root = $("graphRoot");
  root.innerHTML = "";
  if (!state.ast) {
    return;
  }
  const steps = extractSteps(state.ast);
  if (!steps.length) {
    return;
  }
  // Layout: simple vertical column at depth 0, children expand to the right.
  const NODE_W = 200;
  const NODE_H_HEAD = 60;
  const GAP_Y = 28;
  const CHILD_X = 240;
  const positions = new Map();
  let curY = 0;
  function layoutList(list, x, parentPath) {
    let maxY = curY;
    list.forEach((step, idx) => {
      const path = parentPath ? [...parentPath, idx] : [idx];
      const myY = curY;
      const id = pathKey(path);
      const childKinds = ["do", "else", "catch", "finally"].filter((k) => Array.isArray(step[k]) && step[k].length);
      const myH = NODE_H_HEAD + (childKinds.length ? 14 : 0);
      positions.set(id, { x, y: myY, w: NODE_W, h: myH, step, path });
      curY += myH + GAP_Y;
      childKinds.forEach((kind) => {
        const childPath = [...path, kind];
        const startY = curY;
        layoutList(step[kind], x + CHILD_X, childPath);
        // Mark the kind block top so we can draw a label later if needed.
      });
      maxY = curY;
    });
    return maxY;
  }
  layoutList(steps, 0, null);

  // Determine viewbox.
  const items = [...positions.values()];
  const maxX = Math.max(...items.map((p) => p.x + p.w)) + 40;
  const maxY = Math.max(...items.map((p) => p.y + p.h)) + 40;

  // Edges: each sibling step → next sibling step at same parent. Plus parent → first child for each child kind.
  const edges = [];
  function collectEdges(list, parentPath, parentId = null, parentKind = null) {
    list.forEach((step, idx) => {
      const path = parentPath ? [...parentPath, idx] : [idx];
      const key = pathKey(path);
      if (idx > 0) {
        const prevKey = pathKey(parentPath ? [...parentPath, idx - 1] : [idx - 1]);
        edges.push({ from: prevKey, to: key, kind: "seq" });
      } else if (parentId) {
        edges.push({ from: parentId, to: key, kind: parentKind === "do" ? "loop" : "control" });
      }
      ["do", "else", "catch", "finally"].forEach((kind) => {
        if (Array.isArray(step[kind]) && step[kind].length) {
          const childPath = [...path, kind];
          collectEdges(step[kind], childPath, key, kind);
        }
      });
    });
  }
  collectEdges(steps, null);

  // Apply current graph transform.
  root.setAttribute("transform", `translate(${graph.tx} ${graph.ty}) scale(${graph.scale})`);
  svg.setAttribute("viewBox", `0 0 ${Math.max(maxX, 100)} ${Math.max(maxY, 100)}`);

  // Draw edges first (under nodes).
  for (const e of edges) {
    const from = positions.get(e.from);
    const to = positions.get(e.to);
    if (!from || !to) continue;
    const fx = from.x + from.w / 2;
    const fy = from.y + from.h;
    const tx = to.x + to.w / 2;
    const ty = to.y;
    const path = document.createElementNS("http://www.w3.org/2000/svg", "path");
    const klass =
      e.kind === "loop"
        ? "graph-edge is-loop"
        : e.kind === "control"
          ? "graph-edge is-control"
          : "graph-edge";
    path.setAttribute("class", klass);
    const c1x = fx;
    const c1y = (fy + ty) / 2;
    const c2x = tx;
    const c2y = (fy + ty) / 2;
    path.setAttribute("d", `M ${fx} ${fy} C ${c1x} ${c1y} ${c2x} ${c2y} ${tx} ${ty}`);
    path.setAttribute("marker-end", "url(#arrowhead)");
    path.setAttribute("stroke", "currentColor");
    path.style.color =
      e.kind === "loop"
        ? "var(--accent)"
        : e.kind === "control"
          ? "var(--accent-2)"
          : "var(--line-strong)";
    root.appendChild(path);
  }

  // Draw nodes.
  for (const pos of positions.values()) {
    const family = pos.step.action ? pos.step.action.split(".")[0] : "misc";
    const fo = document.createElementNS("http://www.w3.org/2000/svg", "foreignObject");
    fo.setAttribute("x", String(pos.x));
    fo.setAttribute("y", String(pos.y));
    fo.setAttribute("width", String(pos.w));
    fo.setAttribute("height", String(pos.h + 40));
    fo.setAttribute("class", "graph-node-foreign");
    const withSummary = renderWithSummary(pos.step.with);
    const selected = pathKey(state.selectedStepPath || []) === pathKey(pos.path);
    fo.innerHTML = `
      <div xmlns="http://www.w3.org/1999/xhtml" class="graph-node family-${html(family)} ${selected ? "is-selected" : ""}" data-step-path="${html(pathKey(pos.path))}">
        <div class="graph-node-head">
          <span class="id">${html(pos.step.id || "(no id)")}</span>
          <span class="action">${html(pos.step.action || "?")}</span>
        </div>
        <div class="graph-node-body">${withSummary || "<em style=\"color: var(--faint)\">无参数</em>"}</div>
        ${pos.step.retry ? `<div class="graph-node-foot">retry × ${html(pos.step.retry.times || 1)}</div>` : ""}
      </div>`;
    root.appendChild(fo);
  }

  // Bind clicks (event delegation).
  svg.onclick = (event) => {
    const node = event.target.closest("[data-step-path]");
    if (node) {
      selectStep(parsePathKey(node.dataset.stepPath));
    }
  };
}

function renderWithSummary(w) {
  if (!w || typeof w !== "object") return "";
  return Object.entries(w)
    .slice(0, 3)
    .map(([k, v]) => {
      let val = typeof v === "string" ? v : pretty(v);
      if (val.length > 38) val = val.slice(0, 36) + "…";
      return `<span><strong>${html(k)}</strong>: ${html(val)}</span>`;
    })
    .join("<br />");
}

function pathKey(path) {
  return (path || []).map((p) => (typeof p === "number" ? String(p) : `:${p}`)).join("/");
}
function parsePathKey(key) {
  if (!key) return [];
  return key.split("/").map((s) => (s.startsWith(":") ? s.slice(1) : Number(s)));
}

// Graph pan + zoom
function bindGraphPan() {
  const svg = $("graphSvg");
  let panning = false;
  let startX = 0;
  let startY = 0;
  let origTx = 0;
  let origTy = 0;
  svg.addEventListener("pointerdown", (e) => {
    if (e.target.closest("[data-step-path]")) return; // node drag handled elsewhere (selection)
    panning = true;
    svg.classList.add("is-panning");
    startX = e.clientX;
    startY = e.clientY;
    origTx = graph.tx;
    origTy = graph.ty;
    svg.setPointerCapture(e.pointerId);
  });
  svg.addEventListener("pointermove", (e) => {
    if (!panning) return;
    graph.tx = origTx + (e.clientX - startX);
    graph.ty = origTy + (e.clientY - startY);
    $("graphRoot").setAttribute("transform", `translate(${graph.tx} ${graph.ty}) scale(${graph.scale})`);
  });
  svg.addEventListener("pointerup", () => { panning = false; svg.classList.remove("is-panning"); });
  svg.addEventListener("pointercancel", () => { panning = false; svg.classList.remove("is-panning"); });
  svg.addEventListener("wheel", (e) => {
    if (!e.ctrlKey && !e.metaKey) return;
    e.preventDefault();
    const factor = e.deltaY < 0 ? 1.08 : 1 / 1.08;
    graph.scale = Math.max(0.4, Math.min(2.5, graph.scale * factor));
    $("graphRoot").setAttribute("transform", `translate(${graph.tx} ${graph.ty}) scale(${graph.scale})`);
  }, { passive: false });

  // Drop target for actions library
  svg.addEventListener("dragover", (e) => { e.preventDefault(); });
  svg.addEventListener("drop", (e) => {
    e.preventDefault();
    const id = e.dataTransfer.getData("text/x-lumo-action");
    if (id) appendStepToSource(id);
  });
}

// ─── Tree view ─────────────────────────────────────────────────────────────

function renderTree() {
  const root = $("treeRoot");
  root.innerHTML = "";
  if (!state.ast) return;
  const steps = extractSteps(state.ast);
  root.appendChild(renderTreeList(steps, null));
}

function renderTreeList(list, parentPath) {
  const wrap = document.createElement("div");
  list.forEach((step, idx) => {
    const path = parentPath ? [...parentPath, idx] : [idx];
    const key = pathKey(path);
    const childKinds = ["do", "else", "catch", "finally"].filter((k) => Array.isArray(step[k]) && step[k].length);
    const hasChildren = childKinds.length > 0;
    const selected = pathKey(state.selectedStepPath || []) === key;
    const node = document.createElement("div");
    node.className = `tree-node ${hasChildren ? "" : "is-leaf"} ${selected ? "is-selected" : ""}`;
    node.dataset.stepPath = key;
    node.innerHTML = `
      <span class="caret">${hasChildren ? "▾" : "·"}</span>
      <span class="label"><span class="id">${html(step.id || "(no id)")}</span><span class="action">${html(step.action || "?")}</span></span>
      <span class="badge">${html(step.action ? step.action.split(".")[0] : "?")}</span>
    `;
    node.addEventListener("click", (e) => {
      e.stopPropagation();
      const caret = e.target.closest(".caret");
      if (caret && hasChildren) {
        const childrenEl = node.nextElementSibling;
        if (childrenEl?.classList.contains("tree-children")) {
          childrenEl.classList.toggle("is-collapsed");
          node.querySelector(".caret").textContent = childrenEl.classList.contains("is-collapsed") ? "▸" : "▾";
        }
        return;
      }
      selectStep(path);
    });
    wrap.appendChild(node);
    if (hasChildren) {
      const childWrap = document.createElement("div");
      childWrap.className = "tree-children";
      childKinds.forEach((kind) => {
        const label = document.createElement("div");
        label.style.fontSize = "10.5px";
        label.style.color = "var(--accent-2)";
        label.style.padding = "4px 4px 2px";
        label.textContent = `▿ ${kind}`;
        childWrap.appendChild(label);
        childWrap.appendChild(renderTreeList(step[kind], [...path, kind]));
      });
      wrap.appendChild(childWrap);
    }
  });
  return wrap;
}

// ─── Steps view (YingDao-style linear list + inline expand + DnD) ─────────

const AI_MODES = ["off", "fallback", "primary"];
const AI_LABEL = { off: "AI 关", fallback: "AI 兜底", primary: "AI 主导" };
const AI_HELPER_LABEL = {
  heal_selector: "选择器自愈",
  extract_visual: "视觉抽取",
  decide: "语义决策",
};

function renderStepList() {
  const root = $("stepList");
  root.innerHTML = "";
  if (!state.ast) {
    root.innerHTML = `
      <div class="flow-dropzone flow-dropzone-empty" id="emptyDropZone">
        <div class="flow-dropzone-icon">🧩</div>
        <div class="flow-dropzone-title">从左侧指令面板 <strong>拖拽指令</strong> 到这里开始设计流程</div>
        <div class="flow-dropzone-sub">支持鼠标拖动重排、AI 模式一键开启、嵌套循环 / 条件分支</div>
        <div class="step-add-row" style="margin-top:14px"><button id="addFirstStepBtn">+ 或点此创建第一个步骤</button></div>
      </div>`;
    $("addFirstStepBtn")?.addEventListener("click", () => appendStepToSource("control.log"));
    bindFlowDropZone($("emptyDropZone"));
    return;
  }
  const steps = extractSteps(state.ast);
  if (!steps.length) {
    root.innerHTML = `
      <div class="flow-dropzone flow-dropzone-empty" id="emptyDropZone">
        <div class="flow-dropzone-icon">🧩</div>
        <div class="flow-dropzone-title">把左侧动作 <strong>拖到这里</strong> 组装流程</div>
        <div class="flow-dropzone-sub">或点击下方 + 立即添加一个 control.log 占位步骤</div>
        <div class="step-add-row" style="margin-top:14px"><button id="addFirstStepBtn">+ 添加步骤</button></div>
      </div>`;
    $("addFirstStepBtn")?.addEventListener("click", () => appendStepToSource("control.log"));
    bindFlowDropZone($("emptyDropZone"));
    return;
  }
  const items = [];
  walkSteps(steps, (step, info) => items.push({ step, ...info }));

  // Build flowchart: START → row → connector → row → connector ... → END
  const parts = [`<div class="flow-node flow-node-terminal flow-node-start">▶ 开始</div>`];
  parts.push(`<div class="flow-connector"><span class="flow-arrow">▼</span></div>`);
  items.forEach((it, i) => {
    parts.push(renderStepRow(it, i));
    parts.push(`<div class="flow-connector" data-insert-after="${html(pathKey(it.path))}" title="拖动作到这里 = 在此处插入">
      <span class="flow-arrow">▼</span>
      <button class="flow-insert-here" data-insert-after-btn="${html(pathKey(it.path))}" title="在此处插入步骤">+</button>
    </div>`);
  });
  parts.push(`<div class="flow-node flow-node-terminal flow-node-end">■ 结束</div>`);
  parts.push(`<div class="step-add-row"><button id="addStepBottomBtn">+ 末尾添加步骤</button></div>`);

  root.innerHTML = parts.join("");
  bindStepListEvents();
  bindFlowDropZone(root);
}

function bindFlowDropZone(zone) {
  if (!zone) return;
  zone.addEventListener("dragover", (e) => {
    if (e.dataTransfer.types.includes("text/x-lumo-action")) {
      e.preventDefault();
      zone.classList.add("is-drop-hover");
    }
  });
  zone.addEventListener("dragleave", () => zone.classList.remove("is-drop-hover"));
  zone.addEventListener("drop", (e) => {
    zone.classList.remove("is-drop-hover");
    const actionId = e.dataTransfer.getData("text/x-lumo-action");
    if (actionId) { e.preventDefault(); appendStepToSource(actionId); }
  });
}

function renderStepRow({ step, depth, path }, idx) {
  const ai = step.ai || {};
  const aiMode = (ai.mode || "off").toLowerCase();
  const selectedKey = pathKey(state.selectedStepPath || []);
  const myKey = pathKey(path);
  const selected = selectedKey === myKey;
  const family = (step.action || "misc").split(".")[0];
  const zh = zhAction(step.action);
  const summary = renderWithSummary(step.with);
  const indent = depth * 16 + 4;
  return `<div class="step-row family-${html(family)} ${selected ? "is-selected" : ""}"
              data-step-path="${html(myKey)}" data-step-idx="${idx}"
              draggable="true"
              style="padding-left: ${indent}px">
    <span class="step-handle" title="拖动重排（同级）">⋮⋮</span>
    <span class="step-num">${idx + 1}</span>
    <span class="step-id" title="${html(step.id || '')}">${html(step.id || "(no id)")}</span>
    <span class="step-action" title="${html(step.action || '?')}">
      <span class="step-action-zh">${html(zh.label)}</span>
      <span class="step-action-code">${html(step.action || "?")}</span>
    </span>
    <span class="step-summary">${summary || '<em style="color: var(--faint)">无参数</em>'}</span>
    <button class="step-ai-btn ai-state-${html(aiMode)}" data-ai-toggle="${html(myKey)}"
            title="AI 模式：${html(AI_LABEL[aiMode] || aiMode)} · 点击打开 AI 抽屉">✨</button>
    <button class="step-icon-btn step-expand-btn" data-expand="${html(myKey)}" title="展开配置">⤢</button>
    <button class="step-icon-btn step-insert-btn" data-insert="${html(myKey)}" title="在此后插入">+</button>
    <button class="step-icon-btn step-del-btn" data-del="${html(myKey)}" title="删除">×</button>
    <div class="step-expand-body" data-expand-body="${html(myKey)}" hidden></div>
  </div>`;
}

function bindStepListEvents() {
  const root = $("stepList");
  // Container drop: action library → append new step at end
  root.addEventListener("dragover", (e) => {
    if (e.dataTransfer.types.includes("text/x-lumo-action")) e.preventDefault();
  });
  root.addEventListener("drop", (e) => {
    if (e.target !== root) return; // children handle their own
    const actionId = e.dataTransfer.getData("text/x-lumo-action");
    if (actionId) { e.preventDefault(); appendStepToSource(actionId); }
  });
  // Flow-connector insert buttons (click + between steps)
  root.querySelectorAll("[data-insert-after-btn]").forEach((b) => {
    b.addEventListener("click", (e) => {
      e.stopPropagation();
      insertStepAfterPath(parsePathKey(b.dataset.insertAfterBtn));
    });
  });
  // Drop on connector = insert action there
  root.querySelectorAll(".flow-connector[data-insert-after]").forEach((c) => {
    c.addEventListener("dragover", (e) => {
      if (e.dataTransfer.types.includes("text/x-lumo-action")) {
        e.preventDefault();
        c.classList.add("is-drop-hover");
      }
    });
    c.addEventListener("dragleave", () => c.classList.remove("is-drop-hover"));
    c.addEventListener("drop", (e) => {
      c.classList.remove("is-drop-hover");
      const actionId = e.dataTransfer.getData("text/x-lumo-action");
      if (!actionId) return;
      e.preventDefault();
      insertStepAfterPath(parsePathKey(c.dataset.insertAfter), actionId);
    });
  });
  // Row click → select
  root.querySelectorAll(".step-row").forEach((row) => {
    row.addEventListener("click", (e) => {
      if (e.target.closest("button") || e.target.closest("[data-expand-body]")) return;
      selectStep(parsePathKey(row.dataset.stepPath));
    });
  });
  // AI ✨ → open drawer
  root.querySelectorAll("[data-ai-toggle]").forEach((b) => {
    b.addEventListener("click", (e) => {
      e.stopPropagation();
      openAiDrawer(parsePathKey(b.dataset.aiToggle));
    });
  });
  // Expand
  root.querySelectorAll("[data-expand]").forEach((b) => {
    b.addEventListener("click", (e) => {
      e.stopPropagation();
      toggleStepExpand(parsePathKey(b.dataset.expand));
    });
  });
  // Insert
  root.querySelectorAll("[data-insert]").forEach((b) => {
    b.addEventListener("click", (e) => {
      e.stopPropagation();
      insertStepAfterPath(parsePathKey(b.dataset.insert));
    });
  });
  // Delete
  root.querySelectorAll("[data-del]").forEach((b) => {
    b.addEventListener("click", (e) => {
      e.stopPropagation();
      const path = parsePathKey(b.dataset.del);
      const step = findStepByPath(extractSteps(state.ast), path);
      if (step && confirm(`删除步骤 "${step.id || step.action}"？`)) deleteStepByPath(path);
    });
  });
  // Drag / drop reorder (sibling-level only)
  let dragKey = null;
  root.querySelectorAll(".step-row").forEach((row) => {
    row.addEventListener("dragstart", (e) => {
      dragKey = row.dataset.stepPath;
      e.dataTransfer.effectAllowed = "move";
      e.dataTransfer.setData("text/x-lumo-step", dragKey);
      row.classList.add("is-dragging");
    });
    row.addEventListener("dragend", () => {
      row.classList.remove("is-dragging");
      root.querySelectorAll(".is-drop-target").forEach((r) => r.classList.remove("is-drop-target"));
    });
    row.addEventListener("dragover", (e) => {
      const srcKey = dragKey || e.dataTransfer.getData("text/x-lumo-step");
      if (!srcKey || srcKey === row.dataset.stepPath) return;
      // Only allow dropping on same-parent sibling.
      if (sameParent(parsePathKey(srcKey), parsePathKey(row.dataset.stepPath))) {
        e.preventDefault();
        row.classList.add("is-drop-target");
      }
    });
    row.addEventListener("dragleave", () => row.classList.remove("is-drop-target"));
    row.addEventListener("drop", (e) => {
      e.preventDefault();
      const srcKey = e.dataTransfer.getData("text/x-lumo-step") || dragKey;
      const dstKey = row.dataset.stepPath;
      row.classList.remove("is-drop-target");
      if (!srcKey || !dstKey || srcKey === dstKey) return;
      const srcPath = parsePathKey(srcKey);
      const dstPath = parsePathKey(dstKey);
      if (sameParent(srcPath, dstPath)) moveStepBefore(srcPath, dstPath);
    });
    // Drop from action library: insert as new step under the same parent
    row.addEventListener("drop", (e) => {
      const actionId = e.dataTransfer.getData("text/x-lumo-action");
      if (actionId) {
        e.preventDefault();
        insertNewStepNear(parsePathKey(row.dataset.stepPath), actionId);
      }
    });
  });
  // Bottom add
  $("addStepBottomBtn")?.addEventListener("click", () => appendStepToSource("control.log"));
}

function sameParent(a, b) {
  if (!a || !b || !a.length || !b.length) return true;
  return pathKey(a.slice(0, -1)) === pathKey(b.slice(0, -1));
}

async function toggleStepExpand(path) {
  const key = pathKey(path);
  const body = $("stepList").querySelector(`[data-expand-body="${cssEscape(key)}"]`);
  if (!body) return;
  if (!body.hidden) {
    body.hidden = true;
    body.innerHTML = "";
    return;
  }
  const step = findStepByPath(extractSteps(state.ast), path);
  if (!step) return;
  body.hidden = false;
  body.innerHTML = `<div class="step-expand-inner"><em style="color: var(--faint); font-size: 11px">加载 schema…</em></div>`;
  try {
    const schema = await loadSchema(step.action);
    const fields = renderSchemaFields(schema, step.with || {});
    body.querySelector(".step-expand-inner").innerHTML = `
      <div class="prop-form">
        ${fields || '<em style="color: var(--faint); font-size: 11px">该动作未声明 properties</em>'}
        <button class="primary" data-apply-expand="${html(key)}" style="margin-top: 8px">将变更写入 YAML</button>
      </div>`;
    body.querySelector("[data-apply-expand]").addEventListener("click", () => {
      const newWith = readWithFromContainer(body);
      const updated = mutateStepInSource(state.source, step.id, {
        id: step.id, action: step.action, with: newWith, ai: step.ai, retry: step.retry, when: step.when, bind: step.bind,
        do: step.do, else: step.else, catch: step.catch, finally: step.finally,
      });
      state.source = updated;
      state.ast = parseYaml(state.source);
      toast("已应用到 YAML 缓冲区", "记得点 💾 保存", "ok");
      renderActiveView();
    });
  } catch (e) {
    body.querySelector(".step-expand-inner").innerHTML = `<em style="color: var(--bad); font-size: 11px">schema 加载失败: ${html(String(e))}</em>`;
  }
}

function readWithFromContainer(root) {
  const out = {};
  root.querySelectorAll("[data-with-key]").forEach((el) => {
    const key = el.dataset.withKey;
    const type = el.dataset.type;
    if (type === "boolean") out[key] = el.checked;
    else if (type === "integer" || type === "number") {
      if (el.value === "" || el.value === null) return;
      out[key] = Number(el.value);
    } else if (type === "object" || type === "array") {
      if (!el.value.trim()) return;
      try { out[key] = JSON.parse(el.value); } catch { out[key] = el.value; }
    } else if (el.value !== "") {
      out[key] = el.value;
    }
  });
  return out;
}

function cssEscape(s) {
  return s.replace(/(["\\:])/g, "\\$1");
}

// ─── AI drawer (per-step ✨) ───────────────────────────────────────────────

let aiDrawerPath = null;

function openAiDrawer(path) {
  aiDrawerPath = path;
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

  const close = () => { overlay.hidden = true; overlay.innerHTML = ""; aiDrawerPath = null; };
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
      do: step.do, else: step.else, catch: step.catch, finally: step.finally,
    });
    if (addCap && newMode !== "off") updated = ensureLlmCapability(updated);
    state.source = updated;
    state.ast = parseYaml(state.source);
    close();
    toast("已写入 ai 配置", `step ${step.id} · mode=${newMode}`, "ok");
    renderActiveView();
  });
}

// ─── YAML mutation helpers (insert / delete / move / ai / capability) ─────

function findStepRange(lines, originalId) {
  const escaped = String(originalId).replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
  const re = new RegExp(`^(\\s*)-\\s*id:\\s*${escaped}\\b`);
  let startIdx = -1;
  let baseIndent = 0;
  for (let i = 0; i < lines.length; i++) {
    const m = lines[i].match(re);
    if (m) { startIdx = i; baseIndent = m[1].length; break; }
  }
  if (startIdx < 0) return null;
  let endIdx = lines.length;
  for (let j = startIdx + 1; j < lines.length; j++) {
    const t = lines[j];
    if (t.trim() === "") continue;
    const ind = t.match(/^( *)/)[1].length;
    if (ind <= baseIndent && !/^\s*$/.test(t)) { endIdx = j; break; }
  }
  return { startIdx, endIdx, baseIndent };
}

function insertStepAfterPath(path, actionId) {
  const step = findStepByPath(extractSteps(state.ast), path);
  if (!step) return;
  const lines = state.source.split(/\r?\n/);
  const range = findStepRange(lines, step.id);
  if (!range) return;
  const useAction = actionId || "control.log";
  const id = actionId
    ? `${actionId.replace(/\./g, "_")}_${Math.floor(Math.random() * 999)}`
    : `step_${Math.floor(Math.random() * 9999)}`;
  const body = actionId ? { id, action: useAction, with: {} } : { id, action: useAction, with: { message: "TODO" } };
  const block = emitStep(body, range.baseIndent).split("\n");
  const newLines = lines.slice(0, range.endIdx).concat(block).concat(lines.slice(range.endIdx));
  state.source = newLines.join("\n");
  state.ast = parseYaml(state.source);
  toast("已插入步骤", `${id} (${useAction})`, "ok");
  renderActiveView();
}

function insertNewStepNear(path, actionId) {
  const step = findStepByPath(extractSteps(state.ast), path);
  if (!step) { appendStepToSource(actionId); return; }
  const lines = state.source.split(/\r?\n/);
  const range = findStepRange(lines, step.id);
  if (!range) { appendStepToSource(actionId); return; }
  const id = `${actionId.replace(/\./g, "_")}_${Math.floor(Math.random() * 999)}`;
  const block = emitStep({ id, action: actionId, with: {} }, range.baseIndent).split("\n");
  const newLines = lines.slice(0, range.endIdx).concat(block).concat(lines.slice(range.endIdx));
  state.source = newLines.join("\n");
  state.ast = parseYaml(state.source);
  toast("已添加节点", `${id} (${actionId})`, "ok");
  renderActiveView();
}

function deleteStepByPath(path) {
  const step = findStepByPath(extractSteps(state.ast), path);
  if (!step) return;
  const lines = state.source.split(/\r?\n/);
  const range = findStepRange(lines, step.id);
  if (!range) return;
  const newLines = lines.slice(0, range.startIdx).concat(lines.slice(range.endIdx));
  state.source = newLines.join("\n");
  state.ast = parseYaml(state.source);
  if (pathKey(state.selectedStepPath || []) === pathKey(path)) {
    state.selectedStepPath = null;
    state.selectedStepId = null;
  }
  toast("已删除步骤", step.id, "warn");
  renderActiveView();
  renderInspector();
}

function moveStepBefore(srcPath, dstPath) {
  const srcStep = findStepByPath(extractSteps(state.ast), srcPath);
  const dstStep = findStepByPath(extractSteps(state.ast), dstPath);
  if (!srcStep || !dstStep) return;
  const lines = state.source.split(/\r?\n/);
  const srcR = findStepRange(lines, srcStep.id);
  if (!srcR) return;
  const block = lines.slice(srcR.startIdx, srcR.endIdx);
  const remaining = lines.slice(0, srcR.startIdx).concat(lines.slice(srcR.endIdx));
  const dstR = findStepRange(remaining, dstStep.id);
  if (!dstR) return;
  const final = remaining.slice(0, dstR.startIdx).concat(block).concat(remaining.slice(dstR.startIdx));
  state.source = final.join("\n");
  state.ast = parseYaml(state.source);
  toast("已重排", `${srcStep.id} → before ${dstStep.id}`, "ok");
  renderActiveView();
}

function ensureLlmCapability(source) {
  const ast = parseYaml(source);
  const llm = ast?.spec?.capabilities?.llm;
  if (Array.isArray(llm) && (llm.includes("*") || llm.length > 0)) return source;
  return ensureCapability(source, "llm", "*");
}

function ensureNetworkCapability(source, host) {
  const ast = parseYaml(source);
  const net = ast?.spec?.capabilities?.network || [];
  if (Array.isArray(net) && (net.includes(host) || net.includes("*"))) return source;
  return ensureCapability(source, "network", host);
}

/** Add `<key>: [<value>]` to the spec.capabilities block. Idempotent. */
function ensureCapability(source, key, value) {
  const lines = source.split(/\r?\n/);
  const specIdx = lines.findIndex((l) => /^spec\s*:/.test(l));
  if (specIdx < 0) {
    // No spec block — append at end.
    return source + `\nspec:\n  capabilities:\n    ${key}:\n      - ${yamlScalar(value)}\n`;
  }
  // Find capabilities: within spec.
  let capIdx = -1;
  let capIndent = 2;
  for (let i = specIdx + 1; i < lines.length; i++) {
    const t = lines[i];
    if (/^\S/.test(t)) break; // left spec
    if (/^\s*capabilities\s*:/.test(t)) { capIdx = i; capIndent = (t.match(/^( *)/)[1].length); break; }
  }
  if (capIdx < 0) {
    // Insert capabilities block right after spec:
    const indent = "  ";
    const insertAt = specIdx + 1;
    const block = [
      `${indent}capabilities:`,
      `${indent}  ${key}:`,
      `${indent}    - ${yamlScalar(value)}`,
    ];
    return lines.slice(0, insertAt).concat(block).concat(lines.slice(insertAt)).join("\n");
  }
  // Find <key>: within capabilities.
  let keyIdx = -1;
  let keyIndent = capIndent + 2;
  for (let j = capIdx + 1; j < lines.length; j++) {
    const t = lines[j];
    if (t.trim() === "") continue;
    const ind = t.match(/^( *)/)[1].length;
    if (ind <= capIndent && !/^\s*$/.test(t)) break;
    const re = new RegExp(`^\\s{${keyIndent}}${key}\\s*:`);
    if (re.test(t)) { keyIdx = j; break; }
  }
  if (keyIdx < 0) {
    // Append `<key>:\n  - <value>` after capabilities:
    const ind = " ".repeat(keyIndent);
    const block = [`${ind}${key}:`, `${ind}  - ${yamlScalar(value)}`];
    return lines.slice(0, capIdx + 1).concat(block).concat(lines.slice(capIdx + 1)).join("\n");
  }
  // Append `- value` under existing key block.
  const itemIndent = " ".repeat(keyIndent + 2);
  return lines.slice(0, keyIdx + 1).concat([`${itemIndent}- ${yamlScalar(value)}`]).concat(lines.slice(keyIdx + 1)).join("\n");
}

// ─── Code view ─────────────────────────────────────────────────────────────

function renderCode() {
  const ta = $("codeEditor");
  ta.value = state.source || "";
  syncGutter();
}

function syncGutter() {
  const ta = $("codeEditor");
  const lines = (ta.value || "").split("\n").length;
  const gutter = $("codeGutter");
  let s = "";
  for (let i = 1; i <= lines; i++) s += i + "\n";
  gutter.textContent = s;
}

// ─── Step selection + inspector ───────────────────────────────────────────

function selectStep(path) {
  state.selectedStepPath = path;
  const step = path ? findStepByPath(extractSteps(state.ast), path) : null;
  state.selectedStepId = step?.id || null;
  if (state.viewMode === "graph") renderGraph();
  if (state.viewMode === "tree") renderTree();
  renderInspector();
}

async function renderInspector() {
  const body = $("inspectorBody");
  const path = state.selectedStepPath;
  if (!path) {
    body.innerHTML = `<div class="prop-empty">选择一个节点查看属性</div>`;
    return;
  }
  const step = findStepByPath(extractSteps(state.ast), path);
  if (!step) {
    body.innerHTML = `<div class="prop-empty">该节点已不存在</div>`;
    return;
  }
  body.innerHTML = `<div class="prop-form" id="propForm">
    <div class="prop-field">
      <label>Step ID</label>
      <input data-prop="id" value="${html(step.id || "")}" />
    </div>
    <div class="prop-field">
      <label>Action</label>
      <input data-prop="action" value="${html(step.action || "")}" />
      <span class="hint">家族：<strong>${html((step.action || "").split(".")[0] || "?")}</strong></span>
    </div>
    <div class="section-title">with: 参数（按 Action JSON Schema）</div>
    <div id="schemaFields"><em style="color: var(--faint); font-size: 11px">加载 schema…</em></div>
    <button class="primary" id="applyStepBtn" style="margin-top: 8px">将变更写入 YAML</button>
    <div class="hint">编辑后点击 "写入 YAML"。Code 视图为权威源；此面板是 schema-aware 辅助。</div>
    <button id="runThisStepBtn" style="margin-top: 6px">▷ 单独运行此节点</button>
  </div>`;

  if (step.action) {
    try {
      const schema = await loadSchema(step.action);
      const fields = renderSchemaFields(schema, step.with || {});
      $("schemaFields").innerHTML = fields || `<em style="color: var(--faint); font-size: 11px">该动作未声明 properties</em>`;
    } catch (e) {
      $("schemaFields").innerHTML = `<em style="color: var(--bad); font-size: 11px">schema 加载失败: ${html(String(e))}</em>`;
    }
  } else {
    $("schemaFields").innerHTML = "";
  }

  $("applyStepBtn").addEventListener("click", () => applyInspectorEdits(step));
  $("runThisStepBtn").addEventListener("click", () => runStep(step.id));
}

async function loadSchema(actionId) {
  if (state.schemaCache.has(actionId)) return state.schemaCache.get(actionId);
  const schema = await call("action_schema", { id: actionId });
  state.schemaCache.set(actionId, schema);
  return schema;
}

function renderSchemaFields(schema, withValue) {
  const props = schema?.properties || {};
  const required = new Set(schema?.required || []);
  return Object.entries(props)
    .map(([key, spec]) => {
      const cur = withValue?.[key];
      const type = Array.isArray(spec.type) ? spec.type[0] : spec.type;
      const desc = spec.description || "";
      let control = "";
      const val = cur === undefined || cur === null ? "" : typeof cur === "string" ? cur : JSON.stringify(cur);
      if (type === "boolean") {
        control = `<label class="toggle"><input type="checkbox" data-with-key="${html(key)}" data-type="boolean" ${cur ? "checked" : ""}/> ${cur ? "true" : "false"}</label>`;
      } else if (type === "integer" || type === "number") {
        control = `<input type="number" data-with-key="${html(key)}" data-type="${type}" value="${html(val)}"/>`;
      } else if (type === "object" || type === "array") {
        control = `<textarea data-with-key="${html(key)}" data-type="${type}" style="min-height: 60px">${html(val)}</textarea>`;
      } else {
        control = `<input type="text" data-with-key="${html(key)}" data-type="string" value="${html(val)}"/>`;
      }
      return `<div class="prop-field">
        <label>${html(key)} ${required.has(key) ? '<span class="req">●</span>' : ""}</label>
        ${control}
        ${desc ? `<span class="hint">${html(desc)}</span>` : ""}
      </div>`;
    })
    .join("");
}

function readInspectorWith() {
  const out = {};
  $$("[data-with-key]").forEach((el) => {
    const key = el.dataset.withKey;
    const type = el.dataset.type;
    if (type === "boolean") {
      out[key] = el.checked;
    } else if (type === "integer" || type === "number") {
      if (el.value === "" || el.value === null) return;
      out[key] = Number(el.value);
    } else if (type === "object" || type === "array") {
      if (!el.value.trim()) return;
      try { out[key] = JSON.parse(el.value); }
      catch { out[key] = el.value; }
    } else {
      if (el.value !== "") out[key] = el.value;
    }
  });
  return out;
}

function applyInspectorEdits(step) {
  const newId = $$('[data-prop="id"]')[0]?.value?.trim() || step.id;
  const newAction = $$('[data-prop="action"]')[0]?.value?.trim() || step.action;
  const newWith = readInspectorWith();
  // Locate the step block in the YAML by `- id: <step.id>` and rewrite the
  // shallow scalars + `with:` block in place. This is a "good-enough" textual
  // mutation that keeps comments and unknown keys intact.
  const updated = mutateStepInSource(state.source, step.id, { id: newId, action: newAction, with: newWith });
  state.source = updated;
  state.ast = parseYaml(state.source);
  state.selectedStepId = newId;
  toast("已应用到 YAML 缓冲区", "记得点 💾 保存", "ok");
  renderActiveView();
  renderInspector();
}

function mutateStepInSource(source, originalId, patch) {
  const lines = source.split(/\r?\n/);
  const escapeId = originalId.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
  const re = new RegExp(`^(\\s*)-\\s*id:\\s*${escapeId}\\b`);
  let startIdx = -1;
  let baseIndent = 0;
  for (let i = 0; i < lines.length; i++) {
    const m = lines[i].match(re);
    if (m) { startIdx = i; baseIndent = m[1].length; break; }
  }
  if (startIdx < 0) {
    // Fallback: append as new step at end of `steps:` section.
    return source + `\n${emitStep(patch, 4)}`;
  }
  // Determine block end: next sibling line with same indent OR less.
  const itemIndent = baseIndent + 2; // contents are indented 2 more
  let endIdx = lines.length;
  for (let j = startIdx + 1; j < lines.length; j++) {
    const t = lines[j];
    if (t.trim() === "") continue;
    const ind = t.match(/^( *)/)[1].length;
    if (ind <= baseIndent && !/^\s*$/.test(t)) { endIdx = j; break; }
  }
  // Re-emit block.
  const replacement = emitStep(patch, baseIndent);
  return lines.slice(0, startIdx).concat(replacement.split("\n")).concat(lines.slice(endIdx)).join("\n");
}

function emitStep(step, baseIndent) {
  const pad = " ".repeat(baseIndent);
  const cIndent = " ".repeat(baseIndent + 2);
  let out = `${pad}- id: ${yamlScalar(step.id)}\n${cIndent}action: ${yamlScalar(step.action)}\n`;
  if (step.when !== undefined && step.when !== null && step.when !== "") {
    out += `${cIndent}when: ${yamlInline(step.when)}\n`;
  }
  if (step.bind) out += `${cIndent}bind: ${yamlScalar(step.bind)}\n`;
  if (step.with && Object.keys(step.with).length) {
    out += `${cIndent}with:\n`;
    for (const [k, v] of Object.entries(step.with)) {
      out += `${cIndent}  ${k}: ${yamlInline(v)}\n`;
    }
  }
  if (step.retry && Object.keys(step.retry).length) {
    out += `${cIndent}retry:\n`;
    for (const [k, v] of Object.entries(step.retry)) {
      out += `${cIndent}  ${k}: ${yamlInline(v)}\n`;
    }
  }
  if (step.ai && (step.ai.mode || step.ai.model || step.ai.prompt)) {
    out += `${cIndent}ai:\n`;
    if (step.ai.mode) out += `${cIndent}  mode: ${step.ai.mode}\n`;
    if (step.ai.model) out += `${cIndent}  model: ${yamlScalar(step.ai.model)}\n`;
    if (step.ai.prompt) out += `${cIndent}  prompt: ${yamlInline(step.ai.prompt)}\n`;
  }
  for (const kind of ["do", "else", "catch", "finally"]) {
    const arr = step[kind];
    if (Array.isArray(arr) && arr.length) {
      out += `${cIndent}${kind}:\n`;
      for (const child of arr) {
        out += emitStep(child, baseIndent + 4) + "\n";
      }
    }
  }
  return out.replace(/\n$/, "");
}

function yamlScalar(s) {
  if (typeof s !== "string") return JSON.stringify(s);
  if (/^[\w.\-/]+$/.test(s)) return s;
  return JSON.stringify(s);
}

function yamlInline(v) {
  if (v === null || v === undefined) return "~";
  if (typeof v === "string") {
    if (v.includes("\n")) return `|\n      ${v.split("\n").join("\n      ")}`;
    if (/[:#{}\[\]&*!|<>='"%@`,]/.test(v) || /^\s|\s$/.test(v)) return JSON.stringify(v);
    return v;
  }
  if (typeof v === "boolean" || typeof v === "number") return String(v);
  return JSON.stringify(v);
}

function appendStepToSource(actionId) {
  if (!state.source) {
    state.source = `apiVersion: lumorpa.io/v1\nkind: Flow\nmetadata:\n  id: untitled\n  version: 0.1.0\nspec:\n  steps:\n`;
  }
  const id = `${actionId.replace(/\./g, "_")}_${Math.floor(Math.random() * 1000)}`;
  // Append at end of file with 4-space indent (matches typical examples).
  state.source += `\n    - id: ${id}\n      action: ${actionId}\n      with: {}\n`;
  state.ast = parseYaml(state.source);
  renderActiveView();
  toast("已添加节点", `${id} (${actionId}) — 切到代码视图后点 💾 保存`, "ok");
}

function appendStepWithSelector(actionId, element) {
  if (!state.source) {
    state.source = `apiVersion: lumorpa.io/v1\nkind: Flow\nmetadata:\n  id: untitled\n  version: 0.1.0\nspec:\n  steps:\n`;
  }
  const id = `${actionId.replace(/\./g, "_")}_${Math.floor(Math.random() * 1000)}`;
  const css = element?.fingerprints?.css || "";
  const label = element?.label ? `  # ${element.label}` : "";
  state.source += `\n    - id: ${id}${label}\n      action: ${actionId}\n      with:\n        selector: ${JSON.stringify(css)}\n`;
  state.ast = parseYaml(state.source);
  renderActiveView();
  toast("已添加节点", `${id} → ${element?.label || actionId}`, "ok");
}

// ─── Run / RunStep + Time-Travel ───────────────────────────────────────────

async function runSelectedFlow() {
  if (!state.flowPath) { toast("先选择一个流程", "", "warn"); return; }
  setStatus("运行中…", "warn");
  $("runBtn").disabled = true;
  try {
    const response = await call("run_flow", {
      path: state.flowPath,
      inputsJson: $("inputsJson").value || "{}",
      noStore: false,
    });
    onRunComplete(response);
  } catch (error) {
    setStatus("运行失败", "bad");
    handleRunError(error);
  } finally {
    $("runBtn").disabled = false;
  }
}

async function runStep(stepId) {
  if (!state.flowPath || !stepId) return;
  setStatus(`单步运行 ${stepId} …`, "warn");
  try {
    const response = await call("run_step", {
      path: state.flowPath,
      stepId,
      inputsJson: $("inputsJson").value || "{}",
      noStore: false,
    });
    onRunComplete(response);
    toast("单步完成", `${stepId} · ${response.report.success ? "成功" : "失败"}`, response.report.success ? "ok" : "bad");
  } catch (error) {
    setStatus("单步失败", "bad");
    handleRunError(error);
  }
}

/** Detect `capability denied: <kind> \`<target>\`` and offer one-click whitelist. */
function handleRunError(error) {
  const msg = String(error);
  const m = msg.match(/capability denied:\s*(network|fs\.read|fs\.write|llm)\s*`([^`]+)`/i);
  if (!m) { toast("运行失败", msg, "bad"); return; }
  const kind = m[1].toLowerCase();
  const target = m[2];
  const stack = $("toastStack");
  const node = document.createElement("div");
  node.className = "toast bad";
  node.innerHTML = `
    <div class="toast-title">能力被拒绝: ${html(kind)} ${html(target)}</div>
    <div class="toast-body">流程的 spec.capabilities 没有声明这一项。点击下方按钮自动添加并重新校验。</div>
    <div class="toast-actions">
      <button class="primary" data-grant="${html(kind)}|${html(target)}">+ 加入白名单并保存</button>
      <button data-dismiss>忽略</button>
    </div>`;
  stack.appendChild(node);
  node.querySelector("[data-grant]").addEventListener("click", async () => {
    try {
      if (kind === "network") state.source = ensureNetworkCapability(state.source, target);
      else if (kind === "llm") state.source = ensureLlmCapability(state.source);
      else state.source = ensureCapability(state.source, kind.replace(".", ""), target); // fs.read/fs.write
      state.ast = parseYaml(state.source);
      if (state.flowPath) await call("save_flow_source", { path: state.flowPath, source: state.source });
      toast("已加入白名单", `${kind} ${target}`, "ok");
      node.remove();
      renderActiveView();
    } catch (e) {
      toast("写入失败", String(e), "bad");
    }
  });
  node.querySelector("[data-dismiss]").addEventListener("click", () => node.remove());
}

function onRunComplete(response) {
  state.activeRun = response.run;
  state.activeRunSteps = response.steps || [];
  // The run report's `outputs` is the full steps snapshot ({ "<id>": { result,
  // _ai? } }). Stash the per-step `_ai` traces so the timeline can show which
  // steps were rescued/decided by AI (purple "AI heal" badge).
  state.activeRunAi = collectAiTraces(response.report.outputs);
  $("outputBox").textContent = pretty({ runId: response.report.runId, outputs: response.report.outputs });
  $("timelineLabel").textContent = `${response.report.runId.slice(0, 14)}… · ${response.report.durationMs}ms`;
  $("timelineCounts").textContent = `${response.report.stepsOk}/${response.report.stepsTotal} 成功 · ${response.report.stepsFailed} 失败`;
  loadArtifacts(response.report.runId).finally(renderTimeline);
  setStatus(response.report.success ? "运行完成" : "运行失败", response.report.success ? "ok" : "bad");
  switchRightSection("outputs");
}

// Walk a steps-snapshot value and collect `{ stepId: _ai }` for every step
// whose output carries an `_ai.used` trace. Tolerates `null`/non-object input.
function collectAiTraces(outputs) {
  const traces = {};
  if (!outputs || typeof outputs !== "object") return traces;
  for (const [id, entry] of Object.entries(outputs)) {
    if (entry && typeof entry === "object" && entry._ai && entry._ai.used) {
      traces[id] = entry._ai;
    }
  }
  return traces;
}

// Resolve the `_ai` trace for a timeline step. step_runs paths look like
// `loop/item.3/click`; the snapshot is keyed by bare step id, so match on the
// final path segment as well as state === "ai_healed".
function aiTraceForStep(step) {
  if (!step) return null;
  const traces = state.activeRunAi || {};
  const leaf = String(step.path || step.stepId || "").split("/").pop();
  return traces[step.stepId] || traces[leaf] || null;
}

// X-07 Time-Travel: pull the run's blob artifacts (screenshots / DOM / HAR) so
// the scrubber can show what the screen looked like at each step. Tolerates the
// IPC missing on older builds.
async function loadArtifacts(runId) {
  state.activeArtifacts = [];
  state.artifactBlobCache = new Map();
  try {
    state.activeArtifacts = (await call("list_artifacts", { runId })) || [];
  } catch (_) {
    state.activeArtifacts = [];
  }
}

// Find the artifact (preferring screenshots) attributed to a given step path /
// id. step_runs.path and artifacts.step_id are matched loosely so loop-nested
// steps still line up.
function artifactForStep(step) {
  if (!state.activeArtifacts.length || !step) return null;
  const matches = state.activeArtifacts.filter(
    (a) => a.step_id && (a.step_id === step.stepId || a.step_id === step.path)
  );
  const pool = matches.length ? matches : [];
  return pool.find((a) => a.kind === "screenshot") || pool[0] || null;
}

async function blobDataUrl(artifactId) {
  if (state.artifactBlobCache.has(artifactId)) return state.artifactBlobCache.get(artifactId);
  try {
    const dto = await call("read_artifact_blob", { artifactId });
    state.artifactBlobCache.set(artifactId, dto.dataUrl);
    return dto.dataUrl;
  } catch (_) {
    state.artifactBlobCache.set(artifactId, null);
    return null;
  }
}

function renderTimeline() {
  const track = $("timelineTrack");
  const steps = state.activeRunSteps;
  if (!steps.length) {
    track.innerHTML = "";
    $("timelineDetail").innerHTML = `<span style="color: var(--muted); font-size: 11px">运行或单步执行后，每个 step 会出现在这里。</span>`;
    return;
  }
  const max = Math.max(...steps.map((s) => s.durationMs || 1));
  track.innerHTML = steps
    .map((s, idx) => {
      const w = Math.max(20, ((s.durationMs || 1) / max) * 100);
      const left = ((idx / Math.max(1, steps.length - 1)) * (100 - 8));
      const aiHealed = s.state === "ai_healed" || !!aiTraceForStep(s);
      const klass = [
        s.state === "failed" ? "is-failed" : s.state === "skipped" ? "is-skipped" : "",
        aiHealed ? "is-ai" : "",
      ].filter(Boolean).join(" ");
      const titleAi = aiHealed ? " · ✨ AI" : "";
      return `<div class="timeline-step ${klass}" data-idx="${idx}" style="left: ${left}%; width: ${Math.min(8, 100 - left)}%" title="${html(s.path)} · ${s.state}${titleAi}"></div>`;
    })
    .join("");
  track.querySelectorAll(".timeline-step").forEach((el) => {
    el.addEventListener("click", () => showStepDetail(Number(el.dataset.idx)));
  });
  showStepDetail(0);
}

function showStepDetail(idx) {
  const step = state.activeRunSteps[idx];
  if (!step) return;
  state.activeStepRun = step;
  $$(".timeline-step").forEach((el, i) => el.classList.toggle("is-active", i === idx));
  const art = artifactForStep(step);
  const aiTrace = aiTraceForStep(step);
  const aiBadge = aiTrace
    ? `<div class="kv"><span>AI</span><strong class="ai-heal-badge">✨ ${html(AI_HELPER_LABEL[aiTrace.helper] || aiTrace.helper || "AI heal")}${
        typeof aiTrace.confidence === "number" ? ` · ${(aiTrace.confidence * 100).toFixed(0)}%` : ""
      }</strong></div>${
        aiTrace.healed_selector
          ? `<div class="kv"><span>新选择器</span><strong>${html(aiTrace.healed_selector)}</strong></div>`
          : ""
      }${
        aiTrace.reasoning
          ? `<div class="kv"><span>AI 理由</span><strong style="font-weight: 400">${html(aiTrace.reasoning)}</strong></div>`
          : ""
      }`
    : "";
  $("timelineDetail").innerHTML = `
    <div class="kv"><span>节点</span><strong>${html(step.path)}</strong></div>
    <div class="kv"><span>状态</span><strong style="color: ${step.state === "failed" ? "var(--bad)" : "var(--ok)"}">${html(step.state)}</strong></div>
    <div class="kv"><span>耗时</span><strong>${html(step.durationMs ?? 0)} ms</strong></div>
    ${aiBadge}
    ${step.error ? `<div class="kv"><span>错误</span><strong style="color: var(--bad)">${html(step.error)}</strong></div>` : ""}
    ${art ? `<div class="timeline-screenshot" id="timelineShot"><div class="shot-loading">加载快照…</div></div>` : ""}
    ${step.outputJson ? `<pre>${html(pretty(step.outputJson))}</pre>` : ""}
  `;
  if (art) {
    blobDataUrl(art.id).then((url) => {
      const box = $("timelineShot");
      if (!box) return;
      if (!url) {
        box.innerHTML = `<div class="shot-loading">快照不可用</div>`;
        return;
      }
      if (art.mime && art.mime.startsWith("image/")) {
        box.innerHTML = `<img src="${url}" alt="step ${html(step.path)} screenshot" loading="lazy" />
          <div class="shot-caption">${html(art.kind)} · ${html(step.path)}</div>`;
      } else if (art.mime === "text/html") {
        box.innerHTML = `<iframe src="${url}" sandbox=""></iframe>
          <div class="shot-caption">${html(art.kind)} · ${html(step.path)}</div>`;
      } else {
        box.innerHTML = `<div class="shot-caption">${html(art.kind)} · ${html(art.mime)} (${html(art.size)} bytes)</div>`;
      }
    });
  }
}

// ─── Runs history ──────────────────────────────────────────────────────────

async function refreshRuns() {
  state.runs = await call("list_runs", { limit: 60 });
  const list = $("runsList");
  if (!state.runs.length) {
    list.innerHTML = `<div class="prop-empty">尚无运行记录</div>`;
    return;
  }
  list.innerHTML = state.runs
    .map(
      (r) => `<div class="run-row ${r.id === state.activeRun?.id ? "is-active" : ""}" data-run-id="${html(r.id)}">
      <div>
        <div class="title">${html(r.flowId)} · ${html(r.state)}</div>
        <div class="id">${html(r.id)} · ${html(r.durationMs ?? "-")}ms · ${html(r.triggerKind)}</div>
      </div>
      <span class="status-badge ${r.state === "ok" ? "ready" : r.state === "failed" ? "planned" : "partial"}">${html(r.state)}</span>
    </div>`
    )
    .join("");
  list.onclick = (e) => {
    const row = e.target.closest("[data-run-id]");
    if (row) showHistoricalRun(row.dataset.runId);
  };
}

async function showHistoricalRun(runId) {
  const detail = await call("show_run", { runId });
  state.activeRun = detail.run;
  state.activeRunSteps = detail.steps;
  // X-10: fetch AI cost rows in parallel; tolerate the storage layer not having
  // any (older runs predate ai_calls). X-07: also pull blob artifacts so the
  // replay scrubber can show per-step screenshots.
  let aiCalls = [];
  try { aiCalls = await call("run_cost", { runId }); } catch (_) {}
  await loadArtifacts(runId);
  $$("#runsList .run-row").forEach((el) => el.classList.toggle("is-active", el.dataset.runId === runId));
  const tokenTotal = (detail.run.costToken || 0).toLocaleString();
  const usd = ((detail.run.costUsdMicro || 0) / 1_000_000).toFixed(4);
  const costRows = aiCalls
    .map(
      (c) => `<tr style="border-top: 1px solid var(--line)">
        <td style="font-family:'SF Mono',Consolas,monospace">${html(c.step_id || "-")}</td>
        <td>${html(c.provider)}</td>
        <td>${html(c.model)}</td>
        <td style="text-align:right">${c.input_tokens}</td>
        <td style="text-align:right">${c.output_tokens}</td>
        <td style="text-align:right">${c.latency_ms}ms</td>
        <td style="text-align:right">$${(c.cost_usd_micro / 1_000_000).toFixed(4)}</td>
      </tr>`
    )
    .join("");
  const costBlock = aiCalls.length
    ? `
    <div class="section-title" style="margin-top: 10px">AI 用量 (X-10)</div>
    <table style="width: 100%; font-size: 11.5px; border-collapse: collapse">
      <thead><tr><th style="text-align:left">step</th><th style="text-align:left">provider</th><th style="text-align:left">model</th><th style="text-align:right">in</th><th style="text-align:right">out</th><th style="text-align:right">latency</th><th style="text-align:right">USD</th></tr></thead>
      <tbody>${costRows}</tbody>
    </table>`
    : "";
  $("runDetail").innerHTML = `
    <div class="kv-list" style="padding: 0">
      <div class="kv"><span>流程</span><strong>${html(detail.run.flowId)} @ ${html(detail.run.flowVersion)}</strong></div>
      <div class="kv"><span>状态</span><strong>${html(detail.run.state)}</strong></div>
      <div class="kv"><span>开始时间</span><strong>${html(detail.run.startedAt || "-")}</strong></div>
      <div class="kv"><span>结束时间</span><strong>${html(detail.run.finishedAt || "-")}</strong></div>
      <div class="kv"><span>耗时</span><strong>${html(detail.run.durationMs ?? "-")} ms</strong></div>
      <div class="kv"><span>AI Token</span><strong>${tokenTotal}</strong></div>
      <div class="kv"><span>AI 费用</span><strong>$${usd}</strong></div>
    </div>
    <div class="section-title" style="margin-top: 10px">节点明细</div>
    <table style="width: 100%; font-size: 11.5px; border-collapse: collapse">
      <thead><tr><th style="text-align: left">#</th><th style="text-align: left">路径</th><th>状态</th><th>毫秒</th></tr></thead>
      <tbody>
        ${detail.steps
          .map(
            (s) => `<tr style="border-top: 1px solid var(--line)">
          <td>${s.seq}</td>
          <td style="font-family: 'SF Mono', Consolas, monospace">${"&nbsp;".repeat(Math.max(0, s.depth) * 2)}${html(s.path)}</td>
          <td style="text-align: center"><span class="status-badge ${s.state === "ok" ? "ready" : s.state === "failed" ? "planned" : "partial"}">${html(s.state)}</span></td>
          <td style="text-align: right">${s.durationMs ?? 0}</td>
        </tr>`
          )
          .join("")}
      </tbody>
    </table>
    ${costBlock}
    <pre style="margin-top: 10px; background: var(--surface-soft); padding: 10px; border-radius: 8px; font-size: 11px; max-height: 220px; overflow: auto">${html(pretty(detail.run.outputs))}</pre>
  `;
}

// ─── Providers ─────────────────────────────────────────────────────────────

async function refreshProviders() {
  state.providers = await call("provider_status");
  renderProviderList();
  refreshActiveProviderPill();
}

function refreshActiveProviderPill() {
  const pill = $("activeProviderPill");
  if (!state.providers) return;
  const active = state.providers.active;
  pill.querySelector(".dot")?.style?.setProperty("background", active ? "var(--ok)" : "var(--warn)");
  pill.lastChild.textContent = active ? `模型源 · ${active}` : "未激活模型源";
  // Net pill
  const net = $("netPill");
  net.querySelector(".dot")?.style?.setProperty("background", state.providers.networkEnabled ? "var(--ok)" : "var(--warn)");
  net.classList.toggle("warn", !state.providers.networkEnabled);
  net.lastChild.textContent = state.providers.networkEnabled ? "LLM 网络: 已开启" : "LLM 网络: 未开启";
}

function renderProviderList() {
  const list = $("providerList");
  const profiles = state.providers?.profiles || [];
  if (!profiles.length) {
    list.innerHTML = `<div class="prop-empty">尚未配置模型源。点击右上角 "+ 新增" 或 "重置为默认"。</div>`;
    return;
  }
  list.innerHTML = profiles
    .map((p) => {
      const isActive = state.providers.active === p.name;
      const keyState = p.hasKey ? '<span class="status-badge ready">key ✓</span>' : '<span class="status-badge partial">key ✗</span>';
      return `<div class="provider-card ${isActive ? "is-active" : ""}">
        <div class="provider-card-head">
          <span class="name">${html(p.name)}</span>
          ${isActive ? '<span class="status-badge ready">active</span>' : ""}
          ${keyState}
          <span class="meta">${html(p.kind)}${p.wireApi ? ` / ${p.wireApi}` : ""}</span>
        </div>
        <div class="provider-card-row"><span>default</span><span>${html(p.defaultModel || "—")}</span></div>
        <div class="provider-card-row"><span>base_url</span><span>${html(p.baseUrl || "—")}</span></div>
        <div class="provider-card-row"><span>api_key_env</span><span>${html(p.apiKeyEnv || (p.hasInlineKey ? "(inline)" : "—"))}</span></div>
        <div class="provider-card-actions">
          <button data-edit-provider="${html(p.name)}">编辑</button>
          <button data-use-provider="${html(p.name)}">设为默认</button>
          <button class="danger" data-remove-provider="${html(p.name)}">删除</button>
        </div>
      </div>`;
    })
    .join("");
  list.querySelectorAll("[data-edit-provider]").forEach((b) =>
    b.addEventListener("click", () => openProviderEditor(b.dataset.editProvider))
  );
  list.querySelectorAll("[data-use-provider]").forEach((b) =>
    b.addEventListener("click", async () => {
      try {
        state.providers = await call("use_provider", { name: b.dataset.useProvider });
        renderProviderList();
        refreshActiveProviderPill();
        toast("已切换默认模型源", b.dataset.useProvider, "ok");
      } catch (e) { toast("切换失败", String(e), "bad"); }
    })
  );
  list.querySelectorAll("[data-remove-provider]").forEach((b) =>
    b.addEventListener("click", async () => {
      if (!confirm(`删除模型源 "${b.dataset.removeProvider}"?`)) return;
      try {
        state.providers = await call("remove_provider", { name: b.dataset.removeProvider });
        renderProviderList();
        refreshActiveProviderPill();
        toast("已删除", b.dataset.removeProvider, "ok");
      } catch (e) { toast("删除失败", String(e), "bad"); }
    })
  );
}

function openProviderEditor(name) {
  const existing = state.providers.profiles.find((p) => p.name === name);
  state.providerDraft = existing
    ? JSON.parse(JSON.stringify(existing))
    : {
        name: "",
        kind: "openai",
        wireApi: "chat",
        baseUrl: "",
        apiKey: "",
        apiKeyEnv: "",
        defaultModel: "",
        reasoningEffort: "",
        models: [],
        headers: {},
        notes: "",
      };
  renderProviderEditor();
}

function renderProviderEditor() {
  const d = state.providerDraft;
  $("providerEditorTitle").textContent = d?.name ? `编辑 · ${d.name}` : "新建模型源";
  if (!d) {
    $("providerEditBody").innerHTML = `<div class="prop-empty">从左侧选择一个模型源，或点击右上角"+ 新增"</div>`;
    return;
  }
  $("providerEditBody").innerHTML = `
    <div class="row">
      <div class="field"><label>name</label><input id="pName" value="${html(d.name)}" placeholder="如 deepseek / claude / azure-east" /></div>
      <div class="field"><label>kind</label>
        <select id="pKind">
          <option value="openai" ${d.kind === "openai" ? "selected" : ""}>OpenAI 兼容</option>
          <option value="anthropic" ${d.kind === "anthropic" ? "selected" : ""}>Anthropic</option>
        </select>
      </div>
    </div>
    <div class="row three">
      <div class="field"><label>wire_api</label>
        <select id="pWire">
          <option value="">—</option>
          <option value="chat" ${d.wireApi === "chat" ? "selected" : ""}>chat (/chat/completions)</option>
          <option value="responses" ${d.wireApi === "responses" ? "selected" : ""}>responses (Responses API)</option>
        </select>
      </div>
      <div class="field"><label>default_model</label><input id="pModel" value="${html(d.defaultModel || "")}" placeholder="gpt-4o-mini / claude-opus-4-7" /></div>
      <div class="field"><label>reasoning_effort</label>
        <select id="pEffort">
          <option value="">—</option>
          ${["low", "medium", "high"].map((v) => `<option ${d.reasoningEffort === v ? "selected" : ""}>${v}</option>`).join("")}
        </select>
      </div>
    </div>
    <div class="field"><label>base_url</label><input id="pBase" value="${html(d.baseUrl || "")}" placeholder="https://api.example.com/v1" /></div>
    <div class="row">
      <div class="field"><label>api_key_env</label><input id="pEnv" value="${html(d.apiKeyEnv || "")}" placeholder="如 OPENAI_API_KEY" /></div>
      <div class="field"><label>api_key (内联，谨慎)</label><input type="password" id="pInline" value="${html(d.apiKey || "")}" placeholder="留空优先使用环境变量" /></div>
    </div>
    <div class="field"><label>models (可选 · 逗号分隔)</label><input id="pModels" value="${html((d.models || []).join(", "))}" placeholder="gpt-4o, gpt-4o-mini" /></div>
    <div class="field">
      <label>额外 headers</label>
      <div class="headers-editor" id="pHeaders"></div>
      <button id="addHeaderBtn" style="margin-top: 4px">+ 添加 header</button>
    </div>
    <div class="field"><label>备注</label><textarea id="pNotes" style="min-height: 50px">${html(d.notes || "")}</textarea></div>
    <label class="toggle"><input type="checkbox" id="pActivate" ${d.name === state.providers?.active ? "checked" : ""}/> 保存后设为默认</label>
  `;
  renderHeadersEditor();
  $("addHeaderBtn").addEventListener("click", () => {
    const k = prompt("Header 名");
    if (!k) return;
    state.providerDraft.headers[k] = "";
    renderHeadersEditor();
  });
}

function renderHeadersEditor() {
  const wrap = $("pHeaders");
  const entries = Object.entries(state.providerDraft.headers || {});
  if (!entries.length) {
    wrap.innerHTML = `<div style="font-size: 11px; color: var(--faint)">尚无 header</div>`;
    return;
  }
  wrap.innerHTML = entries
    .map(
      ([k, v], i) => `<div class="header-row">
      <input value="${html(k)}" data-hk="${i}" />
      <input value="${html(v)}" data-hv="${i}" />
      <button class="icon danger" data-hd="${i}">×</button>
    </div>`
    )
    .join("");
  wrap.querySelectorAll("[data-hd]").forEach((b) =>
    b.addEventListener("click", () => {
      const idx = Number(b.dataset.hd);
      const key = Object.keys(state.providerDraft.headers)[idx];
      delete state.providerDraft.headers[key];
      renderHeadersEditor();
    })
  );
  wrap.querySelectorAll("[data-hk]").forEach((inp) =>
    inp.addEventListener("change", () => {
      const idx = Number(inp.dataset.hk);
      const entries = Object.entries(state.providerDraft.headers);
      const [oldK, oldV] = entries[idx];
      const newK = inp.value.trim();
      if (newK && newK !== oldK) {
        delete state.providerDraft.headers[oldK];
        state.providerDraft.headers[newK] = oldV;
        renderHeadersEditor();
      }
    })
  );
  wrap.querySelectorAll("[data-hv]").forEach((inp) =>
    inp.addEventListener("change", () => {
      const idx = Number(inp.dataset.hv);
      const key = Object.keys(state.providerDraft.headers)[idx];
      state.providerDraft.headers[key] = inp.value;
    })
  );
}

function collectProviderDraft() {
  const d = state.providerDraft;
  d.name = $("pName").value.trim();
  d.kind = $("pKind").value;
  d.wireApi = $("pWire").value || null;
  d.defaultModel = $("pModel").value.trim();
  d.reasoningEffort = $("pEffort").value || null;
  d.baseUrl = $("pBase").value.trim();
  d.apiKeyEnv = $("pEnv").value.trim();
  d.apiKey = $("pInline").value;
  d.models = $("pModels").value.split(",").map((s) => s.trim()).filter(Boolean);
  d.notes = $("pNotes").value;
  d.activate = $("pActivate").checked;
  return d;
}

async function saveProvider() {
  if (!state.providerDraft) return;
  const draft = collectProviderDraft();
  if (!draft.name) { toast("名称不能为空", "", "warn"); return; }
  try {
    state.providers = await call("save_provider", { profile: draft });
    renderProviderList();
    refreshActiveProviderPill();
    toast("已保存模型源", draft.name, "ok");
  } catch (e) {
    toast("保存失败", String(e), "bad");
  }
}

async function testProvider() {
  if (!state.providerDraft?.name) { toast("先保存 / 选择模型源", "", "warn"); return; }
  const r = await call("test_provider", { name: state.providerDraft.name });
  if (r.ok) {
    toast("✓ 测试通过", `${r.provider}/${r.model} · ${r.inputTokens}↑/${r.outputTokens}↓`, "ok");
  } else {
    toast("✗ 测试失败", r.error || "unknown", "bad");
  }
}

// ─── Recorder ──────────────────────────────────────────────────────────────

let recorderTick = null;
let recorderStartedAt = 0;
let recorderEventCount = 0;
let recorderEventUnlisten = null;

async function refreshRecorder() {
  const status = await call("recorder_status");
  applyRecorderStatus(status);
}

function applyRecorderStatus(status) {
  state.recorder = status;
  $("recorderPill").classList.toggle("is-on", !!status.recording);
  $("recorderPillText").textContent = status.recording
    ? `录制中 · ${status.target || "browser"}`
    : "未录制";
  $("recorderStartBtn").disabled = !!status.recording;
  $("recorderStopBtn").disabled = !status.recording;
  $("recorderNote").innerHTML = html(status.note || "");
  const backendEl = $("recorderBackend");
  if (backendEl) backendEl.textContent = status.backend || "—";
  const elapsedEl = $("recorderStatElapsed");
  const eventsEl = $("recorderStatEvents");
  if (elapsedEl && eventsEl) {
    elapsedEl.classList.toggle("is-pulsing", !!status.recording);
    eventsEl.classList.toggle("is-pulsing", !!status.recording);
  }
  if (status.recording) {
    if (!recorderTick) {
      recorderStartedAt = status.started_at ? Date.parse(status.started_at) : Date.now();
      recorderEventCount = 0;
      $("recorderEvents").textContent = "0";
      recorderStreamReset(status.target || "browser", status.backend || "");
      recorderTick = setInterval(updateRecorderElapsed, 1000);
    }
  } else if (recorderTick) {
    clearInterval(recorderTick);
    recorderTick = null;
    recorderStreamAppend("muted", "[idle] 录制已停止");
  }
  // Always refresh elapsed display so static "00:00" is shown when idle.
  updateRecorderElapsed();
}

function updateRecorderElapsed() {
  const el = $("recorderElapsed");
  if (!el) return;
  if (!state.recorder?.recording) { el.textContent = "00:00"; return; }
  const sec = Math.max(0, Math.floor((Date.now() - recorderStartedAt) / 1000));
  const mm = String(Math.floor(sec / 60)).padStart(2, "0");
  const ss = String(sec % 60).padStart(2, "0");
  el.textContent = `${mm}:${ss}`;
}

function pad2(n) { return String(n).padStart(2, "0"); }

function recorderStreamReset(target, backend) {
  const box = $("recorderStream");
  if (!box) return;
  box.innerHTML = `<div class="head">▶ live event stream · target=${html(target)} · backend=${html(backend || "—")}</div>`;
  recorderStreamAppend("muted", `[init] session 已启动 · 等待事件…`);
  recorderStreamAppend("muted", `[note] CDP Runtime.addBinding 已注入 · click/input/change/keydown 实时透传 · ActionBuffer 在 stop 时合并 · Alt+点击 抓取同款`);
}

function recorderStreamAppend(cls, text) {
  const box = $("recorderStream");
  if (!box) return;
  const ts = new Date().toLocaleTimeString("zh-CN", { hour12: false });
  const line = document.createElement("div");
  line.className = `line ${cls}`;
  line.innerHTML = `<span class="ts">${html(ts)}</span>${html(text)}`;
  box.appendChild(line);
  box.scrollTop = box.scrollHeight;
}

function summarizeSelector(s) {
  if (!s) return "?";
  return s.length > 60 ? s.slice(0, 57) + "…" : s;
}

function onLiveRecorderEvent(evt) {
  // evt = { source, kind, atMs|at_ms, payload }
  const kind = evt?.kind || "event";
  const source = evt?.source || "?";
  const payload = evt?.payload || {};
  const summary = (() => {
    if (kind === "navigate" && payload.url) return `→ ${payload.url}`;
    if (kind === "launched") return payload.msg || "browser launched";
    if (kind === "heartbeat") return `tick #${payload.n ?? "?"}`;
    if (kind === "binding_ready") return `binding ready @ ${payload.url || ""}`;
    if (kind === "click") return `🖱  ${summarizeSelector(payload.selector)}${payload.label ? "  ·  " + payload.label : ""}`;
    if (kind === "input") return `⌨️  ${summarizeSelector(payload.selector)} = ${JSON.stringify(payload.value ?? "")}`;
    if (kind === "change") return `🔁  ${summarizeSelector(payload.selector)} = ${JSON.stringify(payload.value ?? "")}`;
    if (kind === "keydown") return `⌨️  key=${payload.key} on ${summarizeSelector(payload.selector)}`;
    if (kind === "similar_grab") {
      const cnt = payload.sibling_count ?? "?";
      const sample = Array.isArray(payload.sample_values) ? payload.sample_values.slice(0, 3).join(" / ") : "";
      return `📋 alt-click → ${cnt} 个同款 · ${summarizeSelector(payload.generalized_selector)}${sample ? "  ·  " + sample : ""}`;
    }
    if (kind === "bind_error") return `bind error: ${payload.error || ""}`;
    try { return JSON.stringify(payload); } catch { return String(payload); }
  })();
  const cls = (() => {
    if (kind === "heartbeat") return "warn";
    if (kind === "similar_grab") return "ok";
    if (kind === "click" || kind === "input" || kind === "change") return "ok";
    if (kind === "bind_error") return "bad";
    return "tick";
  })();
  if (kind !== "heartbeat" && kind !== "binding_ready") {
    recorderEventCount += 1;
    const el = $("recorderEvents"); if (el) el.textContent = String(recorderEventCount);
  }
  const sec = Math.max(0, Math.floor((Date.now() - (recorderStartedAt || Date.now())) / 1000));
  recorderStreamAppend(cls, `[${pad2(sec)}s] ${source}.${kind.padEnd(12)} · ${summary}`);
}

async function ensureRecorderListener() {
  if (recorderEventUnlisten) return;
  try {
    const listenFn = window.__TAURI__?.event?.listen;
    if (!listenFn) return;
    recorderEventUnlisten = await listenFn("lumo://recorder-event", (e) => {
      onLiveRecorderEvent(e?.payload);
    });
  } catch (err) {
    console.warn("recorder listen failed", err);
  }
}

async function startRecording() {
  try {
    await ensureRecorderListener();
    const status = await call("recorder_start", { target: $("recorderTarget").value });
    applyRecorderStatus(status);
    toast("已开始录制", `${state.recorder.target || "browser"} · ${state.recorder.backend || ""}`, "ok");
  } catch (e) {
    toast("启动失败", String(e), "bad");
  }
}

async function stopRecording() {
  try {
    const result = await call("recorder_stop");
    recorderStreamAppend("tick", `[done] ${result.events} 事件 · ${result.note}`);
    renderRecorderPatch(result.yamlHint || "");
    toast("录制结束", `${result.events} 事件 · 看下方 YAML 草稿`, "ok");
    await refreshRecorder();
  } catch (e) {
    toast("停止失败", String(e), "bad");
  }
}

// Captured YAML patch waiting to be merged into the active flow. Lives as
// long as the recorder view is open; cleared when the user merges or
// dismisses it.
let pendingRecorderPatch = "";

function renderRecorderPatch(yaml) {
  pendingRecorderPatch = yaml;
  const box = $("recorderPatch");
  if (!box) return;
  const stripped = (yaml || "").trim();
  const hasSteps = stripped && !stripped.includes("no actionable events were captured");
  if (!hasSteps) {
    box.innerHTML = `<div class="muted" style="padding:8px 10px">本次录制没有产生可合并的步骤。继续操作浏览器，或检查 Chromium 是否已正确启动。</div>`;
    return;
  }
  box.innerHTML = `
    <div class="recorder-patch-head">
      <span>▶ Recorder YAML patch · 可粘贴到 spec.steps</span>
      <div style="display:flex;gap:6px">
        <button id="recorderPatchCopyBtn" title="复制到剪贴板">📋 复制</button>
        <button id="recorderPatchSaveBtn" title="另存为一个新流程文件，进入「录制产物」">💾 另存为新流程</button>
        <button class="primary" id="recorderPatchInsertBtn" title="追加到当前打开的流程末尾">⤓ 插入到当前流程</button>
      </div>
    </div>
    <pre class="recorder-patch-body"><code>${html(yaml)}</code></pre>
  `;
  const copy = $("recorderPatchCopyBtn");
  if (copy) copy.onclick = () => {
    navigator.clipboard.writeText(pendingRecorderPatch).then(
      () => toast("已复制", "Recorder YAML patch", "ok"),
      (e) => toast("复制失败", String(e), "bad"),
    );
  };
  const insert = $("recorderPatchInsertBtn");
  if (insert) insert.onclick = () => insertRecorderPatchIntoFlow();
  const save = $("recorderPatchSaveBtn");
  if (save) save.onclick = () => saveRecorderPatchAsFlow();
}

async function saveRecorderPatchAsFlow() {
  if (!pendingRecorderPatch.trim()) {
    toast("没有可保存内容", "先录制一段操作", "bad");
    return;
  }
  const proposed = `rec-${new Date().toISOString().slice(0, 19).replace(/[T:]/g, "-")}`;
  const name = prompt("保存为流程名（不带扩展名）：", proposed);
  if (!name) return;
  try {
    const path = await call("save_recording_as_flow", {
      name,
      yamlHint: pendingRecorderPatch,
    });
    await refreshFlows();
    await loadFlow(path);
    pendingRecorderPatch = "";
    const box = $("recorderPatch");
    if (box) box.innerHTML = `<div class="muted" style="padding:8px 10px">已保存到 ${html(path)}。可继续录制或返回设计页编辑。</div>`;
    toast("已保存到流程库", path, "ok");
  } catch (e) {
    toast("保存失败", String(e), "bad");
  }
}

function insertRecorderPatchIntoFlow() {
  if (!pendingRecorderPatch.trim()) {
    toast("没有可插入内容", "先录制一段操作", "bad");
    return;
  }
  if (!state.flowPath) {
    toast("请先打开流程", "录制结果会追加到当前编辑的流程末尾", "bad");
    return;
  }
  // Strip the YAML header comments (everything before the first list dash) so
  // the patch slots cleanly under `spec.steps`. Indent every line two spaces
  // to match the existing spec.steps indentation.
  const lines = pendingRecorderPatch.split("\n");
  const firstStepIdx = lines.findIndex((l) => /^\s*-\s+/.test(l));
  const stepsBlock = firstStepIdx >= 0 ? lines.slice(firstStepIdx) : lines;
  const indented = stepsBlock
    .filter((l) => l.length > 0)
    .map((l) => "  " + l)
    .join("\n");
  const banner = "\n  # === recorder patch (review before keeping) ===\n";
  const newSource = (state.source || "").replace(/\n*$/, "") + banner + indented + "\n";
  state.source = newSource;
  try {
    state.ast = parseYaml(state.source);
  } catch (e) {
    toast("YAML 解析失败", String(e), "bad");
    return;
  }
  // Mirror the source back into the open code editor if any.
  const codeEl = $("codeArea");
  if (codeEl) codeEl.value = newSource;
  if (typeof renderActiveView === "function") renderActiveView();
  toast("已合并", `Recorder patch 已追加到 ${state.flowPath}`, "ok");
  pendingRecorderPatch = "";
  const box = $("recorderPatch");
  if (box) box.innerHTML = `<div class="muted" style="padding:8px 10px">已合并到 ${html(state.flowPath)}。可继续录制以追加更多步骤；记得 💾 保存。</div>`;
}

// ─── Settings ──────────────────────────────────────────────────────────────

async function refreshSettings() {
  const [info, skills] = await Promise.all([call("app_info"), call("list_skills")]);
  state.app = info;
  $("environmentBox").innerHTML = [
    kv("版本", info.version),
    kv("平台", `${info.platform} ${info.arch}`),
    kv("应用数据", info.dataDir),
    kv("Providers", info.providersPath),
    kv("Skills 根", info.skillsPath),
    kv("Examples", info.examplesDir || "-"),
    kv("LLM 网络", info.networkEnabled ? "已开启 (LUMO_ALLOW_LLM_NETWORK=1)" : "未开启"),
  ].join("");
  $("skillsBox").innerHTML = skills.length
    ? skills.map((s) => kv(s.name, s.description || s.source)).join("")
    : `<div class="kv"><span>暂无</span><strong>把 SKILL.md 放到 ${html(info.skillsPath)}</strong></div>`;
  $("appMeta").textContent = `${info.platform} ${info.arch} · v${info.version}`;
  $("versionPill").lastChild.textContent = `v${info.version}`;
}

function kv(label, value) {
  return `<div class="kv"><span>${html(label)}</span><strong title="${html(value)}">${html(value)}</strong></div>`;
}

// ─── Feature map ───────────────────────────────────────────────────────────

async function loadFeatureMap() {
  state.features = await call("feature_map");
}

function renderFeatures() {
  const grid = $("featureGrid");
  if (!state.features.length) { grid.innerHTML = ""; return; }
  grid.innerHTML = state.features
    .map(
      (sec) => `<div class="feature-section">
      <h3>${html(sec.title)}</h3>
      <div class="feature-list">
        ${sec.items
          .map(
            (it) => `<div class="feature-item">
            <span class="fid">${html(it.id)}</span>
            <div>
              <div class="ftitle">${html(it.title)}</div>
              <span class="fnote">${html(it.note)} · ${html(it.stage)}</span>
            </div>
            <span class="status-badge ${html(it.status)}">${html(it.status)}</span>
          </div>`
          )
          .join("")}
      </div>
    </div>`
    )
    .join("");
}

// ─── Boot + event wiring ───────────────────────────────────────────────────

function reportError(error) {
  setStatus("操作失败", "bad");
  toast("操作失败", String(error), "bad");
}

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
