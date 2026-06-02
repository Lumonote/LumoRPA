// Tauri command surface. Wraps `window.__TAURI__.core.invoke` so the rest of the
// app calls `call(cmd, args)` and gets a clear error when running outside Tauri.

export const invoke = window.__TAURI__?.core?.invoke;

export async function call(cmd, args = {}) {
  if (!invoke) throw new Error("Tauri API unavailable (run via `cargo tauri dev`)");
  return invoke(cmd, args);
}
