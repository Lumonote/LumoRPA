// Action JSON Schema → form fields, and reading those fields back into a `with`
// object. Shared by the inspector and the steps view's inline expand.

import { $$, html } from "../dom.js";
import { call } from "../api.js";
import { state } from "../state.js";

export async function loadSchema(actionId) {
  if (state.schemaCache.has(actionId)) return state.schemaCache.get(actionId);
  const schema = await call("action_schema", { id: actionId });
  state.schemaCache.set(actionId, schema);
  return schema;
}

export function renderSchemaFields(schema, withValue) {
  const props = schema?.properties || {};
  const required = new Set(schema?.required || []);
  return Object.entries(props)
    .map(([key, spec]) => {
      const cur = withValue?.[key];
      const type = Array.isArray(spec.type) ? spec.type[0] : spec.type;
      const desc = spec.description || "";
      let control = "";
      const val = cur === undefined || cur === null ? "" : typeof cur === "string" ? cur : JSON.stringify(cur);
      if (type === "boolean") {
        control = `<label class="toggle"><input type="checkbox" data-with-key="${html(key)}" data-type="boolean" ${cur ? "checked" : ""}/> ${cur ? "true" : "false"}</label>`;
      } else if (type === "integer" || type === "number") {
        control = `<input type="number" data-with-key="${html(key)}" data-type="${type}" value="${html(val)}"/>`;
      } else if (type === "object" || type === "array") {
        control = `<textarea data-with-key="${html(key)}" data-type="${type}" style="min-height: 60px">${html(val)}</textarea>`;
      } else {
        control = `<input type="text" data-with-key="${html(key)}" data-type="string" value="${html(val)}"/>`;
      }
      return `<div class="prop-field">
        <label>${html(key)} ${required.has(key) ? '<span class="req">●</span>' : ""}</label>
        ${control}
        ${desc ? `<span class="hint">${html(desc)}</span>` : ""}
      </div>`;
    })
    .join("");
}

export function readWithFromContainer(root) {
  const out = {};
  root.querySelectorAll("[data-with-key]").forEach((el) => {
    const key = el.dataset.withKey;
    const type = el.dataset.type;
    if (type === "boolean") out[key] = el.checked;
    else if (type === "integer" || type === "number") {
      if (el.value === "" || el.value === null) return;
      out[key] = Number(el.value);
    } else if (type === "object" || type === "array") {
      if (!el.value.trim()) return;
      try { out[key] = JSON.parse(el.value); } catch { out[key] = el.value; }
    } else if (el.value !== "") {
      out[key] = el.value;
    }
  });
  return out;
}

export function readInspectorWith() {
  const out = {};
  $$("[data-with-key]").forEach((el) => {
    const key = el.dataset.withKey;
    const type = el.dataset.type;
    if (type === "boolean") {
      out[key] = el.checked;
    } else if (type === "integer" || type === "number") {
      if (el.value === "" || el.value === null) return;
      out[key] = Number(el.value);
    } else if (type === "object" || type === "array") {
      if (!el.value.trim()) return;
      try { out[key] = JSON.parse(el.value); }
      catch { out[key] = el.value; }
    } else {
      if (el.value !== "") out[key] = el.value;
    }
  });
  return out;
}
