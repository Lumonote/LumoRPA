// Feature-map view: roadmap sections + per-item status badges.

import { $, html } from "./dom.js";
import { call } from "./api.js";
import { state } from "./state.js";

export async function loadFeatureMap() {
  state.features = await call("feature_map");
}

export function renderFeatures() {
  const grid = $("featureGrid");
  if (!state.features.length) { grid.innerHTML = ""; return; }
  grid.innerHTML = state.features
    .map(
      (sec) => `<div class="feature-section">
      <h3>${html(sec.title)}</h3>
      <div class="feature-list">
        ${sec.items
          .map(
            (it) => `<div class="feature-item">
            <span class="fid">${html(it.id)}</span>
            <div>
              <div class="ftitle">${html(it.title)}</div>
              <span class="fnote">${html(it.note)} · ${html(it.stage)}</span>
            </div>
            <span class="status-badge ${html(it.status)}">${html(it.status)}</span>
          </div>`
          )
          .join("")}
      </div>
    </div>`
    )
    .join("");
}
