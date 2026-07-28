// P0-2 纯逻辑助手：human-prompt 回执值归一化 / 取消目标挑选 / 倒计时格式化。
// 不依赖 DOM 与 Tauri，供 node --test 直接单测。

/// 把用户在模态里的原始输入归一成后端 `human_respond` 期望的 value 形状：
/// input → 字符串；confirm / approve → 布尔（裸 bool 直通，字符串按
/// y/yes/true/1/是 判真，其余判假）。参照 lib.rs `decode_human_response`：
/// 裸 string/bool 直通引擎。
export function normalizeHumanValue(kind, raw) {
  if (kind === "input") return String(raw ?? "");
  if (typeof raw === "boolean") return raw;
  const s = String(raw ?? "").trim().toLowerCase();
  return s === "y" || s === "yes" || s === "true" || s === "1" || s === "是";
}

/// 从 `list_runs` 结果里挑出仍在进行中的 run id（state === "running"）。
/// 引擎在 run 启动时即以 running 状态落库（vm.rs create_run），所以取消
/// 按钮点按时查一次运行列表即可拿到进行中 run_id，无需前端后台轮询。
export function pickRunningRunIds(runs) {
  return (Array.isArray(runs) ? runs : [])
    .filter((r) => r && r.state === "running" && r.id)
    .map((r) => r.id);
}

/// 剩余毫秒 → 中文倒计时文案（向上取整到秒，负数钳为 0）。
export function formatCountdown(remainMs) {
  const total = Math.max(0, Math.ceil(Number(remainMs || 0) / 1000));
  const m = Math.floor(total / 60);
  const s = total % 60;
  return m > 0 ? `${m}分${String(s).padStart(2, "0")}秒` : `${s}秒`;
}
