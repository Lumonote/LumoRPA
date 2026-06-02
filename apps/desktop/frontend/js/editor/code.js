// Code view: raw YAML textarea + line-number gutter.

import { $ } from "../dom.js";
import { state } from "../state.js";

export function renderCode() {
  const ta = $("codeEditor");
  ta.value = state.source || "";
  syncGutter();
}

export function syncGutter() {
  const ta = $("codeEditor");
  const lines = (ta.value || "").split("\n").length;
  const gutter = $("codeGutter");
  let s = "";
  for (let i = 1; i <= lines; i++) s += i + "\n";
  gutter.textContent = s;
}
