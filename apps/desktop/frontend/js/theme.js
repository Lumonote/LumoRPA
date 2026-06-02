// Theme (light/dark/auto) + window/panel opacity presets.

import { $, $$ } from "./dom.js";
import { call } from "./api.js";
import { state } from "./state.js";
import { PRESETS } from "./constants.js";

export function applyTheme(value) {
  state.theme = value;
  localStorage.setItem("lumo.theme", value);
  const root = document.documentElement;
  const dark =
    value === "dark" ||
    (value === "auto" && window.matchMedia("(prefers-color-scheme: dark)").matches);
  root.classList.toggle("theme-dark", dark);
  $$("[data-theme]").forEach((b) => b.classList.toggle("is-active", b.dataset.theme === value));
}

export function applyPanelAlpha(percent) {
  const clamped = Math.max(20, Math.min(100, Number(percent)));
  state.panelAlpha = clamped;
  localStorage.setItem("lumo.panel", String(clamped));
  document.documentElement.style.setProperty("--panel-alpha", String(clamped / 100));
  ["panelAlphaTop", "panelAlphaSlider"].forEach((id) => {
    const el = $(id);
    if (el && Number(el.value) !== clamped) el.value = String(clamped);
  });
  const lbl = $("panelAlphaValue");
  if (lbl) lbl.textContent = `${clamped}%`;
}

export function applyWindowAlpha(percent) {
  const clamped = Math.max(0, Math.min(100, Number(percent)));
  state.windowAlpha = clamped;
  localStorage.setItem("lumo.win", String(clamped));
  document.documentElement.style.setProperty("--window-alpha", String(clamped / 100));
  ["windowAlphaTop", "windowAlphaSlider"].forEach((id) => {
    const el = $(id);
    if (el && Number(el.value) !== clamped) el.value = String(clamped);
  });
  const lbl = $("windowAlphaValue");
  if (lbl) lbl.textContent = `${clamped}%`;
  // Drive the actual Tauri window background alpha (0..=255) so the OS-level
  // vibrancy is exposed to whatever extent the user wants.
  const alpha = Math.round((clamped / 100) * 255);
  call("set_window_alpha", { options: { alpha } }).catch(() => {});
}

export function applyPreset(name) {
  const preset = PRESETS[name];
  if (!preset) return;
  applyWindowAlpha(preset.window);
  applyPanelAlpha(preset.panel);
  $$(".preset-row [data-preset]").forEach((b) => b.classList.toggle("is-active", b.dataset.preset === name));
}
