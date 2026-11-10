const html = (value) => String(value ?? "").replaceAll("&", "&amp;").replaceAll("<", "&lt;").replaceAll(">", "&gt;").replaceAll('"', "&quot;").replaceAll("'", "&#39;");

export function renderInvocationUsage(rows = []) {
  if (!rows.length) return '<p>本次运行没有调用用量记录。</p>';
  return `<table><thead><tr><th>节点 / 尝试</th><th>状态</th><th>Tokens</th><th>费用（美元）</th><th>核对</th></tr></thead><tbody>${rows.map((row, index) => {
    const unknown = row.usageState !== "recorded";
    const tokens = row.tokensUsed == null ? "未知" : `${unknown ? "至少 " : ""}${row.tokensUsed}`;
    const cost = row.costUsdMicro == null ? "未知" : `${unknown ? "至少 " : ""}${(row.costUsdMicro / 1_000_000).toFixed(6)}`;
    return `<tr><td>${html(row.nodeId)} / ${html(row.attempt)}</td><td>${html(row.state)} · ${unknown ? "用量待核对" : "已记录"}</td><td>${html(tokens)}</td><td>${html(cost)}</td><td>${unknown && row.state !== "running" ? `<button type="button" data-resolve-usage="${index}">核对账单</button>` : html(row.resolvedBy || "—")}</td></tr>`;
  }).join("")}</tbody></table><p>待核对的用量可能包含尚未上报的费用。请查阅模型服务账单后填写本次尝试的最终总量。</p>`;
}

export function createUsageResolution(row, values) {
  const parse = (value, label) => {
    const text = String(value ?? "").trim();
    if (!/^\d+$/.test(text) || !Number.isSafeInteger(Number(text))) throw new Error(`${label}必须是非负整数`);
    return Number(text);
  };
  const tokensUsed = parse(values.tokensUsed, "Tokens");
  const costUsdMicro = parse(values.costUsdMicro, "费用");
  const resolvedBy = String(values.resolvedBy || "").trim();
  const reason = String(values.reason || "").trim();
  if (!resolvedBy || !reason) throw new Error("请填写核对人和账单依据");
  if (tokensUsed < (row.tokensUsed ?? 0) || costUsdMicro < (row.costUsdMicro ?? 0)) throw new Error("最终总量不能小于已记录用量");
  return { runId: row.runId, nodeId: row.nodeId, attempt: row.attempt, expectedRevision: row.revision, tokensUsed, costUsdMicro, resolvedBy, reason };
}

export function mountInvocationUsage(root, call, selectedRun) {
  const head = root.querySelector(".mc-head");
  if (!head) return;
  const button = document.createElement("button");
  button.type = "button";
  button.textContent = "用量明细";
  head.append(button);
  button.addEventListener("click", async () => {
    const runId = selectedRun();
    if (!runId || button.disabled) return;
    button.disabled = true;
    const dialog = document.createElement("dialog");
    dialog.className = "usage-dialog";
    dialog.innerHTML = `<h3>运行用量</h3><p>${html(runId)}</p><div data-usage-rows>加载中…</div><div data-usage-form></div><p data-usage-error role="alert"></p><button type="button" data-usage-close>关闭</button>`;
    document.body.append(dialog);
    const error = dialog.querySelector("[data-usage-error]");
    const rowsBox = dialog.querySelector("[data-usage-rows]");
    const formBox = dialog.querySelector("[data-usage-form]");
    let rows = [];
    let busy = false;
    const refresh = async () => { rows = await call("agent_usage", { runId }); rowsBox.innerHTML = renderInvocationUsage(rows); };
    dialog.querySelector("[data-usage-close]").addEventListener("click", () => { if (!busy) dialog.remove(); });
    dialog.addEventListener("cancel", (event) => { if (busy) event.preventDefault(); else dialog.remove(); });
    dialog.addEventListener("click", (event) => {
      const target = event.target.closest("[data-resolve-usage]");
      if (!target || busy) return;
      const row = rows[Number(target.dataset.resolveUsage)];
      if (!row) return;
      formBox.innerHTML = `<form data-usage-resolution><h4>核对 ${html(row.nodeId)} · 第 ${html(row.attempt)} 次尝试</h4><label>最终 Tokens 总量<input name="tokensUsed" type="number" min="${row.tokensUsed ?? 0}" step="1" required value="${row.tokensUsed ?? 0}"></label><label>最终费用（微美元，1 美元 = 1000000）<input name="costUsdMicro" type="number" min="${row.costUsdMicro ?? 0}" step="1" required value="${row.costUsdMicro ?? 0}"></label><label>核对人<input name="resolvedBy" required></label><label>账单依据<textarea name="reason" required></textarea></label><button type="submit">保存核对结果</button></form>`;
      const form = formBox.querySelector("form");
      form.addEventListener("submit", async (submit) => {
        submit.preventDefault();
        if (busy) return;
        try {
          const values = Object.fromEntries(new FormData(form));
          const resolution = createUsageResolution(row, values);
          busy = true;
          error.textContent = "";
          for (const control of form.elements) control.disabled = true;
          await call("agent_resolve_usage", { resolution });
          formBox.innerHTML = "";
          await refresh();
        } catch (cause) {
          error.textContent = cause instanceof Error ? cause.message : String(cause);
          for (const control of form.elements) control.disabled = false;
        } finally { busy = false; }
      });
    });
    if (typeof dialog.showModal === "function") dialog.showModal(); else dialog.setAttribute("open", "");
    try { await refresh(); } catch (cause) { error.textContent = `加载用量失败：${cause instanceof Error ? cause.message : String(cause)}`; }
    finally { button.disabled = false; }
  });
}
