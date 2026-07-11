// Shared mutable singletons. Every panel reads/writes this one `state` object,
// matching the original monolith's single global. `graph` holds the Graph
// view's pan/zoom transform.

function loadJson(key, fallback) {
  try {
    return JSON.parse(localStorage.getItem(key) || "") ?? fallback;
  } catch {
    return fallback;
  }
}

function loadAlpha(key, fallback, legacyDefaults = []) {
  const raw = localStorage.getItem(key);
  const legacy = Array.isArray(legacyDefaults) ? legacyDefaults : [legacyDefaults];
  if (raw === null || legacy.map(String).includes(raw)) return fallback;
  const value = Number(raw);
  return Number.isFinite(value) ? value : fallback;
}

export const state = {
  app: null,
  examples: [],
  actions: [],
  actionsByFamily: new Map(),
  actionFamilyCollapsed: loadJson("lumo.actionFamilies", {}),
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
  breakpoints: new Set(),     // F-20: step *id-chain paths* (parentId/childId) marked as breakpoints
  debugRunId: null,           // F-20: id of the current paused debug run (for step/continue)
  debugPausedAt: null,        // F-20: step path the debug run is currently paused before
  providers: null,
  providerDraft: null,
  ocrModels: [],
  features: [],
  viewMode: "steps",
  currentView: "design",
  capabilityHubMounted: false,
  rightSection: "inspector",
  windowAlpha: loadAlpha("lumo.win", 0, 18),
  panelAlpha: loadAlpha("lumo.panel", 14, [18, 38, 62]),
  theme: localStorage.getItem("lumo.theme") || "auto",
  recorder: { recording: false, target: null, startedAt: null },
  schemaCache: new Map(),
  elTab: "elements",
  elements: [
    {
      id: "el_login_username",
      label: "用户名输入框",
      group: "登录页",
      automation: "web",
      scope: "cloud",
      syncState: "synced",
      owner: "Studio",
      source: "https://example.com/login",
      tag: "input",
      role: "textbox",
      cloudGroup: { id: "cg_login", name: "示例登录云元素组", permission: "edit" },
      usedIn: ["browser.type"],
      lastValidated: "2026-06-04 09:40",
      fingerprints: {
        id: "username",
        data_testid: "login-username",
        css: "form.login input[name='username']",
        aria_label: "Username",
        text_includes: "Username",
        xpath: "//form[@class='login']//input[@name='username']",
        a11y: "TextField[name='Username']",
        visual: "anchor:topRight(login-card)",
      },
    },
    {
      id: "el_login_submit",
      label: "登录按钮",
      group: "登录页",
      automation: "web",
      scope: "cloud",
      syncState: "dirty",
      owner: "Studio",
      source: "https://example.com/login",
      tag: "button",
      role: "button",
      cloudGroup: { id: "cg_login", name: "示例登录云元素组", permission: "edit" },
      usedIn: ["browser.click"],
      lastValidated: "2026-06-04 09:41",
      similar: ["el_login_submit_cn", "el_login_submit_en"],
      fingerprints: {
        data_testid: "login-submit",
        css: "form.login button[type='submit']",
        aria_label: "登录",
        text_includes: "登录",
        xpath: "//form[@class='login']//button[@type='submit']",
        a11y: "Button[name='登录']",
        visual: "anchor:bottomRight(login-card)",
      },
    },
    {
      id: "el_h1",
      label: "页面主标题 H1",
      group: "首页",
      automation: "web",
      scope: "local",
      syncState: "local",
      owner: "本机",
      source: "https://example.com",
      tag: "h1",
      role: "heading",
      usedIn: [],
      lastValidated: "2026-06-03 17:18",
      fingerprints: {
        css: "main h1",
        text_includes: "Welcome",
        xpath: "//main//h1[1]",
        a11y: "Heading[level=1]",
        visual: "anchor:topCenter",
      },
    },
    {
      id: "el_desktop_search",
      label: "企业微信搜索框",
      group: "企业微信窗口",
      automation: "desktop",
      scope: "local",
      syncState: "local",
      owner: "本机",
      source: "WeCom.exe / MainWindow",
      tag: "edit",
      role: "searchbox",
      bounds: { x: 88, y: 78, w: 236, h: 32 },
      usedIn: [],
      lastValidated: "2026-06-04 10:05",
      fingerprints: {
        a11y: "Window[name='企业微信'] > Edit[name='搜索']",
        visual: "anchor:leftTop(search-panel)",
      },
    },
  ],
  images: [
    {
      id: "img_login_card",
      label: "登录卡片截图",
      group: "登录页",
      scope: "cloud",
      syncState: "synced",
      source: "https://example.com/login",
      capturedAt: "2025-12-04 14:32",
      thumbnail: null,
      hash: "phash:8b3f2c9a…",
      usedIn: ["image.locate"],
    },
  ],
  datatables: [
    {
      id: "tbl_orders",
      label: "订单列表结构",
      group: "示例数据",
      source: "browser.extract(all=true)",
      columns: ["order_id", "customer", "amount", "status"],
      rows: 128,
      capturedAt: "2026-06-04 09:55",
    },
  ],
};

// Graph view pan + zoom transform.
export const graph = {
  scale: 1,
  tx: 24,
  ty: 24,
};
