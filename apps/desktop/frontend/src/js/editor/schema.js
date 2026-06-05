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
      const isSet = cur !== undefined && cur !== null;
      const type = Array.isArray(spec.type) ? spec.type[0] : spec.type;
      const desc = spec.description || "";
      const hasDefault = Object.prototype.hasOwnProperty.call(spec, "default");
      const examples = Array.isArray(spec.examples) ? spec.examples : [];
      let control = "";
      let typeHint = "";
      const val = !isSet ? "" : typeof cur === "string" ? cur : JSON.stringify(cur);
      // Placeholder hint: prefer schema default, then first example.
      let placeholder = "";
      if (!isSet && hasDefault && spec.default !== null && spec.default !== undefined) {
        placeholder = typeof spec.default === "string" ? spec.default : JSON.stringify(spec.default);
      } else if (!isSet && examples.length) {
        placeholder = typeof examples[0] === "string" ? examples[0] : JSON.stringify(examples[0]);
      }
      const ph = placeholder ? ` placeholder="${html(placeholder)}"` : "";

      if (Array.isArray(spec.enum) && spec.enum.length) {
        // Preselect current value, else schema default.
        const selected = isSet ? cur : hasDefault ? spec.default : undefined;
        const options = spec.enum
          .map((opt) => {
            const ov = typeof opt === "string" ? opt : JSON.stringify(opt);
            const isSel = String(opt) === String(selected);
            return `<option value="${html(ov)}"${isSel ? " selected" : ""}>${html(ov)}</option>`;
          })
          .join("");
        control = `<select data-with-key="${html(key)}" data-type="string">${options}</select>`;
      } else if (type === "boolean") {
        control = `<label class="toggle"><input type="checkbox" data-with-key="${html(key)}" data-type="boolean" ${cur ? "checked" : ""}/> ${cur ? "true" : "false"}</label>`;
      } else if (type === "integer" || type === "number") {
        control = `<input type="number" data-with-key="${html(key)}" data-type="${type}" value="${html(val)}"${ph}/>`;
      } else if (type === "object" || type === "array") {
        typeHint = " (JSON)";
        control = `<textarea data-with-key="${html(key)}" data-type="${type}" style="min-height: 60px"${ph}>${html(val)}</textarea>`;
      } else {
        control = `<input type="text" data-with-key="${html(key)}" data-type="string" value="${html(val)}"${ph}/>`;
      }
      return `<div class="prop-field">
        <label>${html(key)}${typeHint} ${required.has(key) ? '<span class="req">●</span>' : ""}</label>
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
