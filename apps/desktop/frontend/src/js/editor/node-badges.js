// Per-step control-metadata badges, rendered in a node footer on the canvas.
//
// Kept free of DOM and app state on purpose: producing a badge list is pure
// data-to-markup work, and isolating it means it can be tested directly instead
// of through a rendered SVG tree.

/** Escape a value for safe interpolation into markup. */
function esc(v) {
  return String(v ?? "")
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;")
    .replace(/\u0027/g, "&#39;");
}

/**
 * Describe the footer badges for one step.
 *
 * Returns plain descriptors so callers can render or assert on them without
 * parsing markup.
 */
export function stepBadges(step) {
  const badges = [];
  if (!step || typeof step !== "object") return badges;

  if (step.retry && typeof step.retry === "object") {
    badges.push({ kind: "retry", text: `retry × ${step.retry.times ?? 1}` });
  }

  const oe = step.on_error;
  if (oe && typeof oe === "object" && oe.mode && oe.mode !== "interrupt") {
    // Only non-default modes are badged. An explicit `interrupt` behaves
    // exactly like the absent field, so badging it would advertise a
    // difference that does not exist.
    badges.push({
      kind: "on-error",
      mode: oe.mode,
      text: oe.mode === "continue" ? "忽略错误" : `重试 × ${oe.retry_count ?? 0}`,
    });
  }
  return badges;
}

/** Render the badges for one step as the node footer markup (empty when none). */
export function renderStepFoot(step) {
  const badges = stepBadges(step);
  if (!badges.length) return "";
  const html = badges
    .map((b) =>
      b.kind === "retry"
        ? `<span class="badge retry">${esc(b.text)}</span>`
        : `<span class="badge on-error ${esc(b.mode)}">${esc(b.text)}</span>`,
    )
    .join("");
  return `<div class="graph-node-foot">${html}</div>`;
}
