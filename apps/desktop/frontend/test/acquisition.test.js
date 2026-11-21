import { test } from "node:test";
import assert from "node:assert/strict";
import { renderCampaignForm } from "../src/js/acquisition.js";
import { newAcquisitionCampaign, validateAcquisitionCampaign, renderAcquisitionApproval, renderAcquisitionTemplate, renderAcquisitionMessages, renderAcquisitionPlatforms, renderAcquisitionSources } from "../src/js/acquisition.js";

test("approval review and preview preserve the saved text and quote untrusted names", () => {
  const c = newAcquisitionCampaign();
  Object.assign(c, { name: "预约", product: "门店预约", targetAudience: "店主", keywords: ["预约"] });
  c.outreach.messageTemplate = "{{name}}：{{audience}}使用{{product}}";
  assert.equal(validateAcquisitionCampaign(c), "");
  assert.equal(renderAcquisitionTemplate(c.outreach.messageTemplate, { displayName: "{{product}}" }, c), "{{product}}：店主使用门店预约");
  c.product = "<script>bad()</script>";
  const review = renderAcquisitionApproval(c);
  assert.ok(review.includes("&lt;script&gt;") && !review.includes("<script>"));
  assert.ok(review.includes(c.outreach.messageTemplate));
  c.maxLeads = 201;
  assert.notEqual(validateAcquisitionCampaign(c), "");
});
test("uncertain message can be reconciled without exposing a send action", () => {
  const html = renderAcquisitionMessages([{ id:"m", direction:"outbound", status:"uncertain", body:"测试" }]);
  assert.ok(html.includes("data-acq-resolve=\"sent\"") && html.includes("data-acq-resolve=\"failed\""));
  assert.ok(!html.includes("data-acq-send="));
});

test("native window control is opt-in and visible in the scope review", () => {
  const c = newAcquisitionCampaign();
  assert.equal(c.outreach.desktopVision, false);
  assert.match(renderCampaignForm(c, false), /value="desktop" disabled/);
  c.outreach.desktopVision = true;
  assert.match(renderCampaignForm(c), /value="desktop" selected/);
  assert.match(renderAcquisitionApproval(c), /桌面视觉（平台窗口截图发送至视觉模型）/);
});

test("selected platforms include zero-result platforms and evidence links keep their own origin", () => {
  const stats = renderAcquisitionPlatforms(["xiaohongshu","douyin","weibo"], [{platform:"douyin"}, {platform:"douyin"}, {platform:"weibo"}]);
  for (const expected of ["小红书 · 0 位","抖音 · 2 位","微博 · 1 位"]) assert.ok(stats.includes(expected));
  const html = renderAcquisitionSources([{sourceUrl:"https://weibo.com/5/abc", evidence:"一条需求"}, {sourceUrl:"javascript:alert(1)", evidence:"<script>"}]);
  assert.ok(html.includes("https://weibo.com/5/abc") && html.includes("&lt;script&gt;"));
  assert.ok(!html.includes("javascript:"));
});
