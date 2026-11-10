// Step policies are separate from an action's `with` schema.
import { html } from "../dom.js";

export const RETRY_ERROR_KINDS = [
  "selector_not_found", "extract_failed", "cond_error", "capability_denied",
  "budget_exceeded", "io", "network", "timeout", "other",
];

export function advancedFormValues(step = {}) {
  return {
    when: step.when ?? "",
    bind: step.bind ?? "",
    retryEnabled: step.retry != null,
    retryTimes: step.retry?.times ?? 0,
    retryBackoff: step.retry?.backoff ?? "fixed",
    retryInitialMs: step.retry?.initial_ms ?? 500,
    retryOn: step.retry?.on ?? [],
    errorMode: step.on_error?.mode ?? "interrupt",
    errorRetryCount: step.on_error?.retry_count ?? 0,
    errorRetryIntervalMs: step.on_error?.retry_interval_ms ?? 1000,
    errorOutputEnabled: Object.hasOwn(step.on_error ?? {}, "output"),
    errorOutput: Object.hasOwn(step.on_error ?? {}, "output") ? JSON.stringify(step.on_error.output, null, 2) : "",
  };
}

function option(value, label, selected) {
  return `<option value="${html(value)}"${value === selected ? " selected" : ""}>${label}</option>`;
}

export function renderAdvancedFields(step) {
  const v = advancedFormValues(step);
  const errors = [...new Set([...RETRY_ERROR_KINDS, ...v.retryOn])];
  return `<details class="prop-advanced"${v.when || v.bind || v.retryEnabled || step.on_error ? " open" : ""}>
    <summary>条件、结果与错误处理</summary>
    <div class="prop-field"><label for="stepWhen">执行条件</label>
      <input id="stepWhen" data-advanced="when" value="${html(v.when)}" placeholder="例如 order.total &gt; 0" />
      <span class="hint">留空时始终执行。</span></div>
    <div class="prop-field"><label for="stepBind">结果变量</label>
      <input id="stepBind" data-advanced="bind" value="${html(v.bind)}" placeholder="例如 order_result" /></div>
    <fieldset><legend><label><input type="checkbox" data-advanced="retryEnabled"${v.retryEnabled ? " checked" : ""} /> 失败重试</label></legend>
      <div data-advanced-retry${v.retryEnabled ? "" : " hidden"}>
        <div class="prop-field"><label>重试次数</label><input type="number" min="0" max="4294967295" step="1" data-advanced="retryTimes" value="${html(v.retryTimes)}" /></div>
        <div class="prop-field"><label>等待方式</label><select data-advanced="retryBackoff">${option("fixed", "固定间隔", v.retryBackoff)}${option("exponential", "指数退避", v.retryBackoff)}</select></div>
        <div class="prop-field"><label>初始间隔（毫秒）</label><input type="number" min="0" step="1" data-advanced="retryInitialMs" value="${html(v.retryInitialMs)}" /></div>
        <div class="prop-field"><label>重试错误类型</label>${errors.map((kind) => `<label><input type="checkbox" data-retry-kind="${html(kind)}"${v.retryOn.includes(kind) ? " checked" : ""} /> ${html(kind)}</label>`).join("")}
          <span class="hint">未选择时重试除超时以外的错误。超时需要单独勾选。</span></div>
      </div>
    </fieldset>
    <div class="prop-field"><label>重试策略用尽后</label><select data-advanced="errorMode">${option("interrupt", "终止流程", v.errorMode)}${option("continue", "记录错误并继续", v.errorMode)}${option("retry", "额外重试，仍失败则终止", v.errorMode)}</select></div>
    <div data-advanced-error-retry${v.errorMode === "retry" ? "" : " hidden"}>
      <div class="prop-field"><label>额外重试次数</label><input type="number" min="0" max="4294967295" step="1" data-advanced="errorRetryCount" value="${html(v.errorRetryCount)}" /></div>
      <div class="prop-field"><label>额外重试间隔（毫秒）</label><input type="number" min="0" step="1" data-advanced="errorRetryIntervalMs" value="${html(v.errorRetryIntervalMs)}" /></div>
    </div>
    <div class="prop-field"><label><input type="checkbox" data-advanced="errorOutputEnabled"${v.errorOutputEnabled ? " checked" : ""} /> 设置失败默认输出</label>
      <textarea data-advanced="errorOutput" aria-label="失败默认输出（JSON）" placeholder='例如 {"status":"failed"}'${v.errorOutputEnabled ? "" : " hidden"}>${html(v.errorOutput)}</textarea>
      <span class="hint">在“记录错误并继续”时交给下游。支持 JSON 对象、数组、字符串、数字、布尔值和 null。</span></div>
  </details>`;
}

export function wireAdvancedFields(root) {
  const get = (key) => root.querySelector(`[data-advanced="${key}"]`);
  const refresh = () => {
    root.querySelector("[data-advanced-retry]").hidden = !get("retryEnabled").checked;
    root.querySelector("[data-advanced-error-retry]").hidden = get("errorMode").value !== "retry";
    get("errorOutput").hidden = !get("errorOutputEnabled").checked;
  };
  for (const key of ["retryEnabled", "errorMode", "errorOutputEnabled"]) get(key).addEventListener("change", refresh);
  refresh();
}

export function readAdvancedFields(root) {
  const values = {};
  for (const el of root.querySelectorAll("[data-advanced]")) {
    values[el.dataset.advanced] = el.type === "checkbox" ? el.checked : el.value;
  }
  values.retryOn = Array.from(root.querySelectorAll("[data-retry-kind]"))
    .filter((el) => el.checked).map((el) => el.dataset.retryKind);
  return values;
}

function unsignedInteger(value, label, max = Number.MAX_SAFE_INTEGER) {
  if (!/^\d+$/.test(String(value))) throw new Error(`${label}必须是非负整数`);
  const number = Number(value);
  if (!Number.isSafeInteger(number) || number > max) throw new Error(`${label}超出允许范围`);
  return number;
}

export function applyAdvancedValues(step, values) {
  const next = { ...step };
  const when = String(values.when ?? "").trim();
  const bind = String(values.bind ?? "").trim();
  if (when) next.when = when;
  else delete next.when;
  if (bind) next.bind = bind;
  else delete next.bind;
  if (values.retryEnabled) {
    if (!["fixed", "exponential"].includes(values.retryBackoff)) throw new Error("请选择有效的重试等待方式");
    const on = (values.retryOn ?? []).map((kind) => kind.trim().toLowerCase());
    if (on.some((kind) => !RETRY_ERROR_KINDS.includes(kind))) throw new Error("存在不支持的重试错误类型");
    next.retry = {
      ...step.retry,
      times: unsignedInteger(values.retryTimes, "重试次数", 4294967295),
      backoff: values.retryBackoff,
      initial_ms: unsignedInteger(values.retryInitialMs, "初始间隔"),
      on: [...new Set(on)],
    };
  } else delete next.retry;

  const mode = values.errorMode;
  if (!["interrupt", "continue", "retry"].includes(mode)) throw new Error("请选择有效的错误处理方式");
  const error = { ...step.on_error, mode };
  if (mode === "retry") {
    error.retry_count = unsignedInteger(values.errorRetryCount, "额外重试次数", 4294967295);
    error.retry_interval_ms = unsignedInteger(values.errorRetryIntervalMs, "额外重试间隔");
  } else {
    delete error.retry_count;
    delete error.retry_interval_ms;
  }
  if (values.errorOutputEnabled) {
    try { error.output = JSON.parse(values.errorOutput); }
    catch { throw new Error("失败默认输出必须是有效的 JSON"); }
  } else delete error.output;
  if (mode === "interrupt" && !Object.hasOwn(error, "output") && Object.keys(error).length === 1) delete next.on_error;
  else next.on_error = error;
  return next;
}
