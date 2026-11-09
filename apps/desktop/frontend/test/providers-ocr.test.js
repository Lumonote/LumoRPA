import test from "node:test";
import assert from "node:assert/strict";

let invoke;
globalThis.window = { __TAURI__: { core: { invoke: (...args) => invoke(...args) } } };
globalThis.localStorage = { getItem: () => null };
const { state } = await import("../src/js/state.js");
const { renderOcrModelPicker, runOcrModelCheck, refreshProviders } = await import("../src/js/providers.js");
const model = (status = {}) => ({
  preset: { id: "modelscope/test/ocr", repo: "test/ocr", label: "Local OCR", engine: "local", runnable: true },
  cacheDir: "/cache/ocr", downloaded: true, downloadState: "complete", runtimeState: "unchecked", inferenceState: "unchecked",
  ...status,
});
function render(status) {
  state.ocrModels = status ? [model(status)] : [];
  return renderOcrModelPicker("modelscope/test/ocr");
}

test("complete OCR files still show unchecked runtime and unverified inference", () => {
  const html = render({});
  assert.match(html, /文件完整/);
  assert.match(html, /环境未检查/);
  assert.match(html, /识别未验证/);
  assert.match(html, /data-check-ocr-model="modelscope\/test\/ocr"/);
  assert.match(html, /data-test-ocr-model="modelscope\/test\/ocr"/);
  assert.doesNotMatch(html, /本地识别已验证/);
});

test("partial files can be redownloaded but cannot be used for an inference test", () => {
  const html = render({ downloadState: "partial", downloadReason: "missing shard", downloaded: false });
  assert.match(html, /文件不完整/);
  assert.match(html, /missing shard/);
  assert.match(html, /data-download-ocr-model="[^"]+">重新下载/);
  assert.match(html, /<button[^>]*disabled[^>]*data-test-ocr-model/);
  assert.doesNotMatch(html, /<button[^>]*disabled[^>]*data-check-ocr-model/);
});

test("runtime and inference failures are distinct and untrusted reasons are escaped", () => {
  const html = render({ runtimeState: "missing", runtimeReason: "missing torch <img onerror=1>", inferenceState: "failed", inferenceReason: "worker <script>failed</script>" });
  assert.match(html, /运行环境缺失/);
  assert.match(html, /识别测试失败/);
  assert.match(html, /&lt;img/);
  assert.match(html, /&lt;script&gt;/);
  assert.doesNotMatch(html, /<img|<script>/);
});

test("verified inference is explicit and unsupported models cannot run a local inference test", () => {
  assert.match(render({ runtimeState: "ready", inferenceState: "ready" }), /环境检查通过/);
  assert.match(render({ runtimeState: "ready", inferenceState: "ready" }), /本地识别已验证/);
  const html = render({ preset: { ...model().preset, runnable: false } });
  assert.match(html, /<button[^>]*disabled[^>]*data-test-ocr-model/);
  assert.match(html, /暂不支持本地识别/);
});

test("legacy downloaded flags do not claim verified readiness and an empty list can be refreshed", () => {
  const html = render({ downloadState: undefined });
  assert.doesNotMatch(html, /文件完整|本地识别已验证/);
  assert.match(html, /文件状态未检查/);
  assert.match(render(null), /data-refresh-ocr-models/);
});

function ui(context) {
  const previous = globalThis.document;
  context.after(() => { globalThis.document = previous; });
  const panel = { outerHTML: "" };
  const fields = new Map();
  const element = () => ({ value: "", innerHTML: "", lastChild: { textContent: "" }, classList: { toggle() {} }, querySelector: () => null, querySelectorAll: () => [] });
  const editor = element();
  Object.defineProperty(editor, "innerHTML", { set() { throw new Error("OCR operations must preserve the provider form"); } });
  editor.querySelector = () => panel;
  fields.set("providerEditBody", editor);
  fields.set("pOcrModel", { value: model().preset.id });
  fields.set("pName", { value: "original unsaved name" });
  globalThis.document = { getElementById(id) {
    if (!fields.has(id)) fields.set(id, element());
    return fields.get(id);
  } };
  return { panel, fields };
}

test("runtime checks use the local IPC contract, preserve edits and prevent duplicate requests", async (context) => {
  const { fields, panel } = ui(context);
  state.ocrModels = [model()];
  let finish;
  const calls = [];
  invoke = (command, args) => {
    calls.push({ command, args });
    return new Promise((resolve) => { finish = resolve; });
  };
  const checking = runOcrModelCheck(model().preset.id);
  assert.match(panel.outerHTML, /正在检查运行环境/);
  assert.equal(await runOcrModelCheck(model().preset.id), null);
  fields.get("pName").value = "edited during the check";
  finish(model({ runtimeState: "missing", runtimeReason: "torch missing" }));
  const result = await checking;
  assert.deepEqual(calls, [{ command: "check_ocr_model", args: { model: model().preset.id, inference: false } }]);
  assert.equal(result.runtimeState, "missing");
  assert.equal(fields.get("pName").value, "edited during the check");
  assert.match(panel.outerHTML, /torch missing/);
  assert.doesNotMatch(panel.outerHTML, /环境检查通过/);
});

test("inference tests require complete runnable files and surface IPC failures", async (context) => {
  const { panel } = ui(context);
  let calls = 0;
  invoke = async () => { calls++; throw new Error("worker <unavailable>"); };
  state.ocrModels = [model({ downloadState: "partial" })];
  assert.equal(await runOcrModelCheck(model().preset.id, true), null);
  assert.equal(calls, 0);
  assert.match(panel.outerHTML, /请先准备完整/);
  state.ocrModels = [model()];
  assert.equal(await runOcrModelCheck(model().preset.id, true), null);
  assert.equal(calls, 1);
  assert.match(panel.outerHTML, /worker &lt;unavailable&gt;/);
});

test("a stale list response cannot overwrite newer local inference diagnostics", async (context) => {
  ui(context);
  state.ocrModels = [model()];
  let finishList;
  invoke = async (command, args) => {
    if (command === "provider_status") return { profiles: [], networkEnabled: false };
    if (command === "list_ocr_models") return new Promise((resolve) => { finishList = resolve; });
    assert.equal(command, "check_ocr_model");
    assert.equal(args.inference, true);
    return model({ runtimeState: "ready", inferenceState: "ready" });
  };
  const refresh = refreshProviders();
  await runOcrModelCheck(model().preset.id, true);
  finishList([model()]);
  await refresh;
  assert.equal(state.ocrModels[0].inferenceState, "ready");
});
