// ESLint 9 flat config for the LumoRPA desktop frontend.
// Plain browser ESM (no bundler) — talks to Tauri via `window.__TAURI__`.

import js from "@eslint/js";
import globals from "globals";

export default [
  {
    ignores: ["node_modules/**"],
  },
  js.configs.recommended,
  {
    files: ["src/js/**/*.js"],
    languageOptions: {
      ecmaVersion: 2022,
      sourceType: "module",
      globals: {
        ...globals.browser,
      },
    },
    rules: {
      // The codebase intentionally uses empty `catch {}` blocks to tolerate
      // missing IPC / clipboard on older builds.
      "no-empty": ["error", { allowEmptyCatch: true }],
      // Allow `catch (_) {}` / unused leading args without flagging.
      "no-unused-vars": ["error", { argsIgnorePattern: "^_", varsIgnorePattern: "^_", caughtErrors: "none" }],
    },
  },
  {
    // The flat config itself is a Node ESM module.
    files: ["eslint.config.js"],
    languageOptions: {
      sourceType: "module",
      globals: { ...globals.node },
    },
  },
];
