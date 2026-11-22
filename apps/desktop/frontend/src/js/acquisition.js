// Acquisition workbench. Public content is untrusted and is escaped at every
// HTML boundary. Editors are kept outside snapshot rendering so live updates
// cannot discard an unsaved campaign or message.
export const ACQUISITION_PLATFORMS = Object.freeze({ xiaohongshu: "小红书", douyin: "抖音", weibo: "微博" });
export const ACQUISITION_STATUSES = Object.freeze({ new: "待判断", qualified: "匹配客户", contacted: "已联系", replied: "有回复", converted: "已转化", ignored: "不再联系" });
const CAMPAIGN_STATUSES = { draft: "待运行", running: "运行中", completed: "已完成", failed: "执行异常", cancelled: "已停止" };
const MESSAGE_STATUSES = { draft: "草稿", approved: "已批准", sending: "发送中", sent: "已发送", failed: "发送失败", received: "客户回复", uncertain: "结果待核实" };
export const escapeAcquisitionHtml = (value) => String(value ?? "").replaceAll("&", "&amp;").replaceAll("<", "&lt;").replaceAll(">", "&gt;").replaceAll('"', "&quot;").replaceAll("'", "&#39;");
const esc = escapeAcquisitionHtml;
const text = (value) => typeof value === "string" ? value : value == null ? "" : JSON.stringify(value);
const time = (value) => value ? new Date(value).toLocaleString("zh-CN", { month: "2-digit", day: "2-digit", hour: "2-digit", minute: "2-digit" }) : "—";
const empty = (title, hint = "") => `<div class="acq-empty"><strong>${esc(title)}</strong>${hint ? `<p>${esc(hint)}</p>` : ""}</div>`;
const status = (label) => `<span class="acq-badge">${esc(label)}</span>`;

export function newAcquisitionCampaign() {
  return { id: "", name: "", product: "", targetAudience: "", keywords: [], platforms: Object.keys(ACQUISITION_PLATFORMS), maxLeads: 20, status: "draft", createdAt: 0, updatedAt: 0, lastError: null,
    outreach: { desktopVision: false, analysisProvider: null, visionProvider: null, mode: "approved_auto", messageTemplate: "你好 {{name}}，看到你分享的需求。我这边提供{{product}}，方便了解一下你具体想解决的问题吗？", maxMessagesPerRun: 5, maxMessagesPerDay: 20, minIntervalSeconds: 60, minIntentScore: 70, approvedAt: null } };
}

export function parseAcquisitionKeywords(value) {
  return [...new Set(String(value || "").split(/[,，\n]/).map((item) => item.trim()).filter(Boolean))];
}

export function validateAcquisitionCampaign(campaign) {
  if (!campaign.name?.trim()) return "请填写任务名称。";
  if (!campaign.product?.trim()) return "请描述你的产品或服务。";
  if (!campaign.targetAudience?.trim()) return "请描述希望寻找的客户及需求。";
  if (!campaign.keywords?.length) return "请至少填写一个搜索关键词。";
  if (!campaign.platforms?.length || campaign.platforms.some((p) => !ACQUISITION_PLATFORMS[p])) return "请至少选择一个支持的平台。";
  if (!Number.isInteger(campaign.maxLeads) || campaign.maxLeads < 1 || campaign.maxLeads > 200) return "采集上限须为 1–200 的整数。";
  const policy = campaign.outreach || {};
  if (!["draft", "approved_auto"].includes(policy.mode)) return "请选择私聊模式。";
  if (policy.mode === "approved_auto" && !policy.messageTemplate?.trim()) return "请填写私聊话术。";
  if (policy.messageTemplate?.replace(/\{\{(name|product|audience)\}\}/g, "").includes("{{")) return "话术支持 {{name}}、{{product}} 和 {{audience}}。";
  for (const [key, label, min, max] of [["maxMessagesPerRun", "每次发送上限", 1, 50], ["maxMessagesPerDay", "每日发送上限（UTC）", 1, 200], ["minIntervalSeconds", "发送间隔", 10, 86400], ["minIntentScore", "最低匹配分", 0, 100]]) {
    if (!Number.isInteger(policy[key]) || policy[key] < min || policy[key] > max) return `${label}须为 ${min}–${max} 的整数。`;
  }
  if (policy.maxMessagesPerRun > policy.maxMessagesPerDay) return "每次发送上限不能超过每日发送上限（UTC）。";
  return "";
}

export function renderAcquisitionTemplate(template, lead, campaign) {
  const variables = { name: lead?.displayName, product: campaign?.product, audience: campaign?.targetAudience };
  return String(template || "").replace(/\{\{(name|product|audience)\}\}/g, (_, key) => String(variables[key] || "")).trim();
}

export function filterAcquisitionLeads(leads, { campaignId = "", query = "", status: selectedStatus = "", platform = "" } = {}) {
  const needle = query.trim().toLocaleLowerCase();
  return leads.filter((lead) => (!campaignId || lead.campaignId === campaignId) && (!selectedStatus || lead.status === selectedStatus) && (!platform || lead.platform === platform) && (!needle || [lead.displayName, lead.bio, lead.evidence, lead.platformUserId, ...(lead.tags || [])].join(" ").toLocaleLowerCase().includes(needle)));
}

export function safeAcquisitionUrl(value) {
  try {
    const url = new URL(value);
    if (url.protocol !== "https:" || url.username || url.password) return "";
    if (!["xiaohongshu.com", "douyin.com", "weibo.com", "weibo.cn"].some((host) => url.hostname === host || url.hostname.endsWith(`.${host}`))) return "";
    return url.href;
  } catch { return ""; }
}
function sourceLink(url, label) {
  const safe = safeAcquisitionUrl(url);
  return safe ? `<a href="${esc(safe)}" target="_blank" rel="noopener noreferrer">${esc(label)} ↗</a>` : "";
}

export function renderAcquisitionLeadRows(leads, selectedId = "") {
  if (!leads.length) return empty("还没有匹配的线索", "设置搜索范围并采集，或调整筛选条件。");
  return `<div class="acq-table-wrap"><table class="acq-table"><thead><tr><th>潜在客户 / 公开信号</th><th>匹配度</th><th>阶段</th></tr></thead><tbody>${leads.map((lead) => `<tr class="${lead.id === selectedId ? "is-selected" : ""}"><td><button class="acq-lead-name" data-acq-lead="${esc(lead.id)}"><span class="acq-avatar" aria-hidden="true">${esc(String(lead.displayName || "?").slice(0, 1))}</span><span><strong>${esc(lead.displayName)}</strong><small>${esc(ACQUISITION_PLATFORMS[lead.platform] || lead.platform)} · ${esc(lead.platformUserId)}</small></span></button><p class="acq-evidence-snippet">${esc(lead.evidence)}</p></td><td><strong class="acq-score">${esc(lead.intentScore)}<small>/100</small></strong></td><td>${status(ACQUISITION_STATUSES[lead.status] || lead.status)}</td></tr>`).join("")}</tbody></table></div>`;
}

export function renderAcquisitionApproval(campaign) {
  const policy = campaign.outreach;
  return `<h2>批准本任务的自动私聊</h2><p>批准后，在以下范围内自动发送已确认话术。范围或话术变更后需要重新批准。</p><dl class="acq-review"><dt>任务</dt><dd>${esc(campaign.name)}</dd><dt>平台</dt><dd>${campaign.platforms.map((p) => esc(ACQUISITION_PLATFORMS[p] || p)).join(" / ")}</dd><dt>文本模型源</dt><dd>${esc(policy.analysisProvider || "跟随默认模型源")}</dd><dt>视觉模型源</dt><dd>${esc(policy.visionProvider || "跟随文本模型源")}</dd><dt>操作方式</dt><dd>${policy.desktopVision ? "桌面视觉（平台窗口截图发送至视觉模型）" : "网页自动化"}</dd><dt>产品 / 服务</dt><dd>${esc(campaign.product)}</dd><dt>目标客户</dt><dd>${esc(campaign.targetAudience)}</dd><dt>关键词</dt><dd>${campaign.keywords.map(esc).join("、")}</dd><dt>任务线索总上限</dt><dd>最多 ${esc(campaign.maxLeads)} 位</dd><dt>发送范围</dt><dd>匹配分 ≥ ${esc(policy.minIntentScore)}；每次最多 ${esc(policy.maxMessagesPerRun)} 条，每日（UTC）最多 ${esc(policy.maxMessagesPerDay)} 条，间隔至少 ${esc(policy.minIntervalSeconds)} 秒</dd></dl><h3>已保存话术</h3><blockquote>${esc(policy.messageTemplate)}</blockquote><p class="acq-muted">支持客户昵称、产品和目标客户变量。</p><div class="acq-actions"><button data-acq-action="close-dialog">返回</button><button class="primary" data-acq-action="confirm-approval">批准此范围与话术</button></div>`;
}

export function renderAcquisitionMessages(messages) {
  if (!messages.length) return empty("尚无对话记录", "可以先保存话术草稿，或记录客户已有回复。");
  return messages.map((message) => `<article class="acq-message ${message.direction === "inbound" ? "is-inbound" : "is-outbound"}"><header><strong>${message.direction === "inbound" ? "客户" : "我方"}</strong><span>${esc(MESSAGE_STATUSES[message.status] || message.status)} · ${esc(time(message.createdAt))}</span></header><p>${esc(message.body)}</p>${message.error ? `<p class="acq-inline-error">${esc(message.error)}</p>` : ""}${["draft", "approved"].includes(message.status) ? `<button data-acq-send="${esc(message.id)}">审核并发送</button>` : ""}${message.status === "uncertain" ? `<small>请核对平台会话。</small><button data-acq-resolve="sent" data-message-id="${esc(message.id)}">核实已发送</button><button data-acq-resolve="failed" data-message-id="${esc(message.id)}">核实未发送</button>` : ""}</article>`).join("");
}

export function renderAcquisitionSources(sources) {
  return sources.map((s) => `<article>${sourceLink(s.sourceUrl, "原文")}<blockquote>${esc(s.evidence)}</blockquote></article>`).join("");
}
export function renderAcquisitionPlatforms(platforms, leads) {
  return `<div class="acq-tags" aria-label="各平台线索数">${platforms.map((p) => status(`${ACQUISITION_PLATFORMS[p] || p} · ${leads.filter((l) => l.platform === p).length} 位`)).join("")}</div>`;
}
function renderProfile(detail, analysisPending) {
  const { lead, messages = [], requirements = [] } = detail;
  const profile = lead.profile || {};
  const items = (values) => Array.isArray(values) && values.length ? `<ul>${values.map((value) => `<li>${esc(text(value))}</li>`).join("")}</ul>` : '<p class="acq-muted">暂无足够依据</p>';
  return `<header class="acq-detail-head"><div><span class="acq-eyebrow">客户档案</span><h2>${esc(lead.displayName)}</h2><p>${esc(ACQUISITION_PLATFORMS[lead.platform] || lead.platform)} · ${esc(lead.platformUserId)}</p></div><button data-acq-action="close-detail" aria-label="关闭客户档案">×</button></header><div class="acq-detail-links">${sourceLink(lead.profileUrl, "平台主页")}${sourceLink(lead.sourceUrl, "线索来源")}</div><label class="acq-field">跟进阶段<select data-acq-lead-status>${Object.entries(ACQUISITION_STATUSES).map(([key, label]) => `<option value="${key}"${lead.status === key ? " selected" : ""}>${label}</option>`).join("")}</select></label><section class="acq-detail-section"><div class="acq-section-title"><h3>用户画像</h3><button data-acq-action="analyze"${lead.status === "ignored" ? " disabled" : ""}>${analysisPending ? "补分析" : "重新分析"}</button></div><p>${esc(text(profile.summary) || "尚未建立画像，可基于公开内容和客户回复分析。")}</p><div class="acq-tags">${(lead.tags || []).map((tag) => status(tag)).join("")}</div><h4>需求与痛点</h4>${items([...(Array.isArray(profile.needs) ? profile.needs : []), ...(Array.isArray(profile.painPoints) ? profile.painPoints : [])])}${profile.nextAction ? `<h4>建议跟进</h4><p>${esc(text(profile.nextAction))}</p>` : ""}${profile.confidence != null ? `<small class="acq-muted">分析置信度：${esc(text(profile.confidence))}</small>` : ""}</section><details class="acq-source"><summary>公开来源 · ${detail.sources?.length ?? 1} 条</summary>${renderAcquisitionSources(detail.sources || [{sourceUrl:lead.sourceUrl, evidence:lead.evidence}])}${lead.bio ? `<p>${esc(lead.bio)}</p>` : ""}</details><section class="acq-detail-section"><h3>已记录需求 · ${requirements.length}</h3>${requirements.length ? requirements.map((requirement) => `<article class="acq-demand-mini"><strong>${esc(requirement.summary)}</strong><p>${esc(requirement.category)} · ${esc(requirement.urgency)}</p><blockquote>${esc(requirement.evidence)}</blockquote></article>`).join("") : '<p class="acq-muted">记录客户回复后，可提取需求。</p>'}</section><section class="acq-detail-section"><h3>对话记录</h3><div class="acq-messages">${renderAcquisitionMessages(messages)}</div></section>`;
}

function field(name, label, value, { type = "text", min, max, rows, hint = "", required = true } = {}) {
  const attributes = `${required ? " required" : ""}${min != null ? ` min="${min}"` : ""}${max != null ? ` max="${max}"` : ""}`;
  return `<label class="acq-field">${label}${rows ? `<textarea name="${name}" rows="${rows}"${attributes}>${esc(value)}</textarea>` : `<input name="${name}" type="${type}" value="${esc(value)}"${attributes}>`}${hint ? `<small>${esc(hint)}</small>` : ""}</label>`;
}

function providerSelect(name, label, selected, model, defaultLabel, sources) {
  const choices = sources.filter(source => source[model]?.trim());
  const unavailable = selected && !choices.some(source => source.name === selected);
  return `<label class="acq-field">${label}<select name="${name}">
    <option value="">${defaultLabel}</option>
    ${unavailable ? `<option value="${esc(selected)}" selected>${esc(selected)}（不可用）</option>` : ""}
    ${choices.map(source => `<option value="${esc(source.name)}"${selected === source.name ? " selected" : ""}>${esc(source.name)} · ${esc(source[model])}</option>`).join("")}
  </select></label>`;
}

export function renderCampaignForm(campaign, desktopSupported = true, modelSources = []) {
  const policy = campaign.outreach;
  return `<header class="acq-section-title"><div><span class="acq-eyebrow">搜索与联系策略</span><h2>${campaign.id ? "编辑获客任务" : "创建获客任务"}</h2></div><button type="button" data-acq-action="close-editor">收起</button></header><form data-acq-form><div class="acq-form-grid">${field("name", "任务名称", campaign.name)}${field("product", "产品 / 服务", campaign.product)}${field("targetAudience", "目标客户与希望搜集的需求", campaign.targetAudience, { rows: 3, hint: "例如：正在寻找门店预约系统的小店经营者，关注预约管理和提醒需求。" })}${field("keywords", "搜索关键词", campaign.keywords.join("\n"), { rows: 3, hint: "每行一个，或用逗号分隔。" })}<fieldset class="acq-platform-fields"><legend>搜索平台</legend>${Object.entries(ACQUISITION_PLATFORMS).map(([key, label]) => `<label><input type="checkbox" name="platform" value="${key}"${campaign.platforms.includes(key) ? " checked" : ""}> ${label}</label>`).join("")}</fieldset><label class="acq-field">操作方式<select name="desktopVision"><option value="browser"${!policy.desktopVision ? " selected" : ""}>网页自动化</option><option value="desktop"${policy.desktopVision ? " selected" : ""}${desktopSupported ? "" : " disabled"}>桌面视觉（macOS）</option></select><small>桌面视觉会把获客窗口截图发送至配置的视觉模型，需屏幕录制及辅助功能权限。</small></label>${providerSelect("analysisProvider", "文本模型源", policy.analysisProvider, "textModel", "跟随默认模型源", modelSources)}${providerSelect("visionProvider", "视觉模型源", policy.visionProvider, "visionModel", "跟随文本模型源", modelSources)}${field("maxLeads", "任务线索总上限", campaign.maxLeads, { type: "number", min: 1, max: 200 })}<label class="acq-field">私聊模式<select name="mode"><option value="approved_auto"${policy.mode === "approved_auto" ? " selected" : ""}>批准话术与范围后自动发送</option><option value="draft"${policy.mode === "draft" ? " selected" : ""}>只准备草稿，逐条审核发送</option></select></label>${field("minIntentScore", "自动联系最低匹配分", policy.minIntentScore, { type: "number", min: 0, max: 100 })}<div class="acq-form-wide">${field("messageTemplate", "私聊话术", policy.messageTemplate, { rows: 4, hint: "支持 {{name}} 客户昵称、{{product}} 产品描述、{{audience}} 目标客户。保存后在任务概览批准。" })}</div>${field("maxMessagesPerRun", "每次发送上限", policy.maxMessagesPerRun, { type: "number", min: 1, max: 50 })}${field("maxMessagesPerDay", "每日发送上限（UTC）", policy.maxMessagesPerDay, { type: "number", min: 1, max: 200 })}${field("minIntervalSeconds", "最短发送间隔（秒）", policy.minIntervalSeconds, { type: "number", min: 10, max: 86400 })}</div><div class="acq-form-footer"><span data-acq-dirty class="acq-muted">保存配置后，可采集线索和批准自动联系范围。</span><button type="button" data-acq-action="discard-editor">放弃修改</button><button type="submit" class="primary">保存任务</button></div></form>`;
}

function renderDemands(requirements, leads) {
  if (!requirements.length) return empty("需求池为空", "采集公开需求或记录客户回复后，分析结果会汇集在这里。");
  const byId = new Map(leads.map((lead) => [lead.id, lead]));
  return `<div class="acq-demands">${requirements.map((requirement) => `<article><div class="acq-section-title"><span class="acq-badge">${esc(requirement.category || "待分类")}</span><span class="acq-muted">${esc(requirement.urgency || "未判断")}</span></div><h3>${esc(requirement.summary)}</h3><blockquote>${esc(requirement.evidence)}</blockquote><footer><button data-acq-lead="${esc(requirement.leadId)}">${esc(byId.get(requirement.leadId)?.displayName || "查看客户")}</button><time>${esc(time(requirement.createdAt))}</time></footer></article>`).join("")}</div>`;
}

export function mountAcquisitionCenter({ root, call, listen }) {
  let snapshot = { campaigns: [], leads: [], requirements: [], activity: [], runningCampaignIds: [], computer: {} };
  let selectedId = "", selectedLeadId = "", currentDetail = null, editing = null, dirty = false, visible = true, busy = false, activeTab = "leads", disposed = false;
  let loadVersion = 0, detailVersion = 0, refreshTimer = null, unlisten = null, pendingApproval = null, pendingMessage = null;
  root.innerHTML = `<header class="acq-hero"><div><h1>获客中心</h1></div><div class="acq-actions"><button data-acq-action="refresh">刷新</button><button class="primary" data-acq-action="new">＋ 创建任务</button></div></header><p class="acq-error" data-acq-error role="alert" hidden></p><p class="acq-notice" data-acq-notice role="status" hidden></p><section class="acq-connection"><div data-acq-computer><strong>Computer Use</strong><span>正在检查浏览器和模型配置…</span></div><div class="acq-actions">${Object.entries(ACQUISITION_PLATFORMS).map(([key, label]) => `<button data-acq-login="${key}" title="打开浏览器完成登录">登录${label} ↗</button>`).join("")}</div></section><section class="acq-editor" data-acq-editor hidden></section><div class="acq-workspace"><aside class="acq-campaigns"><header><span class="acq-eyebrow">获客任务</span><span data-acq-campaign-count>0</span></header><div data-acq-campaigns></div></aside><div class="acq-main"><section data-acq-summary></section><div class="acq-tabs" role="tablist" aria-label="获客数据"><button role="tab" data-acq-tab="leads" aria-selected="true">潜在客户</button><button role="tab" data-acq-tab="demands" aria-selected="false">需求池</button><button role="tab" data-acq-tab="activity" aria-selected="false">执行记录</button></div><div class="acq-filters" data-acq-filters><input data-acq-search type="search" aria-label="搜索客户与公开需求" placeholder="搜索客户、需求、标签…"><select data-acq-filter-status aria-label="筛选客户阶段"><option value="">所有阶段</option>${Object.entries(ACQUISITION_STATUSES).map(([key, label]) => `<option value="${key}">${label}</option>`).join("")}</select><select data-acq-filter-platform aria-label="筛选来源平台"><option value="">所有平台</option>${Object.entries(ACQUISITION_PLATFORMS).map(([key, label]) => `<option value="${key}">${label}</option>`).join("")}</select></div><section data-acq-content aria-live="polite"></section></div><aside class="acq-detail" data-acq-detail hidden><div data-acq-detail-content></div><section class="acq-composer"><label class="acq-field">私聊草稿<textarea rows="4" data-acq-draft placeholder="写下准备发送的消息…"></textarea></label><div class="acq-actions"><button data-acq-action="use-template">使用任务话术</button><button data-acq-action="save-draft">保存草稿</button></div><label class="acq-field">记录客户回复<textarea rows="3" data-acq-note placeholder="粘贴客户回复，保存并分析需求…"></textarea></label><button data-acq-action="save-note">记录回复并分析</button></section></aside></div><dialog class="acq-dialog" data-acq-dialog><div data-acq-dialog-content></div></dialog>`;
  const q = (selector) => root.querySelector(selector);
  const campaign = () => snapshot.campaigns.find((item) => item.id === selectedId);
  const notify = (message, isError = false) => { const box = q(isError ? "[data-acq-error]" : "[data-acq-notice]"); box.textContent = message; box.hidden = !message; };
  const report = (error) => notify(error?.message || (typeof error === "string" ? error : JSON.stringify(error)), true);
  const requireSaved = () => { if (dirty) throw new Error("当前任务配置尚未保存，请先保存，再执行或切换任务。"); };
  const selectedLeads = () => snapshot.leads.filter((lead) => !selectedId || lead.campaignId === selectedId);
  const selectedRequirements = () => { const ids = new Set(selectedLeads().map((lead) => lead.id)); return snapshot.requirements.filter((item) => ids.has(item.leadId)); };

  function renderContent() {
    q("[data-acq-filters]").hidden = activeTab !== "leads";
    root.querySelectorAll("[data-acq-tab]").forEach((button) => button.setAttribute("aria-selected", String(button.dataset.acqTab === activeTab)));
    if (activeTab === "leads") q("[data-acq-content]").innerHTML = renderAcquisitionLeadRows(filterAcquisitionLeads(selectedLeads(), { query: q("[data-acq-search]").value, status: q("[data-acq-filter-status]").value, platform: q("[data-acq-filter-platform]").value }), selectedLeadId);
    else if (activeTab === "demands") q("[data-acq-content]").innerHTML = renderDemands(selectedRequirements(), selectedLeads());
    else q("[data-acq-content]").innerHTML = snapshot.activity.length ? `<ol class="acq-log">${snapshot.activity.filter((entry) => !selectedId || entry.campaignId === selectedId).map((entry) => `<li><time>${esc(time(entry.createdAt))}</time><div><strong>${esc(entry.kind)}</strong><p>${esc(text(entry.detail))}</p></div></li>`).join("")}</ol>` : empty("暂无执行记录", "采集、联系、同步及异常都会记录在这里。");
  }

  function renderSnapshot() {
    const current = campaign(), leads = selectedLeads(), running = snapshot.runningCampaignIds.includes(selectedId);
    q("[data-acq-campaign-count]").textContent = snapshot.campaigns.length;
    q("[data-acq-campaigns]").innerHTML = snapshot.campaigns.length ? snapshot.campaigns.map((item) => `<button data-acq-campaign="${esc(item.id)}" class="acq-campaign ${item.id === selectedId ? "is-selected" : ""}"><strong>${esc(item.name)}</strong><span>${item.platforms.map((p) => esc(ACQUISITION_PLATFORMS[p] || p)).join(" · ")}</span><small>${esc(CAMPAIGN_STATUSES[item.status] || item.status)}${item.outreach?.approvedAt ? " · 已批准自动联系" : ""}</small></button>`).join("") : empty("创建首个获客任务", "定义客户条件，开始寻找需求。");
    q("[data-acq-computer]").innerHTML = `<strong><i class="acq-status-dot ${snapshot.computer?.ready ? "is-ready" : ""}"></i>Computer Use</strong><span>${esc(snapshot.computer?.message || "请配置模型，并登录所选平台。")}</span>`;
    q("[data-acq-summary]").innerHTML = current ? `<div class="acq-campaign-head"><div><span class="acq-eyebrow">${esc(CAMPAIGN_STATUSES[current.status] || current.status)}</span><h2>${esc(current.name)}</h2><p>${esc(current.targetAudience)}</p></div><button data-acq-action="edit"${running ? " disabled" : ""}>编辑配置</button></div><div class="acq-metrics"><div><strong>${leads.length}</strong><span>潜在客户</span></div><div><strong>${leads.filter((lead) => ["qualified", "contacted", "replied", "converted"].includes(lead.status)).length}</strong><span>匹配 / 跟进</span></div><div><strong>${leads.filter((lead) => ["replied", "converted"].includes(lead.status)).length}</strong><span>有效回复</span></div><div><strong>${selectedRequirements().length}</strong><span>已发现需求</span></div></div><p class="acq-muted">${current.outreach.desktopVision ? "桌面视觉" : "网页自动化"}</p>${renderAcquisitionPlatforms(current.platforms, leads)}<div class="acq-run-bar"><div class="acq-actions"><button class="primary" data-acq-run="acquire"${running || (current.outreach.mode === "approved_auto" && !current.outreach.approvedAt) ? " disabled" : ""}>一键获客</button><button data-acq-run="discover"${running ? " disabled" : ""}>采集潜在客户</button><button data-acq-run="outreach"${running || (current.outreach.mode === "approved_auto" && !current.outreach.approvedAt) ? " disabled" : ""}>${current.outreach.mode === "draft" ? "批量准备草稿" : "自动联系"}</button><button data-acq-run="sync"${running ? " disabled" : ""}>同步对话</button>${snapshot.pendingAnalysisIds?.length ? `<button data-acq-run="analyze_pending"${running ? " disabled" : ""}>补分析 · ${snapshot.pendingAnalysisIds.length} 位</button>` : ""}${running ? '<button class="acq-stop" data-acq-action="stop">停止任务</button>' : ""}</div><span>${current.outreach.mode === "draft" ? "草稿模式" : current.outreach.approvedAt ? `自动联系已批准 · 每次 ${esc(current.outreach.maxMessagesPerRun)} 条` : '自动联系待批准'}</span></div>${current.outreach.mode === "approved_auto" ? `<div class="acq-approval-strip"><p>${current.outreach.approvedAt ? "按已批准的话术、目标客户与发送配额执行。" : "保存后，请先检查并批准话术与范围，再启动自动联系。"}</p><button data-acq-action="review-approval"${running ? " disabled" : ""}>${current.outreach.approvedAt ? "查看批准范围" : "审核并批准范围"}</button></div>` : ""}${current.lastError ? `<p class="acq-inline-error">${esc(current.lastError)}</p>` : ""}` : empty("尚未选择获客任务", "创建任务，填写产品、目标客户和关键词；采集结果会保留平台来源与原始依据。");
    renderContent();
  }

  async function refresh() {
    const version = ++loadVersion;
    try {
      const next = await call("acquisition_snapshot", { campaignId: selectedId || null });
      if (disposed || version !== loadVersion) return;
      snapshot = { ...snapshot, ...next };
      if (!selectedId && snapshot.campaigns.length) selectedId = snapshot.campaigns[0].id;
      renderSnapshot();
      if (selectedLeadId && visible) await loadDetail(selectedLeadId, false);
    } catch (error) { if (!disposed && version === loadVersion) report(error); }
  }

  async function loadDetail(leadId, resetComposer = true) {
    selectedLeadId = leadId;
    const version = ++detailVersion;
    q("[data-acq-detail]").hidden = false;
    if (resetComposer) {
      currentDetail = null;
      q("[data-acq-detail-content]").innerHTML = empty("正在读取客户档案…");
      q("[data-acq-draft]").value = "";
      q("[data-acq-note]").value = "";
    }
    renderContent();
    const detail = await call("acquisition_lead_detail", { leadId });
    if (disposed || selectedLeadId !== leadId || version !== detailVersion) return;
    currentDetail = detail;
    q("[data-acq-detail-content]").innerHTML = renderProfile(detail, snapshot.pendingAnalysisIds?.includes(leadId));
    if (resetComposer) q("[data-acq-draft]").value = renderAcquisitionTemplate(campaign()?.outreach?.messageTemplate, detail.lead, campaign());
  }

  function editCampaign(value) {
    editing = structuredClone(value);
    dirty = false;
    q("[data-acq-editor]").innerHTML = renderCampaignForm(editing, snapshot.computer?.desktopSupported === true, snapshot.modelSources);
    q("[data-acq-editor]").hidden = false;
    q('[data-acq-form] [name="name"]').focus();
  }

  function readForm(form) {
    const data = new FormData(form);
    return { ...editing, name: String(data.get("name") || "").trim(), product: String(data.get("product") || "").trim(), targetAudience: String(data.get("targetAudience") || "").trim(), keywords: parseAcquisitionKeywords(data.get("keywords")), platforms: data.getAll("platform"), maxLeads: Number(data.get("maxLeads")), outreach: { ...editing.outreach, desktopVision: data.get("desktopVision") === "desktop", analysisProvider: data.get("analysisProvider") || null, visionProvider: data.get("visionProvider") || null, mode: data.get("mode"), messageTemplate: String(data.get("messageTemplate") || "").trim(), maxMessagesPerRun: Number(data.get("maxMessagesPerRun")), maxMessagesPerDay: Number(data.get("maxMessagesPerDay")), minIntervalSeconds: Number(data.get("minIntervalSeconds")), minIntentScore: Number(data.get("minIntentScore")) } };
  }

  function openDialog(html) {
    q("[data-acq-dialog-content]").innerHTML = html;
    const dialog = q("[data-acq-dialog]");
    if (typeof dialog.showModal === "function") dialog.showModal(); else dialog.setAttribute("open", "");
  }
  function closeDialog() {
    const dialog = q("[data-acq-dialog]");
    if (typeof dialog.close === "function") dialog.close(); else dialog.removeAttribute("open");
    pendingApproval = null; pendingMessage = null;
  }

  async function perform(action) {
    if (busy) return;
    busy = true;
    root.setAttribute("aria-busy", "true");
    notify("", true); notify("");
    try { await action(); } catch (error) { report(error); } finally { busy = false; root.setAttribute("aria-busy", "false"); }
  }

  root.addEventListener("submit", (event) => {
    if (!event.target.matches("[data-acq-form]")) return;
    event.preventDefault();
    perform(async () => {
      const value = readForm(event.target), error = validateAcquisitionCampaign(value);
      if (error) throw new Error(error);
      const saved = await call("acquisition_save_campaign", { campaign: value });
      selectedId = saved.id; editing = saved; dirty = false;
      q("[data-acq-editor]").hidden = true;
      closeDetail();
      await refresh(); notify("任务已保存。可开始采集，或审核自动联系范围。");
    });
  });

  function closeDetail() { selectedLeadId = ""; currentDetail = null; ++detailVersion; q("[data-acq-detail]").hidden = true; renderContent(); }

  root.addEventListener("click", (event) => {
    const button = event.target.closest("button");
    if (!button || button.disabled) return;
    if (button.type === "submit" && button.form?.matches("[data-acq-form]")) return;
    if (button.dataset.acqAction === "stop") {
      call("acquisition_cancel", { campaignId: selectedId }).then(() => { notify("已请求停止任务。"); return refresh(); }).catch(report);
      return;
    }
    if (button.dataset.acqTab) { activeTab = button.dataset.acqTab; renderContent(); return; }
    if (button.dataset.acqAction === "close-dialog") { closeDialog(); return; }
    perform(async () => {
      if (button.dataset.acqCampaign) {
        requireSaved(); selectedId = button.dataset.acqCampaign; closeDetail(); q("[data-acq-editor]").hidden = true; await refresh(); return;
      }
      if (button.dataset.acqLead) { await loadDetail(button.dataset.acqLead); return; }
      if (button.dataset.acqLogin) {
        await call("acquisition_browser_open", { platform: button.dataset.acqLogin }); notify("已打开浏览器。完成平台登录后，返回这里开始采集。"); await refresh(); return;
      }
      if (button.dataset.acqRun) {
        requireSaved(); await call("acquisition_start", { campaignId: selectedId, mode: button.dataset.acqRun }); await refresh(); notify("任务已启动，可在执行记录中查看进度。"); return;
      }
      if (button.dataset.acqResolve) {
        await call("acquisition_resolve_message", { messageId: button.dataset.messageId, sent: button.dataset.acqResolve === "sent" }); await refresh(); return;
      }
      if (button.dataset.acqSend) {
        const message = currentDetail?.messages.find((item) => item.id === button.dataset.acqSend);
        if (!message) throw new Error("草稿已变化，请刷新客户档案。");
        pendingMessage = message.id;
        openDialog(`<h2>确认发送这条私聊</h2><p>平台：${esc(ACQUISITION_PLATFORMS[currentDetail.lead.platform])} · 收件人：${esc(currentDetail.lead.displayName)}（${esc(currentDetail.lead.platformUserId)}）</p><blockquote>${esc(message.body)}</blockquote><div class="acq-actions"><button data-acq-action="close-dialog">返回编辑</button><button class="primary" data-acq-action="confirm-send">确认发送</button></div>`); return;
      }
      switch (button.dataset.acqAction) {
        case "refresh": await refresh(); break;
        case "new": requireSaved(); editCampaign(newAcquisitionCampaign()); break;
        case "edit": requireSaved(); if (campaign()) editCampaign(campaign()); break;
        case "close-editor": if (dirty) { notify("请保存配置或放弃修改。", true); break; } q("[data-acq-editor]").hidden = true; break;
        case "discard-editor": dirty = false; editing = null; q("[data-acq-editor]").hidden = true; break;
        case "close-detail": closeDetail(); break;
        case "review-approval": requireSaved(); pendingApproval = structuredClone(campaign()); openDialog(renderAcquisitionApproval(pendingApproval)); break;
        case "confirm-approval": {
          if (!pendingApproval) throw new Error("请重新打开批准范围。");
          const approved = await call("acquisition_approve_outreach", { campaignId: pendingApproval.id, expectedUpdatedAt: pendingApproval.updatedAt });
          closeDialog(); selectedId = approved.id; await refresh(); notify("话术与范围已批准，可以启动自动联系。"); break;
        }
        case "confirm-send": {
          if (!pendingMessage) throw new Error("请重新审核待发送消息。");
          const messageId = pendingMessage; closeDialog();
          const result = await call("acquisition_send_message", { messageId });
          await refresh(); notify(result?.status === "sent" ? "消息已发送并记录。" : `发送状态：${MESSAGE_STATUSES[result?.status] || result?.status || "请核对执行记录"}`); break;
        }
        case "analyze": await call("acquisition_analyze_lead", { leadId: selectedLeadId }); await refresh(); notify("用户画像与需求已更新。"); break;
        case "use-template": q("[data-acq-draft]").value = renderAcquisitionTemplate(campaign()?.outreach?.messageTemplate, currentDetail?.lead, campaign()); break;
        case "save-draft": {
          const body = q("[data-acq-draft]").value.trim(); if (!body) throw new Error("请先填写私聊内容。");
          await call("acquisition_draft_message", { leadId: selectedLeadId, body }); await loadDetail(selectedLeadId, false); notify("草稿已保存，审核后可发送。"); break;
        }
        case "save-note": {
          const body = q("[data-acq-note]").value.trim(); if (!body) throw new Error("请先填写客户回复。");
          const result = await call("acquisition_add_note", { leadId: selectedLeadId, body }); q("[data-acq-note]").value = ""; await refresh();
          notify(result?.analysisError ? `回复已保存，待补分析：${result.analysisError}` : "回复已记录并分析。", Boolean(result?.analysisError)); break;
        }
      }
    });
  });
  root.addEventListener("input", (event) => {
    if (event.target.closest("[data-acq-form]")) { dirty = true; q("[data-acq-dirty]").textContent = "有未保存修改；范围或话术变更后需要重新批准。"; }
    if (event.target.matches("[data-acq-search]")) renderContent();
  });
  root.addEventListener("change", (event) => {
    if (event.target.matches("[data-acq-filter-status], [data-acq-filter-platform]")) renderContent();
    if (event.target.matches("[data-acq-lead-status]")) perform(async () => { await call("acquisition_set_lead_status", { leadId: selectedLeadId, status: event.target.value }); await refresh(); });
  });
  q("[data-acq-dialog]").addEventListener("cancel", () => { pendingApproval = null; pendingMessage = null; });
  if (listen) Promise.resolve(listen("lumo://acquisition-updated", () => {
    if (!visible || disposed) return;
    clearTimeout(refreshTimer);
    refreshTimer = setTimeout(() => { refreshTimer = null; void refresh(); }, 150);
  })).then((stop) => { if (disposed) stop(); else unlisten = stop; }).catch(report);
  renderSnapshot();
  void refresh();
  return {
    refresh,
    setVisible(value) { visible = value; if (value) void refresh(); },
    dispose() { disposed = true; ++loadVersion; ++detailVersion; clearTimeout(refreshTimer); unlisten?.(); },
  };
}
