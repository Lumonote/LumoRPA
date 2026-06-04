// Settings view: environment info + discovered Skills list.

import { $, html, kv } from "./dom.js";
import { call } from "./api.js";
import { state } from "./state.js";

export async function refreshSettings() {
  const [info, skills] = await Promise.all([call("app_info"), call("list_skills")]);
  state.app = info;
  $("environmentBox").innerHTML = [
    kv("版本", info.version),
    kv("平台", `${info.platform} ${info.arch}`),
    kv("应用数据", info.dataDir),
    kv("Providers", info.providersPath),
    kv("Skills 根", info.skillsPath),
    kv("Examples", info.examplesDir || "-"),
    kv("LLM 网络", info.networkEnabled ? "已开启 (本次会话或环境变量)" : "未开启"),
  ].join("");
  $("skillsBox").innerHTML = skills.length
    ? skills.map((s) => kv(s.name, s.description || s.source)).join("")
    : `<div class="kv"><span>暂无</span><strong>把 SKILL.md 放到 ${html(info.skillsPath)}</strong></div>`;
  $("appMeta").textContent = `${info.platform} ${info.arch} · v${info.version}`;
  $("versionPill").lastChild.textContent = `v${info.version}`;
}
