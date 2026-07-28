const escapeHtml = (value) => String(value ?? "").replaceAll("&", "&amp;").replaceAll("<", "&lt;").replaceAll(">", "&gt;").replaceAll('"', "&quot;").replaceAll("'", "&#39;");

export function renderUpdateCenter(status = {}) {
  return `<div class="update-center"><div><span>当前版本</span><strong>${escapeHtml(status.currentVersion || "unknown")}</strong></div><label><span>更新通道</span><select data-update-channel><option value="stable"${status.channel !== "beta" ? " selected" : ""}>Stable</option><option value="beta"${status.channel === "beta" ? " selected" : ""}>Beta</option><option value="rollback">Rollback</option></select></label><div class="update-actions"><button data-update-check${status.configured ? "" : " disabled"}>检查更新</button><button data-update-install hidden>下载并安装</button><button data-update-restart hidden>重启完成更新</button></div><pre data-update-result>${status.configured ? "签名更新服务已配置" : "请配置 HTTPS endpoint 与 LUMO_UPDATER_PUBKEY"}</pre><div class="vc-progress"><i data-update-progress style="width:0%"></i></div></div>`;
}

export function updateProgressPercent(downloaded, total) {
  if (!total) return 0;
  return Math.max(0, Math.min(100, Math.round(Number(downloaded || 0) / Number(total) * 100)));
}

export function bindUpdateCenter(root, call, listen = globalThis.__TAURI__?.event?.listen) {
  if (!root || root.dataset.bound) return;
  root.dataset.bound = "true";
  const channel = () => root.querySelector("[data-update-channel]")?.value || "stable";
  root.addEventListener("click", async (event) => {
    if (event.target.closest("[data-update-check]")) {
      const result = await call("desktop_update_check", { channel: channel() });
      root.querySelector("[data-update-result]").textContent = result.available ? `发现 ${result.version}\n${result.body || ""}` : "当前已是最新版本";
      root.querySelector("[data-update-install]").hidden = !result.available;
    }
    if (event.target.closest("[data-update-install]")) await call("desktop_update_install", { channel: channel() });
    if (event.target.closest("[data-update-restart]")) await call("desktop_update_restart");
  });
  listen?.("lumo://update-progress", ({ payload }) => {
    root.querySelector("[data-update-progress]").style.width = `${updateProgressPercent(payload?.downloaded, payload?.total)}%`;
    root.querySelector("[data-update-result]").textContent = String(payload?.phase || "updating").replaceAll("_", " ");
    if (payload?.phase === "restart_required") root.querySelector("[data-update-restart]").hidden = false;
  });
}
