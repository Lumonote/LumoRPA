// i18n 基建（批次 I）：字符串表 + `t(key, params)`。约定：
//   * zh-CN 是源语言表（键全集）；其他语言缺键时回落 zh-CN；完全未知的键
//     返回键名本身（fail-visible，不出空白）。
//   * 键名用 `域.名称`（common.save / editor.undo / toast.saved…）。
//   * 参数用 `{name}` 占位。
//   * locale 持久化在 localStorage `lumo.locale`；无 localStorage 环境
//     （node --test）下安全降级为内存态。
// 静态 DOM 走 `data-i18n*` 属性 + `applyStaticTranslations()`；动态渲染直接
// 调 `t()`。切换语言用 `setLocale()`，订阅方通过 `subscribeLocale()` 重渲染。
// 现状：全量文案抽取尚未完成（src/js 内联中文 ≈ 千余处）——已迁移的是导航、
// Capability Hub、设置页与能力地图；其余模块仍是中文硬编码，en 下按键回落
// 显示中文，不会出现空白。

const zhCN = {
  "common.save": "保存",
  "common.cancel": "取消",
  "common.close": "关闭",
  "common.delete": "删除",
  "common.confirm": "确认",
  "common.loading": "加载中…",
  "common.ready": "就绪",
  "common.failed": "操作失败",
  "editor.undo": "撤销",
  "editor.redo": "重做",
  "editor.validate": "校验",
  "editor.run": "运行",
  "editor.debug": "调试",
  "editor.save_flow": "保存",
  "editor.history": "历史",
  "editor.undone": "已撤销",
  "editor.redone": "已重做",
  "editor.nothing_to_undo": "没有可撤销的编辑",
  "editor.nothing_to_redo": "没有可重做的编辑",
  "steps.copied": "已复制 {n} 个步骤",
  "steps.pasted": "已粘贴 {n} 个步骤",
  "steps.deleted": "已删除 {n} 个步骤",
  "steps.duplicated": "已创建步骤副本",
  "history.restored": "已回滚",

  // ── 顶部导航 ──────────────────────────────────────────────────────────
  "nav.design": "设计",
  "nav.recorder": "录制",
  "nav.runs": "运行",
  "nav.mission_control": "Mission Control",
  "nav.models": "模型",
  "nav.features": "能力地图",
  "nav.capability_hub": "能力中枢",
  "nav.settings": "设置",

  // ── 能力中枢 ──────────────────────────────────────────────────────────
  "hub.kicker": "LUMO 控制平面 / 01",
  "hub.title": "能力中枢",
  "hub.subtitle": "发现、治理并编排每个本地 Agent 可用的能力。",
  "hub.checking": "正在检查能力服务…",
  "hub.online": "能力服务在线",
  "hub.backend_error": "后端未连接：{detail}",
  "hub.nav_label": "能力中枢分区",
  "hub.nav.overview": "概览",
  "hub.nav.skills": "技能",
  "hub.nav.servers": "MCP 服务器",
  "hub.nav.catalog": "能力目录",
  "hub.nav.profiles": "Agent 画像",
  "hub.nav.jobs": "任务",
  "hub.nav.permissions": "权限",
  "hub.nav.audit": "审计",
  "hub.nav.proposals": "改进提案",
  "hub.metric.servers": "已连接服务器",
  "hub.metric.skills": "已安装技能",
  "hub.metric.profiles": "Agent 画像",
  "hub.metric.vault": "Vault 状态",
  "hub.metric.protected": "已保护",
  "hub.eyebrow.runtime_mesh": "运行时网格",
  "hub.eyebrow.operator_layer": "操作层",
  "hub.eyebrow.protocol_fabric": "协议层",
  "hub.eyebrow.local_intelligence": "本地智能",
  "hub.eyebrow.execution_policy": "执行策略",
  "hub.eyebrow.scheduler": "持久调度",
  "hub.eyebrow.evolution": "受监督演进",
  "hub.eyebrow.secure_ingestion": "安全接入",
  "hub.card.servers": "MCP 服务器",
  "hub.card.profiles": "Agent 画像",
  "hub.card.skills": "技能",
  "hub.card.jobs": "Agent 任务",
  "hub.card.proposals": "改进提案",
  "hub.proposals_note": "所有变更先评估、再人工批准，并以新版本应用。",
  "hub.empty.servers": "尚未配置 MCP 服务器",
  "hub.empty.skills": "尚未安装技能",
  "hub.empty.profiles": "尚未创建 Agent 画像",
  "hub.empty.hint": "连接或导入能力后，这里会显示内容。",
  "hub.unnamed_server": "未命名服务器",
  "hub.unnamed_skill": "未命名技能",
  "hub.unnamed_profile": "未命名画像",
  "hub.no_description": "暂无描述",
  "hub.tools_available": "{n} 个工具可用",
  "hub.legacy": "历史版本",
  "hub.default_model": "默认模型",
  "hub.budget_tools": "{n} 工具",
  "hub.budget_tokens": "{n} tokens",
  "hub.tool_schema": "工具 Schema",
  "hub.input_contract": "入参契约",
  "hub.action.test": "测试",
  "hub.action.discover": "发现工具",
  "hub.action.enable": "启用",
  "hub.action.disable": "停用",
  "hub.action.delete": "删除",
  "hub.action.validate": "校验",
  "hub.action.rollback": "回滚",
  "hub.action.import_server": "导入服务器",
  "hub.status.healthy": "健康",
  "hub.status.unhealthy": "异常",
  "hub.status.unknown": "未知",
  "hub.status.active": "启用中",
  "hub.status.disabled": "已停用",
  "hub.preview.servers_detected": "个服务器",
  "hub.preview.secrets": "个密钥",
  "hub.preview.conflicts": "处冲突",
  "hub.import.title": "导入 MCP 配置",
  "hub.import.note": "支持 JSON / JSONC / YAML / TOML；密钥仅写入 Vault，原值不会进入配置库。",
  "hub.import.mode.auto-discover": "自动发现",
  "hub.import.mode.file": "文件",
  "hub.import.mode.paste": "粘贴",
  "hub.import.mode.manual": "手动",
  "hub.import.preview": "预览",
  "hub.import.choose_source": "请选择导入来源",
  "hub.import.conflicts": "冲突",
  "hub.import.no_scan": "尚未扫描",
  "hub.import.connection_test": "连接测试",
  "hub.import.waiting": "等待中",
  "hub.import.selected": "已选择{mode} — 等待来源输入。",
  "hub.import.summary": "{servers} 个服务器 · {secrets} 个密钥",
  "hub.import.safe_preview": "安全预览",
  "hub.import.filename_label": "配置文件名",
  "hub.import.placeholder": "粘贴 Claude Desktop、Cursor、VS Code 或通用 MCP 配置",
  "hub.skill.local_path": "本地 SKILL.md 或目录路径",
  "hub.skill.import_local": "导入本地 Skill",
  "hub.skill.git_url": "Git 仓库地址",
  "hub.skill.git_revision": "revision（可选）",
  "hub.skill.import_git": "导入 Git Skill",
  "hub.coming.title": "策略就绪工作区",
  "hub.coming.note": "该界面已为后端数据与操作预留，命令接入前保持空态。",

  // ── 能力地图 ──────────────────────────────────────────────────────────
  "features.title": "能力地图 · 设计文档落地度",
  "features.subtitle": "基于 docs/01-Product-Design.md 功能矩阵",
  "features.note": "这一页对照设计文档列出所有功能模块的当前实现状态。<strong>ready</strong> = 已可在本地跑通；<strong>partial</strong> = 部分能力可用；<strong>planned</strong> = 当前里程碑未排期，但 DSL / 架构已为其预留接口。",

  // ── 设置 ──────────────────────────────────────────────────────────────
  "settings.title": "外观与运行环境",
  "settings.opacity": "透明度",
  "settings.window_base": "窗口底色",
  "settings.ui_panel": "界面面板",
  "settings.window_base_hint": "窗口底色不透明度 — 0 = 完全透过，看到真实桌面 / 100 = 不透明",
  "settings.ui_panel_hint": "界面面板不透明度 — 越低越能看到桌面上正在操作的内容",
  "settings.preset.glass": "透视操作",
  "settings.preset.frost": "半透明",
  "settings.preset.solid": "⬜ 实心",
  "settings.preset.invisible": "极简透视",
  "settings.opacity_note": "两个滑块互不影响：窗口底色直接驱动 Tauri 透明背景；界面面板控制首页、工具栏和卡片自身的不透明度，用于边看桌面边编排自动化。",
  "settings.theme": "主题",
  "settings.theme.auto": "🖥 跟随系统",
  "settings.theme.light": "☀️ 浅色",
  "settings.theme.dark": "🌙 深色",
  "settings.theme_note": "主题影响所有 token（前景/线/卡片/选中态），毛玻璃叠加层会自适应深浅。",
  "settings.environment": "运行环境",
  "settings.local_skills": "本地 Skills",
  "settings.voice_privacy": "桌面语音与隐私",
  "settings.update_center": "安全更新中心",
  "settings.language": "语言 / Language",
  "settings.language_note": "切换后立即生效并记住选择。尚未抽取文案的模块在英文下回落显示中文。",
  "settings.language.zh-CN": "简体中文",
  "settings.language.en": "English",
  "settings.autostart": "开机自启",
  "settings.autostart_on": "已开启：登录后自动启动",
  "settings.autostart_off": "已关闭",

  "test.zh_only": "只有中文",
  "test.greet": "你好 {name}，共 {n} 项",
};

const en = {
  "common.save": "Save",
  "common.cancel": "Cancel",
  "common.close": "Close",
  "common.delete": "Delete",
  "common.confirm": "Confirm",
  "common.loading": "Loading…",
  "common.ready": "Ready",
  "common.failed": "Operation failed",
  "editor.undo": "Undo",
  "editor.redo": "Redo",
  "editor.validate": "Validate",
  "editor.run": "Run",
  "editor.debug": "Debug",
  "editor.save_flow": "Save",
  "editor.history": "History",
  "editor.undone": "Undone",
  "editor.redone": "Redone",
  "editor.nothing_to_undo": "Nothing to undo",
  "editor.nothing_to_redo": "Nothing to redo",
  "steps.copied": "Copied {n} step(s)",
  "steps.pasted": "Pasted {n} step(s)",
  "steps.deleted": "Deleted {n} step(s)",
  "steps.duplicated": "Step duplicated",
  "history.restored": "Restored",

  "nav.design": "Design",
  "nav.recorder": "Recorder",
  "nav.runs": "Runs",
  "nav.mission_control": "Mission Control",
  "nav.models": "Models",
  "nav.features": "Capability Map",
  "nav.capability_hub": "Capability Hub",
  "nav.settings": "Settings",

  "hub.kicker": "LUMO CONTROL PLANE / 01",
  "hub.title": "Capability Hub",
  "hub.subtitle": "Discover, govern, and compose the intelligence available to every local agent.",
  "hub.checking": "Checking capability services…",
  "hub.online": "Capability services online",
  "hub.backend_error": "Backend not connected: {detail}",
  "hub.nav_label": "Capability Hub sections",
  "hub.nav.overview": "Overview",
  "hub.nav.skills": "Skills",
  "hub.nav.servers": "MCP Servers",
  "hub.nav.catalog": "Capability Catalog",
  "hub.nav.profiles": "Agent Profiles",
  "hub.nav.jobs": "Jobs",
  "hub.nav.permissions": "Permissions",
  "hub.nav.audit": "Audit",
  "hub.nav.proposals": "Improvement Proposals",
  "hub.metric.servers": "Connected servers",
  "hub.metric.skills": "Installed skills",
  "hub.metric.profiles": "Agent profiles",
  "hub.metric.vault": "Vault posture",
  "hub.metric.protected": "Protected",
  "hub.eyebrow.runtime_mesh": "Runtime mesh",
  "hub.eyebrow.operator_layer": "Operator layer",
  "hub.eyebrow.protocol_fabric": "Protocol fabric",
  "hub.eyebrow.local_intelligence": "Local intelligence",
  "hub.eyebrow.execution_policy": "Execution policy",
  "hub.eyebrow.scheduler": "Durable scheduler",
  "hub.eyebrow.evolution": "Supervised evolution",
  "hub.eyebrow.secure_ingestion": "Secure ingestion",
  "hub.card.servers": "MCP Servers",
  "hub.card.profiles": "Agent Profiles",
  "hub.card.skills": "Skills",
  "hub.card.jobs": "Agent Jobs",
  "hub.card.proposals": "Improvement Proposals",
  "hub.proposals_note": "Every change is evaluated, approved by a human, and applied as a new version.",
  "hub.empty.servers": "No MCP servers configured",
  "hub.empty.skills": "No skills installed",
  "hub.empty.profiles": "No agent profiles created",
  "hub.empty.hint": "Connect or import a capability to populate this view.",
  "hub.unnamed_server": "Unnamed server",
  "hub.unnamed_skill": "Unnamed skill",
  "hub.unnamed_profile": "Unnamed profile",
  "hub.no_description": "No description",
  "hub.tools_available": "{n} tools available",
  "hub.legacy": "legacy",
  "hub.default_model": "Default model",
  "hub.budget_tools": "{n} tools",
  "hub.budget_tokens": "{n} tokens",
  "hub.tool_schema": "Tool schema",
  "hub.input_contract": "Input contract",
  "hub.action.test": "Test",
  "hub.action.discover": "Discover",
  "hub.action.enable": "Enable",
  "hub.action.disable": "Disable",
  "hub.action.delete": "Delete",
  "hub.action.validate": "Validate",
  "hub.action.rollback": "Rollback",
  "hub.action.import_server": "Import server",
  "hub.status.healthy": "Healthy",
  "hub.status.unhealthy": "Unhealthy",
  "hub.status.unknown": "Unknown",
  "hub.status.active": "Active",
  "hub.status.disabled": "Disabled",
  "hub.preview.servers_detected": "servers detected",
  "hub.preview.secrets": "secrets",
  "hub.preview.conflicts": "conflicts",
  "hub.import.title": "Import MCP configuration",
  "hub.import.note": "JSON / JSONC / YAML / TOML supported; secrets go to the Vault only — raw values never reach the config store.",
  "hub.import.mode.auto-discover": "Auto-discover",
  "hub.import.mode.file": "File",
  "hub.import.mode.paste": "Paste",
  "hub.import.mode.manual": "Manual",
  "hub.import.preview": "Preview",
  "hub.import.choose_source": "Choose an import source",
  "hub.import.conflicts": "Conflicts",
  "hub.import.no_scan": "No scan yet",
  "hub.import.connection_test": "Connection test",
  "hub.import.waiting": "Waiting",
  "hub.import.selected": "{mode} selected — awaiting source input.",
  "hub.import.summary": "{servers} servers · {secrets} secrets",
  "hub.import.safe_preview": "Safe preview",
  "hub.import.filename_label": "Config file name",
  "hub.import.placeholder": "Paste a Claude Desktop, Cursor, VS Code or generic MCP config",
  "hub.skill.local_path": "Local SKILL.md or directory path",
  "hub.skill.import_local": "Import local skill",
  "hub.skill.git_url": "Git repository URL",
  "hub.skill.git_revision": "revision (optional)",
  "hub.skill.import_git": "Import Git skill",
  "hub.coming.title": "Policy-ready workspace",
  "hub.coming.note": "This surface is ready for backend data and actions. The empty state is intentional while commands are being connected.",

  "features.title": "Capability Map · design-doc coverage",
  "features.subtitle": "Based on the feature matrix in docs/01-Product-Design.md",
  "features.note": "This page tracks every module against the design doc. <strong>ready</strong> = runs locally today; <strong>partial</strong> = some capabilities available; <strong>planned</strong> = not scheduled for this milestone, but the DSL / architecture already reserves an interface.",

  "settings.title": "Appearance & runtime",
  "settings.opacity": "Opacity",
  "settings.window_base": "Window base",
  "settings.ui_panel": "UI panels",
  "settings.window_base_hint": "Window base opacity — 0 = fully see-through to the real desktop / 100 = opaque",
  "settings.ui_panel_hint": "Panel opacity — lower values keep the desktop behind visible while you work",
  "settings.preset.glass": "See-through",
  "settings.preset.frost": "Frosted",
  "settings.preset.solid": "⬜ Solid",
  "settings.preset.invisible": "Minimal",
  "settings.opacity_note": "The two sliders are independent: window base drives the Tauri transparent background; UI panels control the opacity of the home view, toolbars and cards, so you can orchestrate automation while watching the desktop.",
  "settings.theme": "Theme",
  "settings.theme.auto": "🖥 Follow system",
  "settings.theme.light": "☀️ Light",
  "settings.theme.dark": "🌙 Dark",
  "settings.theme_note": "The theme drives every token (foreground / lines / cards / selection); the frosted overlay adapts to light and dark.",
  "settings.environment": "Runtime",
  "settings.local_skills": "Local skills",
  "settings.voice_privacy": "Desktop voice & privacy",
  "settings.update_center": "Secure update center",
  "settings.language": "Language / 语言",
  "settings.language_note": "Applies immediately and is remembered. Modules whose strings are not extracted yet fall back to Chinese under English.",
  "settings.language.zh-CN": "简体中文",
  "settings.language.en": "English",
  "settings.autostart": "Launch at login",
  "settings.autostart_on": "Enabled: starts after login",
  "settings.autostart_off": "Disabled",

  "test.greet": "Hello {name}, {n} item(s)",
};

const TABLES = { "zh-CN": zhCN, en };
const DEFAULT_LOCALE = "zh-CN";
const STORAGE_KEY = "lumo.locale";

function storedLocale() {
  try {
    return globalThis.localStorage?.getItem(STORAGE_KEY) || null;
  } catch {
    return null;
  }
}

let currentLocale = TABLES[storedLocale()] ? storedLocale() : DEFAULT_LOCALE;
const listeners = new Set();

export function availableLocales() {
  return Object.keys(TABLES);
}

export function getLocale() {
  return currentLocale;
}

// 订阅语言切换；返回退订函数。切换后由订阅方负责重渲染自己的动态 DOM。
export function subscribeLocale(listener) {
  if (typeof listener !== "function") return () => {};
  listeners.add(listener);
  return () => listeners.delete(listener);
}

export function setLocale(locale) {
  const next = TABLES[locale] ? locale : DEFAULT_LOCALE;
  const changed = next !== currentLocale;
  currentLocale = next;
  try {
    globalThis.localStorage?.setItem(STORAGE_KEY, currentLocale);
  } catch {
    // 无 localStorage（测试环境）时仅保留内存态
  }
  if (changed) {
    for (const listener of listeners) {
      // 单个订阅方抛错不能阻断其余订阅方的重渲染
      try { listener(currentLocale); } catch { /* ignore */ }
    }
  }
  return currentLocale;
}

export function t(key, params = null) {
  const text = TABLES[currentLocale][key] ?? TABLES[DEFAULT_LOCALE][key] ?? key;
  if (!params) return text;
  return text.replace(/\{(\w+)\}/g, (match, name) =>
    Object.prototype.hasOwnProperty.call(params, name) ? String(params[name]) : match,
  );
}

// 未收录的枚举值（后端返回的 health/status 等）回落为「人类可读的原值」而不是
// 键名，避免界面上出现 hub.status.xxx 这种裸键。
export function tEnum(prefix, value, fallback) {
  const raw = String(value ?? "unknown");
  const key = `${prefix}.${raw.toLowerCase()}`;
  const text = t(key);
  return text === key ? (fallback ?? raw) : text;
}

const ATTRIBUTE_BINDINGS = [
  ["data-i18n-placeholder", "i18nPlaceholder", "placeholder"],
  ["data-i18n-title", "i18nTitle", "title"],
  ["data-i18n-aria-label", "i18nAriaLabel", "aria-label"],
  ["data-i18n-value", "i18nValue", "value"],
];

// 翻译静态 DOM：`data-i18n` 写 textContent，`data-i18n-html` 写 innerHTML
//（仅用于词表内含标签的固定文案），其余属性见 ATTRIBUTE_BINDINGS。
export function applyStaticTranslations(root = globalThis.document) {
  if (!root?.querySelectorAll) return 0;
  let applied = 0;
  for (const node of root.querySelectorAll("[data-i18n]")) {
    node.textContent = t(node.dataset.i18n);
    applied += 1;
  }
  for (const node of root.querySelectorAll("[data-i18n-html]")) {
    node.innerHTML = t(node.dataset.i18nHtml);
    applied += 1;
  }
  for (const [selector, datasetKey, attribute] of ATTRIBUTE_BINDINGS) {
    for (const node of root.querySelectorAll(`[${selector}]`)) {
      node.setAttribute(attribute, t(node.dataset[datasetKey]));
      applied += 1;
    }
  }
  return applied;
}
