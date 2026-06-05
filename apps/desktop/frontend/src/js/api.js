// Tauri command surface. Wraps `window.__TAURI__.core.invoke` so the rest of the
// app calls `call(cmd, args)` and gets a clear error when running outside Tauri.

export const invoke = window.__TAURI__?.core?.invoke;

export async function call(cmd, args = {}) {
  if (!invoke) throw new Error("Tauri API unavailable (run via `cargo tauri dev`)");
  return invoke(cmd, args);
}

export function errorMessage(error) {
  if (error instanceof Error) return error.message;
  if (typeof error === "string") return error;
  if (error && typeof error === "object") {
    const parts = [];
    for (const key of ["message", "error", "reason", "details"]) {
      if (typeof error[key] === "string" && error[key].trim()) parts.push(error[key].trim());
    }
    if (parts.length) return parts.join("\n");
    try {
      return JSON.stringify(error, null, 2);
    } catch {
      return String(error);
    }
  }
  return String(error);
}
