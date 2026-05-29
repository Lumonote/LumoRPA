//! AI hook provider trait. Implemented by `lumo-ai::AiHooks`, injected into
//! the VM via `StepCtx::with_ai(...)`. The VM dispatches at three failure
//! points (post-hook fallback) and one pre-hook (control.if primary):
//!
//! - `StepError::SelectorNotFound` → [`AiHookProvider::heal_selector`]
//! - `StepError::ExtractFailed`    → [`AiHookProvider::extract_visual`]
//! - `StepError::CondError`        → [`AiHookProvider::decide`]
//! - any final failure when `metadata.ai.diagnose_on_failure: true`
//!   → [`AiHookProvider::diagnose`]
//!
//! S-11/S-12 adds a fifth entry point exercised from the browser selector
//! resolver: [`AiHookProvider::vision_locate`] takes a screenshot (plus
//! optional Set-of-Mark numbering) and returns either a pixel bbox or the
//! winning mark index. The resolver maps that back to a DOM element via
//! `document.elementFromPoint`, completing the OmniParser v2 / UI-TARS style
//! last-resort fallback.
//!
//! Defining the trait in `lumo-core` (which `lumo-ai` already depends on)
//! avoids a circular dependency between the two crates.

use crate::error::StepError;
use async_trait::async_trait;
use bytes::Bytes;
use serde_json::Value;

#[derive(Debug, Clone, Default)]
pub struct HealedSelector {
    pub css: Option<String>,
    pub xpath: Option<String>,
    pub bbox: Option<(u32, u32, u32, u32)>,
    pub confidence: f32,
    pub reasoning: String,
}

#[derive(Debug, Clone, Default)]
pub struct Decision {
    pub result: bool,
    pub confidence: f32,
    pub reasoning: String,
}

/// One element overlay drawn by the Set-of-Mark annotator. The browser
/// resolver passes a list of these to the LLM so the answer can be "mark N"
/// rather than raw coordinates — more robust on small / overlapping targets.
#[derive(Debug, Clone)]
pub struct SoMMark {
    /// 1-based index drawn on screen and referenced in the prompt.
    pub index: u32,
    /// CSS-pixel bbox `(x, y, w, h)` of the element under the mark.
    pub bbox: (f32, f32, f32, f32),
    /// Best-effort tag (e.g. "button", "a") for prompt hinting.
    pub tag: String,
    /// Short visible label (innerText / aria-label / placeholder).
    pub label: String,
}

/// Output of the vision-LLM call. Either a pixel bbox in the screenshot
/// coordinate system *or* an index into the [`SoMMark`] list — both fields
/// being `None` means "could not locate".
#[derive(Debug, Clone, Default)]
pub struct LocatedTarget {
    pub bbox: Option<(f32, f32, f32, f32)>,
    pub mark_index: Option<u32>,
    pub confidence: f32,
    pub reasoning: String,
}

#[async_trait]
pub trait AiHookProvider: Send + Sync {
    async fn heal_selector(
        &self,
        failed_selector: &str,
        prompt: &str,
        page_dom_excerpt: Option<&str>,
        model: Option<&str>,
    ) -> Result<HealedSelector, StepError>;

    async fn extract_visual(
        &self,
        screenshot_png: Option<Bytes>,
        target_description: &str,
        page_text_excerpt: Option<&str>,
        schema: Option<&Value>,
        model: Option<&str>,
    ) -> Result<Value, StepError>;

    async fn decide(
        &self,
        vars_snapshot: &Value,
        prompt: &str,
        model: Option<&str>,
    ) -> Result<Decision, StepError>;

    async fn diagnose(
        &self,
        step_id: &str,
        action: &str,
        error: &str,
        model: Option<&str>,
    ) -> Result<String, StepError>;

    /// S-11/S-12: locate `target_description` inside a screenshot. When
    /// `marks` is empty the provider should answer with a `bbox`; when it
    /// contains a numbered overlay the provider should answer with a
    /// `mark_index` instead. Implementations that lack vision support may
    /// return an empty [`LocatedTarget`] (both bbox and mark_index `None`)
    /// — the caller treats that as "could not locate" and falls through.
    async fn vision_locate(
        &self,
        screenshot_png: Bytes,
        target_description: &str,
        marks: &[SoMMark],
        model: Option<&str>,
    ) -> Result<LocatedTarget, StepError>;
}
