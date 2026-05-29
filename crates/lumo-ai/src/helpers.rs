//! AI helper actions invoked by VM hooks. These are NOT registered into the
//! user-facing `ActionRegistry`; they are direct function calls from
//! `lumo-core::vm` at the four AI insertion points:
//!
//! - `heal_selector`  — selector failure → vision/text reasoning to re-locate
//! - `extract_visual` — extract failure → LLM "look at screenshot" (currently text-only)
//! - `decide`         — `control.if` cond missing/error → LLM semantic decision
//! - `diagnose`       — final failure + `metadata.ai.diagnose_on_failure: true`
//!
//! All four pull from the shared `AiRouter` and respect a `RunBudget`.

use crate::budget::RunBudget;
use crate::{
    cost::cost_micro,
    provider::{ChatMessage, ChatRequest, ChatResponse, ImageAttachment, Role},
    router::AiRouter,
};
use async_trait::async_trait;
use base64::Engine;
use bytes::Bytes;
use lumo_core::ai_hook::{
    AiCallUsage, AiHookProvider, Decision, HealedSelector, LocatedTarget, SoMMark,
};
use lumo_core::error::StepError;
use parking_lot::Mutex;
use serde::Deserialize;
use serde_json::Value;
use std::sync::Arc;
use std::time::Instant;

/// Maximum size (in bytes) of a screenshot we will base64-encode and ship to a
/// vision model. Guards against accidentally inlining a multi-megabyte image
/// into a prompt (provider rejection / cost blowups). 4 MiB.
const MAX_IMAGE_BYTES: usize = 4 * 1024 * 1024;

/// Reject screenshots larger than [`MAX_IMAGE_BYTES`] before encoding. The
/// error names the actual vs max size so callers can downscale.
fn guard_image_size(label: &str, len: usize) -> Result<(), StepError> {
    if len > MAX_IMAGE_BYTES {
        return Err(StepError::msg(format!(
            "{label}: image too large ({len} bytes > max {MAX_IMAGE_BYTES} bytes / 4 MiB); \
             downscale or crop the screenshot before sending to the vision model"
        )));
    }
    Ok(())
}

/// P1-4: build the metered [`AiCallUsage`] record for a completed hook chat
/// round-trip. `helper` is the insertion-point name written to the
/// `ai_calls.helper` column; the cost is derived from the resolved
/// provider/model and the response token counts.
fn usage_of(helper: &str, resp: &ChatResponse, latency_ms: i64) -> AiCallUsage {
    AiCallUsage {
        helper: helper.to_string(),
        provider: resp.provider.clone(),
        model: resp.model.clone(),
        input_tokens: resp.input_tokens,
        output_tokens: resp.output_tokens,
        latency_ms,
        cost_usd_micro: cost_micro(
            &resp.provider,
            &resp.model,
            resp.input_tokens,
            resp.output_tokens,
        ),
    }
}

/// Insertion point ①. Note: P0 sends only text context to the LLM;
/// `screenshot_png` is accepted for future multimodal upgrades.
pub async fn heal_selector(
    router: &AiRouter,
    budget: &RunBudget,
    _screenshot_png: Option<Bytes>,
    failed_selector: &str,
    prompt: &str,
    page_dom_excerpt: Option<&str>,
    model: Option<&str>,
) -> Result<(HealedSelector, AiCallUsage), StepError> {
    budget
        .consume()
        .map_err(|_| StepError::BudgetExceeded { max: budget.max() })?;

    let system = "You are a CSS/XPath selector self-healing assistant for RPA automation. \
                  Given a failed selector and a natural-language target description, propose a more \
                  robust selector. Respond with STRICT JSON only (no Markdown fences): \
                  {\"css\": string|null, \"xpath\": string|null, \"confidence\": number 0..1, \"reasoning\": string}.";
    let mut user = format!("Failed selector: {failed_selector}\nTarget: {prompt}\n");
    if let Some(excerpt) = page_dom_excerpt {
        user.push_str(&format!("\nPage DOM excerpt:\n{excerpt}\n"));
    }

    let req = ChatRequest {
        model: model.unwrap_or("").to_string(),
        system: Some(system.into()),
        temperature: Some(0.0),
        max_tokens: Some(800),
        messages: vec![ChatMessage::text(Role::User, user)],
    };
    let t0 = Instant::now();
    let resp = router
        .chat(req)
        .await
        .map_err(|e| StepError::msg(format!("ai.heal_selector: {e}")))?;
    let usage = usage_of("heal_selector", &resp, t0.elapsed().as_millis() as i64);

    #[derive(Deserialize)]
    struct Out {
        #[serde(default)]
        css: Option<String>,
        #[serde(default)]
        xpath: Option<String>,
        #[serde(default)]
        confidence: f32,
        #[serde(default)]
        reasoning: String,
    }
    let out: Out = parse_json_loose(&resp.content).map_err(|e| {
        StepError::msg(format!(
            "ai.heal_selector parse: {e}; raw: {}",
            resp.content
        ))
    })?;
    Ok((
        HealedSelector {
            css: out.css.filter(|s| !s.is_empty()),
            xpath: out.xpath.filter(|s| !s.is_empty()),
            bbox: None,
            confidence: out.confidence.clamp(0.0, 1.0),
            reasoning: out.reasoning,
        },
        usage,
    ))
}

/// Insertion point ②. `target_description` is the prompt; `schema` (optional)
/// shapes the expected JSON. When `screenshot_png` is supplied the page image
/// is attached so the model can *see* the layout (true multimodal extraction);
/// otherwise it falls back to text-only using `page_text_excerpt`.
pub async fn extract_visual(
    router: &AiRouter,
    budget: &RunBudget,
    screenshot_png: Option<Bytes>,
    target_description: &str,
    page_text_excerpt: Option<&str>,
    schema: Option<&Value>,
    model: Option<&str>,
) -> Result<(Value, AiCallUsage), StepError> {
    budget
        .consume()
        .map_err(|_| StepError::BudgetExceeded { max: budget.max() })?;

    let has_image = screenshot_png.is_some();
    let system_base = if has_image {
        "You are an RPA extraction assistant. Look at the attached screenshot of \
         the page and return ONLY the extracted value as STRICT JSON (no Markdown fences)."
    } else {
        "You are an RPA extraction assistant. Given a target description and the page \
         contents, return ONLY the extracted value as STRICT JSON (no Markdown fences)."
    };
    let system = if let Some(s) = schema {
        format!("{system_base}\n\nReturn an object matching this JSON schema:\n{s}")
    } else {
        format!("{system_base}\n\nReturn a single JSON value (string|number|object|array).")
    };

    let mut user = format!("Target: {target_description}\n");
    if let Some(excerpt) = page_text_excerpt {
        user.push_str(&format!("\nPage text excerpt:\n{excerpt}\n"));
    }

    let mut msg = ChatMessage::text(Role::User, user);
    if let Some(png) = screenshot_png {
        guard_image_size("ai.extract_visual", png.len())?;
        let b64 = base64::engine::general_purpose::STANDARD.encode(&png);
        msg.attachments
            .push(ImageAttachment::base64("image/png", b64));
    }

    let req = ChatRequest {
        model: model.unwrap_or("").to_string(),
        system: Some(system),
        temperature: Some(0.0),
        max_tokens: Some(1500),
        messages: vec![msg],
    };
    let t0 = Instant::now();
    let resp = router
        .chat(req)
        .await
        .map_err(|e| StepError::msg(format!("ai.extract_visual: {e}")))?;
    let usage = usage_of("extract_visual", &resp, t0.elapsed().as_millis() as i64);
    let value: Value = parse_json_loose(&resp.content).map_err(|e| {
        StepError::msg(format!(
            "ai.extract_visual parse: {e}; raw: {}",
            resp.content
        ))
    })?;
    Ok((value, usage))
}

/// Insertion point ③. Returns `Decision { result, confidence, reasoning }`.
pub async fn decide(
    router: &AiRouter,
    budget: &RunBudget,
    vars_snapshot: &Value,
    prompt: &str,
    model: Option<&str>,
) -> Result<(Decision, AiCallUsage), StepError> {
    budget
        .consume()
        .map_err(|_| StepError::BudgetExceeded { max: budget.max() })?;

    let system =
        "You are an RPA branching assistant. Given context variables and a yes/no question, \
                  reply with STRICT JSON only (no Markdown fences): \
                  {\"result\": boolean, \"confidence\": number 0..1, \"reasoning\": string}.";
    let user = format!(
        "Context (JSON):\n{}\n\nQuestion: {prompt}",
        serde_json::to_string_pretty(vars_snapshot).unwrap_or_default()
    );

    let req = ChatRequest {
        model: model.unwrap_or("").to_string(),
        system: Some(system.into()),
        temperature: Some(0.0),
        max_tokens: Some(400),
        messages: vec![ChatMessage::text(Role::User, user)],
    };
    let t0 = Instant::now();
    let resp = router
        .chat(req)
        .await
        .map_err(|e| StepError::msg(format!("ai.decide: {e}")))?;
    let usage = usage_of("decide", &resp, t0.elapsed().as_millis() as i64);

    #[derive(Deserialize)]
    struct Out {
        result: bool,
        #[serde(default)]
        confidence: f32,
        #[serde(default)]
        reasoning: String,
    }
    let out: Out = parse_json_loose(&resp.content)
        .map_err(|e| StepError::msg(format!("ai.decide parse: {e}; raw: {}", resp.content)))?;
    Ok((
        Decision {
            result: out.result,
            confidence: out.confidence.clamp(0.0, 1.0),
            reasoning: out.reasoning,
        },
        usage,
    ))
}

/// Insertion point ⑤ (S-11/S-12). Vision-LLM grounding. The browser
/// resolver calls this when DOM-side strategies all fail; the caller passes
/// either an empty `marks` (asks for raw bbox in screenshot coords) or a
/// Set-of-Mark numbering (asks for a single index). Output is interpreted
/// by [`lumo_actions::vision`] which converts coordinates back into an
/// `Element` via `document.elementFromPoint`.
pub async fn vision_locate(
    router: &AiRouter,
    budget: &RunBudget,
    screenshot_png: Bytes,
    target_description: &str,
    marks: &[SoMMark],
    model: Option<&str>,
) -> Result<(LocatedTarget, AiCallUsage), StepError> {
    budget
        .consume()
        .map_err(|_| StepError::BudgetExceeded { max: budget.max() })?;

    guard_image_size("ai.vision_locate", screenshot_png.len())?;
    let b64 = base64::engine::general_purpose::STANDARD.encode(&screenshot_png);
    let attachment = ImageAttachment::base64("image/png", b64);

    let (system, user_text) = if marks.is_empty() {
        (
            "You are a Vision-LLM grounding assistant for RPA. Given a screenshot of a web page \
             and a natural-language target, return STRICT JSON only (no Markdown fences): \
             {\"bbox\": [x, y, w, h] | null, \"confidence\": number 0..1, \"reasoning\": string}. \
             Coordinates are CSS pixels in the screenshot. Return null when uncertain.".to_string(),
            format!("Target: {target_description}"),
        )
    } else {
        let mut listing = String::new();
        for m in marks {
            listing.push_str(&format!(
                "{} → {} {:?} {}\n",
                m.index,
                m.tag,
                (m.bbox.0 as i32, m.bbox.1 as i32, m.bbox.2 as i32, m.bbox.3 as i32),
                m.label
            ));
        }
        (
            "You are a Set-of-Mark grounding assistant. The screenshot has numbered overlays \
             on candidate elements. Pick the single number whose element best matches the \
             target. Return STRICT JSON only (no Markdown fences): \
             {\"mark\": integer | null, \"confidence\": number 0..1, \"reasoning\": string}. \
             Use `null` when none of the marks fits.".to_string(),
            format!("Target: {target_description}\n\nMarks (index → tag bbox label):\n{listing}"),
        )
    };

    let mut msg = ChatMessage::text(Role::User, user_text);
    msg.attachments.push(attachment);

    let req = ChatRequest {
        model: model.unwrap_or("").to_string(),
        system: Some(system),
        temperature: Some(0.0),
        max_tokens: Some(400),
        messages: vec![msg],
    };
    let t0 = Instant::now();
    let resp = router
        .chat(req)
        .await
        .map_err(|e| StepError::msg(format!("ai.vision_locate: {e}")))?;
    let usage = usage_of("vision_locate", &resp, t0.elapsed().as_millis() as i64);

    #[derive(Deserialize, Default)]
    struct Out {
        #[serde(default)]
        bbox: Option<Vec<f32>>,
        #[serde(default)]
        mark: Option<i64>,
        #[serde(default)]
        confidence: f32,
        #[serde(default)]
        reasoning: String,
    }
    // Vision models occasionally hedge with prose; on parse failure return an
    // empty `LocatedTarget` so the caller falls through cleanly rather than
    // exploding the run. Log the raw content first so the grounding path is
    // debuggable (truncated to keep logs sane).
    let out: Out = match parse_json_loose(&resp.content) {
        Ok(v) => v,
        Err(e) => {
            let raw: String = resp.content.chars().take(500).collect();
            tracing::warn!(
                error = %e,
                raw_truncated = %raw,
                "ai.vision_locate: failed to parse vision output; returning empty target"
            );
            Out::default()
        }
    };
    let bbox = out.bbox.and_then(|v| {
        if v.len() == 4 {
            Some((v[0], v[1], v[2], v[3]))
        } else {
            None
        }
    });
    let mark_index = out
        .mark
        .filter(|n| *n >= 0)
        .map(|n| n as u32)
        .filter(|n| marks.iter().any(|m| m.index == *n));
    Ok((
        LocatedTarget {
            bbox,
            mark_index,
            confidence: out.confidence.clamp(0.0, 1.0),
            reasoning: out.reasoning,
        },
        usage,
    ))
}

/// Insertion point ④. LLM diagnostic appended to a final-failure error message
/// when `metadata.ai.diagnose_on_failure: true`. Best-effort — never errors
/// the run further; caller should swallow errors.
pub async fn diagnose(
    router: &AiRouter,
    budget: &RunBudget,
    step_id: &str,
    action: &str,
    error: &str,
    model: Option<&str>,
) -> Result<(String, AiCallUsage), StepError> {
    budget
        .consume()
        .map_err(|_| StepError::BudgetExceeded { max: budget.max() })?;

    let system = "You are an RPA failure-diagnosis assistant. Given a step id, the action invoked, \
                  and the error message, return ONE concise sentence (≤ 30 words) explaining the most \
                  likely root cause and a single actionable suggestion. No Markdown, no JSON, just plain text.";
    let user = format!("step: {step_id}\naction: {action}\nerror: {error}");

    let req = ChatRequest {
        model: model.unwrap_or("").to_string(),
        system: Some(system.into()),
        temperature: Some(0.2),
        max_tokens: Some(200),
        messages: vec![ChatMessage::text(Role::User, user)],
    };
    let t0 = Instant::now();
    let resp = router
        .chat(req)
        .await
        .map_err(|e| StepError::msg(format!("ai.diagnose: {e}")))?;
    let usage = usage_of("diagnose", &resp, t0.elapsed().as_millis() as i64);
    Ok((resp.content.trim().to_string(), usage))
}

/// `AiHookProvider` impl wrapping a shared `AiRouter` + per-run `RunBudget`.
/// `FlowVm` holds a `Arc<dyn AiHookProvider>` (i.e. `Arc<AiHooks>`).
pub struct AiHooks {
    pub router: Arc<AiRouter>,
    pub budget: RunBudget,
    /// P1-4: per-call usage accumulated across hook dispatches. The VM drains
    /// it with [`AiHookProvider::take_usage`] after each call to write
    /// `ai_calls` ledger rows (it owns the run/step context the hooks lack).
    ///
    /// One run shares a single `AiHooks` (the VM holds an `Arc`), so under
    /// `control.parallel` concurrent branches drain the same buffer: run-level
    /// totals stay exact (no record is dropped or double-counted), but a
    /// record's `step_id` may be attributed to a sibling branch's step.
    usage: Mutex<Vec<AiCallUsage>>,
}

impl AiHooks {
    pub fn new(router: Arc<AiRouter>, budget: RunBudget) -> Self {
        Self {
            router,
            budget,
            usage: Mutex::new(Vec::new()),
        }
    }

    pub fn budget(&self) -> &RunBudget {
        &self.budget
    }

    /// Record one metered hook call into the accumulator.
    fn push_usage(&self, usage: AiCallUsage) {
        self.usage.lock().push(usage);
    }
}

/// P0-1: build the AI hook provider for a run from provider config + the flow's
/// AI policy. Returns `None` (hooks stay off) when the flow disabled AI
/// (`metadata.ai.enabled: false`) or no provider profiles are configured — so a
/// flow that opts into `ai.mode` without any configured backend doesn't spin up
/// doomed LLM calls. Runners attach the result via `FlowVm::with_ai_provider`.
///
/// This is the seam that was previously missing: `with_ai_provider` had zero
/// callers, so the whole hook subsystem was dead at runtime.
pub fn build_hook_provider(
    cfg: &crate::config::ProvidersConfig,
    flow_ai_enabled: bool,
    max_calls_per_run: u32,
) -> Option<Arc<dyn AiHookProvider>> {
    if !flow_ai_enabled || cfg.profiles.is_empty() {
        return None;
    }
    let router = Arc::new(AiRouter::from_config(cfg));
    let budget = RunBudget::new(max_calls_per_run);
    Some(Arc::new(AiHooks::new(router, budget)))
}

#[async_trait]
impl AiHookProvider for AiHooks {
    async fn heal_selector(
        &self,
        failed_selector: &str,
        prompt: &str,
        page_dom_excerpt: Option<&str>,
        model: Option<&str>,
    ) -> Result<HealedSelector, StepError> {
        let (healed, usage) = heal_selector(
            &self.router,
            &self.budget,
            None,
            failed_selector,
            prompt,
            page_dom_excerpt,
            model,
        )
        .await?;
        self.push_usage(usage);
        Ok(healed)
    }

    async fn extract_visual(
        &self,
        screenshot_png: Option<Bytes>,
        target_description: &str,
        page_text_excerpt: Option<&str>,
        schema: Option<&Value>,
        model: Option<&str>,
    ) -> Result<Value, StepError> {
        let (value, usage) = extract_visual(
            &self.router,
            &self.budget,
            screenshot_png,
            target_description,
            page_text_excerpt,
            schema,
            model,
        )
        .await?;
        self.push_usage(usage);
        Ok(value)
    }

    async fn decide(
        &self,
        vars_snapshot: &Value,
        prompt: &str,
        model: Option<&str>,
    ) -> Result<Decision, StepError> {
        let (decision, usage) =
            decide(&self.router, &self.budget, vars_snapshot, prompt, model).await?;
        self.push_usage(usage);
        Ok(decision)
    }

    async fn diagnose(
        &self,
        step_id: &str,
        action: &str,
        error: &str,
        model: Option<&str>,
    ) -> Result<String, StepError> {
        let (text, usage) =
            diagnose(&self.router, &self.budget, step_id, action, error, model).await?;
        self.push_usage(usage);
        Ok(text)
    }

    async fn vision_locate(
        &self,
        screenshot_png: Bytes,
        target_description: &str,
        marks: &[SoMMark],
        model: Option<&str>,
    ) -> Result<LocatedTarget, StepError> {
        let (target, usage) = vision_locate(
            &self.router,
            &self.budget,
            screenshot_png,
            target_description,
            marks,
            model,
        )
        .await?;
        self.push_usage(usage);
        Ok(target)
    }

    fn take_usage(&self) -> Vec<AiCallUsage> {
        std::mem::take(&mut *self.usage.lock())
    }
}

/// Strip optional Markdown fences and parse as JSON.
fn parse_json_loose<T: for<'de> Deserialize<'de>>(s: &str) -> Result<T, serde_json::Error> {
    let trimmed = s.trim();
    let cleaned = if let Some(rest) = trimmed.strip_prefix("```") {
        // ```json\n...\n```  OR  ```\n...\n```
        let after_fence = rest.find('\n').map(|i| &rest[i + 1..]).unwrap_or(rest);
        after_fence
            .strip_suffix("```")
            .unwrap_or(after_fence)
            .trim()
    } else {
        trimmed
    };
    serde_json::from_str(cleaned)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_plain_json() {
        let v: Value = parse_json_loose(r#"{"a":1}"#).unwrap();
        assert_eq!(v["a"], 1);
    }

    #[test]
    fn parses_fenced_json() {
        let v: Value = parse_json_loose("```json\n{\"a\":1}\n```").unwrap();
        assert_eq!(v["a"], 1);
    }

    #[test]
    fn parses_unlabelled_fenced_json() {
        let v: Value = parse_json_loose("```\n{\"a\":1}\n```").unwrap();
        assert_eq!(v["a"], 1);
    }
}
