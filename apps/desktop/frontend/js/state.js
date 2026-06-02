// Shared mutable singletons. Every panel reads/writes this one `state` object,
// matching the original monolith's single global. `graph` holds the Graph
// view's pan/zoom transform.

export const state = {
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
  breakpoints: new Set(),     // F-20: step ids marked as breakpoints
  debugRunId: null,           // F-20: id of the current paused debug run (for step/continue)
  debugPausedAt: null,        // F-20: step path the debug run is currently paused before
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

// Graph view pan + zoom transform.
export const graph = {
  scale: 1,
  tx: 24,
  ty: 24,
};
