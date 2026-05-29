//! Vision-LLM grounding (S-11/S-12) — the "last resort" leg of the
//! Self-Healing Router (A-14).
//!
//! Flow:
//! 1. The DOM-side resolver in [`crate::selectors::resolve_element`] tries
//!    every fingerprint and fails. The browser action layer notices we have
//!    an `AiHookProvider` plus a natural-language prompt and falls through
//!    to [`resolve_via_vision`].
//! 2. We take a CDP screenshot of the current page. The first attempt asks
//!    the vision LLM for a raw bbox; if it returns one we treat the centre
//!    of that bbox as the click point and do `document.elementFromPoint`.
//! 3. On low confidence (or a `null` bbox) we draw a Set-of-Mark overlay
//!    over every clickable element, screenshot again, and ask the LLM for
//!    a mark *index*. The matching `[data-lumo-mark="N"]` element under the
//!    overlay is then identified and marked with `data-lumo-resolved="1"`
//!    so the action layer can `page.find_element` it just like any other
//!    DOM strategy.
//!
//! The hand-off contract with the LLM lives in
//! [`lumo_ai::helpers::vision_locate`]; everything in this file is the
//! browser-side glue around it. OmniParser v2 / UI-TARS local models slot
//! in as alternative `AiHookProvider` implementations without touching
//! anything here.

use base64::Engine;
use bytes::Bytes;
use chromiumoxide::{
    cdp::browser_protocol::page::{CaptureScreenshotFormat, CaptureScreenshotParams},
    Element, Page,
};
use lumo_core::ai_hook::{AiHookProvider, SoMMark};
use lumo_core::error::StepError;
use std::sync::Arc;
use std::time::Duration;

/// Decision threshold below which the bbox path is considered unreliable
/// and the SoM path is tried. Calibrated against Claude / GPT-4o vision
/// returns — most clean targets come back ≥ 0.75.
const BBOX_CONFIDENCE_FLOOR: f32 = 0.55;

/// Try to resolve `target_description` purely through a Vision-LLM. Returns
/// the matched DOM element plus a strategy name (`vision_bbox` or
/// `vision_som`) that the caller can stash in step output, mirroring how
/// `resolve_element` reports the winning DOM fingerprint.
///
/// Errors are intentionally non-noisy — a `StepError::SelectorNotFound` is
/// returned whenever the vision call comes back without a usable target, so
/// the caller can map it onto a final `SelectorNotFound` for the whole
/// step (matching DOM-only behavior).
pub async fn resolve_via_vision(
    page: &Page,
    provider: &Arc<dyn AiHookProvider>,
    target_description: &str,
    model: Option<&str>,
    timeout_ms: u64,
) -> Result<(Element, &'static str), StepError> {
    // Best-effort cleanup before screenshotting — leftover SoM overlays
    // from a previous step would otherwise show up in the screenshot and
    // confuse the LLM.
    clear_som_overlay(page).await;

    let screenshot = tokio::time::timeout(Duration::from_millis(timeout_ms), screenshot_png(page))
        .await
        .map_err(|_| StepError::SelectorNotFound("vision: screenshot timeout".into()))??;

    // ── Phase 1: bbox path ────────────────────────────────────────────
    let bbox_target = provider
        .vision_locate(screenshot.clone(), target_description, &[], model)
        .await?;
    if let Some(bbox) = bbox_target.bbox {
        if bbox_target.confidence >= BBOX_CONFIDENCE_FLOOR {
            let (cx, cy) = bbox_center(bbox);
            if let Some(el) = mark_element_at_point(page, cx, cy).await? {
                tracing::info!(
                    target: "lumo::vision",
                    "vision_bbox match at ({cx:.0}, {cy:.0}) confidence={:.2}",
                    bbox_target.confidence
                );
                return Ok((el, "vision_bbox"));
            }
        }
    }

    // ── Phase 2: Set-of-Mark fallback ─────────────────────────────────
    let marks = inject_som_overlay(page).await?;
    if marks.is_empty() {
        clear_som_overlay(page).await;
        return Err(StepError::SelectorNotFound(
            "vision: no clickable elements visible for Set-of-Mark fallback".into(),
        ));
    }
    let som_screenshot =
        tokio::time::timeout(Duration::from_millis(timeout_ms), screenshot_png(page))
            .await
            .map_err(|_| StepError::SelectorNotFound("vision: SoM screenshot timeout".into()))??;
    let som_target = provider
        .vision_locate(som_screenshot, target_description, &marks, model)
        .await?;

    let result = if let Some(idx) = som_target.mark_index {
        mark_element_by_index(page, idx).await?
    } else {
        None
    };

    // Always clear overlays; doing so AFTER the lookup so the lookup can
    // use the `[data-lumo-mark]` attribute the overlay set on each target.
    clear_som_overlay(page).await;

    match result {
        Some(el) => {
            tracing::info!(
                target: "lumo::vision",
                "vision_som matched mark {} confidence={:.2}",
                som_target.mark_index.unwrap_or(0),
                som_target.confidence
            );
            Ok((el, "vision_som"))
        }
        None => Err(StepError::SelectorNotFound(format!(
            "vision: LLM could not locate `{target_description}`"
        ))),
    }
}

// ─── Screenshot ─────────────────────────────────────────────────────────

/// Capture a full-page PNG screenshot. Public so non-vision callers (e.g.
/// `browser.extract`) can stash a frame for the `extract_visual` AI hook.
pub async fn screenshot_png(page: &Page) -> Result<Bytes, StepError> {
    let params = CaptureScreenshotParams::builder()
        .format(CaptureScreenshotFormat::Png)
        .build();
    let raw = page
        .screenshot(params)
        .await
        .map_err(|e| StepError::msg(format!("vision: screenshot: {e}")))?;
    Ok(Bytes::from(raw))
}

/// Decode a base64 PNG screenshot into a `Bytes` — used by tests and by
/// callers that already have a base64 string in hand (e.g. cached frames).
pub fn decode_base64_png(b64: &str) -> Result<Bytes, StepError> {
    base64::engine::general_purpose::STANDARD
        .decode(b64)
        .map(Bytes::from)
        .map_err(|e| StepError::msg(format!("vision: base64 decode: {e}")))
}

// ─── Coordinate → element ───────────────────────────────────────────────

fn bbox_center((x, y, w, h): (f32, f32, f32, f32)) -> (f32, f32) {
    (x + w / 2.0, y + h / 2.0)
}

/// Run `document.elementFromPoint(x, y)` and tag the result with
/// `data-lumo-resolved="1"` so callers can `page.find_element` it. Returns
/// `None` when the point is outside the viewport or hits nothing useful.
async fn mark_element_at_point(
    page: &Page,
    x: f32,
    y: f32,
) -> Result<Option<Element>, StepError> {
    let js = format!(
        r#"
(() => {{
  document.querySelectorAll('[data-lumo-resolved]').forEach((el) => el.removeAttribute('data-lumo-resolved'));
  const el = document.elementFromPoint({x}, {y});
  if (!el || el.nodeType !== 1) return false;
  el.setAttribute('data-lumo-resolved', '1');
  return true;
}})()
"#
    );
    let result = page
        .evaluate(js)
        .await
        .map_err(|e| StepError::msg(format!("vision: elementFromPoint: {e}")))?;
    let ok: bool = result.into_value().unwrap_or(false);
    if !ok {
        return Ok(None);
    }
    match page.find_element("[data-lumo-resolved='1']").await {
        Ok(el) => Ok(Some(el)),
        Err(_) => Ok(None),
    }
}

/// Look up an element previously tagged with `data-lumo-mark="N"` by
/// [`inject_som_overlay`] and promote it to `data-lumo-resolved="1"`.
async fn mark_element_by_index(
    page: &Page,
    index: u32,
) -> Result<Option<Element>, StepError> {
    let js = format!(
        r#"
(() => {{
  document.querySelectorAll('[data-lumo-resolved]').forEach((el) => el.removeAttribute('data-lumo-resolved'));
  const el = document.querySelector('[data-lumo-mark="{index}"]');
  if (!el || el.nodeType !== 1) return false;
  el.setAttribute('data-lumo-resolved', '1');
  return true;
}})()
"#
    );
    let result = page
        .evaluate(js)
        .await
        .map_err(|e| StepError::msg(format!("vision: mark lookup: {e}")))?;
    let ok: bool = result.into_value().unwrap_or(false);
    if !ok {
        return Ok(None);
    }
    match page.find_element("[data-lumo-resolved='1']").await {
        Ok(el) => Ok(Some(el)),
        Err(_) => Ok(None),
    }
}

// ─── Set-of-Mark overlay ────────────────────────────────────────────────

/// JS that builds the SoM overlay. We pick every element that looks
/// interactive (rough heuristic — buttons, links, inputs, role=button),
/// skip anything off-screen or zero-sized, then draw a coloured chip with
/// the 1-based index over each one. The marker element gets
/// `data-lumo-mark="N"` so the resolver can map LLM output back to it.
///
/// The function returns the array of marks for the host side to forward to
/// `vision_locate`. We cap at 80 marks — beyond that the screenshot turns
/// to noise and the LLM is unlikely to produce a stable answer anyway.
const INJECT_SOM_JS: &str = r#"
(() => {
  const MAX_MARKS = 80;
  const selectors = [
    'a[href]',
    'button',
    'input:not([type=hidden])',
    'select',
    'textarea',
    '[role="button"]',
    '[role="link"]',
    '[role="tab"]',
    '[role="menuitem"]',
    '[tabindex]:not([tabindex="-1"])',
  ];
  // Tear down any leftovers from a prior pass — idempotent re-entry.
  document.querySelectorAll('[data-lumo-mark]').forEach((el) => el.removeAttribute('data-lumo-mark'));
  const old = document.getElementById('__lumo_som_layer__');
  if (old) old.remove();

  const layer = document.createElement('div');
  layer.id = '__lumo_som_layer__';
  layer.style.cssText = 'position:fixed;inset:0;pointer-events:none;z-index:2147483647;font-family:system-ui,sans-serif;';
  document.body.appendChild(layer);

  const isVisible = (el) => {
    const r = el.getBoundingClientRect();
    if (r.width < 8 || r.height < 8) return false;
    if (r.right < 0 || r.bottom < 0) return false;
    const vw = window.innerWidth || document.documentElement.clientWidth;
    const vh = window.innerHeight || document.documentElement.clientHeight;
    if (r.left > vw || r.top > vh) return false;
    const style = window.getComputedStyle(el);
    if (style.visibility === 'hidden' || style.display === 'none' || parseFloat(style.opacity) === 0) return false;
    return true;
  };

  const labelOf = (el) => {
    const aria = el.getAttribute && el.getAttribute('aria-label');
    if (aria) return aria.trim().slice(0, 40);
    const ph = el.getAttribute && el.getAttribute('placeholder');
    if (ph) return ph.trim().slice(0, 40);
    const txt = (el.innerText || el.value || '').trim();
    if (txt) return txt.slice(0, 40);
    return el.tagName ? el.tagName.toLowerCase() : '';
  };

  const seen = new Set();
  const marks = [];
  for (const sel of selectors) {
    for (const el of document.querySelectorAll(sel)) {
      if (seen.has(el)) continue;
      if (!isVisible(el)) continue;
      seen.add(el);
      const idx = marks.length + 1;
      const r = el.getBoundingClientRect();
      el.setAttribute('data-lumo-mark', String(idx));
      const chip = document.createElement('div');
      chip.style.cssText = [
        'position:absolute',
        'left:' + Math.round(r.left) + 'px',
        'top:' + Math.round(r.top) + 'px',
        'background:#ff3b30',
        'color:#fff',
        'font-size:12px',
        'font-weight:600',
        'padding:1px 4px',
        'border-radius:3px',
        'border:1px solid #fff',
        'line-height:1.1',
        'box-shadow:0 0 0 1px rgba(0,0,0,0.6)',
      ].join(';');
      chip.textContent = String(idx);
      layer.appendChild(chip);
      const box = document.createElement('div');
      box.style.cssText = [
        'position:absolute',
        'left:' + Math.round(r.left) + 'px',
        'top:' + Math.round(r.top) + 'px',
        'width:' + Math.round(r.width) + 'px',
        'height:' + Math.round(r.height) + 'px',
        'outline:1.5px solid #ff3b30',
        'background:rgba(255,59,48,0.06)',
      ].join(';');
      layer.appendChild(box);
      marks.push({
        index: idx,
        x: r.left, y: r.top, w: r.width, h: r.height,
        tag: el.tagName ? el.tagName.toLowerCase() : '',
        label: labelOf(el),
      });
      if (marks.length >= MAX_MARKS) break;
    }
    if (marks.length >= MAX_MARKS) break;
  }
  return marks;
})()
"#;

const CLEAR_SOM_JS: &str = r#"
(() => {
  document.querySelectorAll('[data-lumo-mark]').forEach((el) => el.removeAttribute('data-lumo-mark'));
  const old = document.getElementById('__lumo_som_layer__');
  if (old) old.remove();
})()
"#;

#[derive(serde::Deserialize)]
struct RawMark {
    index: u32,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    #[serde(default)]
    tag: String,
    #[serde(default)]
    label: String,
}

pub async fn inject_som_overlay(page: &Page) -> Result<Vec<SoMMark>, StepError> {
    let result = page
        .evaluate(INJECT_SOM_JS)
        .await
        .map_err(|e| StepError::msg(format!("vision: SoM inject: {e}")))?;
    let raw: Vec<RawMark> = result
        .into_value()
        .map_err(|e| StepError::msg(format!("vision: SoM decode: {e}")))?;
    Ok(raw
        .into_iter()
        .map(|m| SoMMark {
            index: m.index,
            bbox: (m.x, m.y, m.w, m.h),
            tag: m.tag,
            label: m.label,
        })
        .collect())
}

pub async fn clear_som_overlay(page: &Page) {
    let _ = page.evaluate(CLEAR_SOM_JS).await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bbox_center_picks_midpoint() {
        let (cx, cy) = bbox_center((10.0, 20.0, 100.0, 40.0));
        assert!((cx - 60.0).abs() < 1e-3);
        assert!((cy - 40.0).abs() < 1e-3);
    }

    #[test]
    fn decode_base64_png_round_trips_bytes() {
        let raw = b"\x89PNG\r\n\x1a\nfake";
        let encoded = base64::engine::general_purpose::STANDARD.encode(raw);
        let decoded = decode_base64_png(&encoded).unwrap();
        assert_eq!(&decoded[..], raw);
    }

    #[test]
    fn decode_base64_png_rejects_invalid_input() {
        let err = decode_base64_png("not~base64!!").unwrap_err();
        assert!(format!("{err}").contains("base64 decode"));
    }

    #[test]
    fn inject_som_script_contains_overlay_id() {
        // Sanity check: the script must build the layer element we later
        // tear down — otherwise `clear_som_overlay` is a no-op.
        assert!(INJECT_SOM_JS.contains("__lumo_som_layer__"));
        assert!(CLEAR_SOM_JS.contains("__lumo_som_layer__"));
    }

    #[test]
    fn inject_som_script_marks_data_attr() {
        // The Rust-side resolver looks elements up via `[data-lumo-mark="N"]`,
        // so the JS must set that attribute on each candidate.
        assert!(INJECT_SOM_JS.contains("data-lumo-mark"));
        assert!(INJECT_SOM_JS.contains("setAttribute('data-lumo-mark'"));
    }

    #[test]
    fn inject_som_script_returns_mark_payload() {
        // The Rust deserializer expects `index`, `x`, `y`, `w`, `h`, `tag`,
        // `label`. Drift between JS and Rust breaks vision fallback silently.
        for key in ["index:", "x:", "y:", "w:", "h:", "tag:", "label:"] {
            assert!(
                INJECT_SOM_JS.contains(key),
                "INJECT_SOM_JS missing field `{key}`"
            );
        }
    }

    #[test]
    fn bbox_confidence_floor_is_in_unit_interval() {
        assert!((0.0..=1.0).contains(&BBOX_CONFIDENCE_FLOOR));
    }
}
