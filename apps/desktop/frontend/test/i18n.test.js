import test from "node:test";
import assert from "node:assert/strict";

import { applyStaticTranslations, availableLocales, getLocale, setLocale, subscribeLocale, t, tEnum } from "../src/js/i18n.js";

test("defaults to zh-CN and resolves known keys", () => {
  setLocale("zh-CN");
  assert.equal(getLocale(), "zh-CN");
  assert.equal(t("common.save"), "保存");
});

test("en locale resolves translated keys and falls back to zh-CN for missing ones", () => {
  setLocale("en");
  assert.equal(t("common.save"), "Save");
  // 仅存在于 zh 表的键回落中文而不是空白
  assert.equal(t("test.zh_only"), "只有中文");
  setLocale("zh-CN");
});

test("unknown keys return the key itself (fail-visible, not blank)", () => {
  assert.equal(t("nope.missing.key"), "nope.missing.key");
});

test("params interpolate {name} placeholders", () => {
  setLocale("zh-CN");
  assert.equal(t("test.greet", { name: "Lumo", n: 3 }), "你好 Lumo，共 3 项");
});

test("unsupported locale falls back to zh-CN", () => {
  setLocale("fr-FR");
  assert.equal(getLocale(), "zh-CN");
});

test("availableLocales lists zh-CN and en", () => {
  assert.deepEqual([...availableLocales()].sort(), ["en", "zh-CN"]);
});

// ── 静态 DOM 翻译 ──────────────────────────────────────────────────────────
// 没有 DOM 环境（node --test），用最小替身覆盖 applyStaticTranslations 实际
// 用到的表面：querySelectorAll + dataset + textContent/innerHTML/setAttribute。
const SELECTOR_DATASET = {
  "[data-i18n]": "i18n",
  "[data-i18n-html]": "i18nHtml",
  "[data-i18n-placeholder]": "i18nPlaceholder",
  "[data-i18n-title]": "i18nTitle",
  "[data-i18n-aria-label]": "i18nAriaLabel",
  "[data-i18n-value]": "i18nValue",
};

function fakeNode(dataset) {
  return {
    dataset,
    textContent: "",
    innerHTML: "",
    attributes: {},
    setAttribute(name, value) { this.attributes[name] = value; },
  };
}

function fakeRoot(nodes) {
  return {
    querySelectorAll(selector) {
      const key = SELECTOR_DATASET[selector];
      return nodes.filter((node) => node.dataset[key] !== undefined);
    },
  };
}

test("applyStaticTranslations fills textContent, innerHTML and attributes", () => {
  setLocale("zh-CN");
  const text = fakeNode({ i18n: "common.save" });
  const markup = fakeNode({ i18nHtml: "features.note" });
  const placeholder = fakeNode({ i18nPlaceholder: "hub.import.placeholder" });
  const aria = fakeNode({ i18nAriaLabel: "hub.nav_label" });
  const applied = applyStaticTranslations(fakeRoot([text, markup, placeholder, aria]));

  assert.equal(applied, 4);
  assert.equal(text.textContent, "保存");
  assert.match(markup.innerHTML, /<strong>ready<\/strong>/);
  assert.equal(placeholder.attributes.placeholder, t("hub.import.placeholder"));
  assert.equal(aria.attributes["aria-label"], "能力中枢分区");
});

test("applyStaticTranslations re-renders the same nodes after a locale switch", () => {
  setLocale("zh-CN");
  const node = fakeNode({ i18n: "hub.title" });
  const root = fakeRoot([node]);
  applyStaticTranslations(root);
  assert.equal(node.textContent, "能力中枢");
  setLocale("en");
  applyStaticTranslations(root);
  assert.equal(node.textContent, "Capability Hub");
  setLocale("zh-CN");
});

test("applyStaticTranslations tolerates a missing root", () => {
  assert.equal(applyStaticTranslations(null), 0);
  assert.equal(applyStaticTranslations({}), 0);
});

// ── 订阅 ───────────────────────────────────────────────────────────────────
test("subscribeLocale fires only on actual changes and can unsubscribe", () => {
  setLocale("zh-CN");
  const seen = [];
  const unsubscribe = subscribeLocale((locale) => seen.push(locale));
  setLocale("en");
  setLocale("en"); // 同值重复设置不应再次通知
  unsubscribe();
  setLocale("zh-CN");
  assert.deepEqual(seen, ["en"]);
});

test("a throwing subscriber does not block the others", () => {
  setLocale("zh-CN");
  const seen = [];
  const unsubBad = subscribeLocale(() => { throw new Error("boom"); });
  const unsubGood = subscribeLocale((locale) => seen.push(locale));
  assert.doesNotThrow(() => setLocale("en"));
  assert.deepEqual(seen, ["en"]);
  unsubBad();
  unsubGood();
  setLocale("zh-CN");
});

// ── 枚举回落 ───────────────────────────────────────────────────────────────
test("tEnum resolves known values and falls back to a readable one", () => {
  setLocale("zh-CN");
  assert.equal(tEnum("hub.status", "healthy"), "健康");
  assert.equal(tEnum("hub.status", "HEALTHY"), "健康");
  // 未收录的值不能渲染成裸键名
  assert.equal(tEnum("hub.status", "degraded", "Degraded"), "Degraded");
  assert.equal(tEnum("hub.status", "degraded"), "degraded");
});
