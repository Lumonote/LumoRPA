//! Multi-strategy element resolver for browser actions. The recorder captures
//! up to six fingerprints per element (id, data-testid, CSS path, aria-label,
//! visible text, XPath) — this is the runtime side that consumes them.
//!
//! On every resolution we try strategies in increasing "cost" order. Cost is
//! a stand-in for how brittle a strategy is in practice (ids are stable,
//! XPath is positional and breaks the most). The first match wins; the
//! winning strategy name is returned so the step output records which
//! fingerprint kept the flow alive.
//!
//! A-14 layered learning on top: outcomes are persisted in
//! [`selector_stats::SelectorStats`] under a canonical hash of the selector
//! spec. The next time the same hash is resolved, strategies that have
//! repeatedly failed are demoted (up to 3× their base cost) so the router
//! self-heals across runs — what the design doc spells out as the "Dijkstra
//! strategy graph".
//!
//! Vision-LLM / Set-of-Mark fallback (S-11/S-12) plug in later as additional
//! strategies; the contract here is "given fingerprints, pick a winner
//! deterministically, surface which one won, and remember".

use chromiumoxide::{Element, Page};
use lumo_core::error::StepError;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::time::Duration;

use crate::selector_stats::SelectorStats;

/// All strategies the recorder may emit. Every field is optional; in the
/// degenerate case the user supplies just one of them.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct MultiSelector {
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub data_testid: Option<String>,
    #[serde(default)]
    pub css: Option<String>,
    #[serde(default)]
    pub aria_label: Option<String>,
    #[serde(default)]
    pub text_includes: Option<String>,
    #[serde(default)]
    pub xpath: Option<String>,
}

/// Base cost table. Lower = preferred. Used as the floor — history-aware
/// ordering multiplies by [`SelectorStats::history_penalty`].
const BASE_COSTS: &[(&str, u32)] = &[
    ("id", 1),
    ("data_testid", 2),
    ("css", 4),
    ("aria_label", 5),
    ("text_includes", 6),
    ("xpath", 8),
];

fn base_cost(name: &str) -> f32 {
    BASE_COSTS
        .iter()
        .find(|(n, _)| *n == name)
        .map(|(_, c)| *c as f32)
        .unwrap_or(10.0)
}

impl MultiSelector {
    pub fn is_empty(&self) -> bool {
        self.id.is_none()
            && self.data_testid.is_none()
            && self.css.is_none()
            && self.aria_label.is_none()
            && self.text_includes.is_none()
            && self.xpath.is_none()
    }

    /// Build a `MultiSelector` from a single CSS string (back-compat for the
    /// existing `selector:` field in step.with).
    pub fn from_css(css: impl Into<String>) -> Self {
        Self {
            css: Some(css.into()),
            ..Default::default()
        }
    }

    /// Strategies in their static cost order (no learning applied). Useful
    /// for tests and human-readable hints.
    pub fn ordered(&self) -> Vec<(&'static str, &str)> {
        let mut out: Vec<(&'static str, &str)> = Vec::new();
        if let Some(v) = &self.id {
            out.push(("id", v));
        }
        if let Some(v) = &self.data_testid {
            out.push(("data_testid", v));
        }
        if let Some(v) = &self.css {
            out.push(("css", v));
        }
        if let Some(v) = &self.aria_label {
            out.push(("aria_label", v));
        }
        if let Some(v) = &self.text_includes {
            out.push(("text_includes", v));
        }
        if let Some(v) = &self.xpath {
            out.push(("xpath", v));
        }
        out
    }

    /// Strategies sorted by `base_cost × history_penalty`, so a strategy that
    /// failed three times at this fingerprint slides behind one with no
    /// history. Used by [`resolve_element`].
    ///
    /// A-14 upgrade: after picking the cheapest strategy, subsequent positions
    /// are chosen greedily with a transition-score discount — strategies that
    /// historically recovered from the previous one in `tried` order get
    /// promoted. Formally:
    ///   `score(s | prev) = base_cost(s) × history_penalty(s) / (1 + 5 × transition_score(prev → s))`
    /// The 5× multiplier is large enough that a single dominant recovery
    /// transition (score ≥ 0.8) can outweigh the next-cheapest base cost.
    pub fn ordered_for_runtime(&self, stats: &SelectorStats) -> Vec<(&'static str, &str)> {
        let hash = self.canonical_hash();
        let candidates: Vec<(&'static str, &str)> = self.ordered();
        if candidates.is_empty() {
            return candidates;
        }

        let base =
            |name: &'static str| -> f32 { base_cost(name) * stats.history_penalty(&hash, name) };

        let mut remaining: Vec<(&'static str, &str, f32)> = candidates
            .into_iter()
            .map(|(name, value)| (name, value, base(name)))
            .collect();

        let mut out: Vec<(&'static str, &str)> = Vec::with_capacity(remaining.len());
        while !remaining.is_empty() {
            let pick_idx = if let Some(prev) = out.last().map(|(n, _)| *n) {
                // Greedy with transition lookahead: divide cost by (1 + 2 × score).
                remaining
                    .iter()
                    .enumerate()
                    .min_by(|a, b| {
                        let sa = a.1 .2 / (1.0 + 5.0 * stats.transition_score(&hash, prev, a.1 .0));
                        let sb = b.1 .2 / (1.0 + 5.0 * stats.transition_score(&hash, prev, b.1 .0));
                        sa.partial_cmp(&sb)
                            .unwrap_or(std::cmp::Ordering::Equal)
                            .then_with(|| {
                                base_cost(a.1 .0)
                                    .partial_cmp(&base_cost(b.1 .0))
                                    .unwrap_or(std::cmp::Ordering::Equal)
                            })
                    })
                    .map(|(i, _)| i)
                    .unwrap_or(0)
            } else {
                // First position: cheapest base cost wins; ties broken by static order.
                remaining
                    .iter()
                    .enumerate()
                    .min_by(|a, b| {
                        a.1 .2
                            .partial_cmp(&b.1 .2)
                            .unwrap_or(std::cmp::Ordering::Equal)
                            .then_with(|| {
                                base_cost(a.1 .0)
                                    .partial_cmp(&base_cost(b.1 .0))
                                    .unwrap_or(std::cmp::Ordering::Equal)
                            })
                    })
                    .map(|(i, _)| i)
                    .unwrap_or(0)
            };
            let (name, value, _) = remaining.remove(pick_idx);
            out.push((name, value));
        }
        out
    }

    /// Best-effort label so error messages are useful when the whole router
    /// fails to find anything.
    pub fn first_hint(&self) -> String {
        self.ordered()
            .first()
            .map(|(name, val)| format!("{name}={val}"))
            .unwrap_or_else(|| "(empty selector spec)".into())
    }

    /// Stable hash of the selector spec. The same fingerprints in any
    /// declaration order produce the same hash so the stats store can group
    /// equivalent specs. Truncated to 16 hex chars to keep the JSON tidy.
    pub fn canonical_hash(&self) -> String {
        let canonical = serde_json::json!({
            "aria_label": self.aria_label.as_deref().unwrap_or(""),
            "css": self.css.as_deref().unwrap_or(""),
            "data_testid": self.data_testid.as_deref().unwrap_or(""),
            "id": self.id.as_deref().unwrap_or(""),
            "text_includes": self.text_includes.as_deref().unwrap_or(""),
            "xpath": self.xpath.as_deref().unwrap_or(""),
        });
        let bytes = serde_json::to_vec(&canonical).unwrap_or_default();
        let digest = Sha256::digest(&bytes);
        let hex = format!("{:x}", digest);
        hex[..16].to_string()
    }
}

/// JS shared between resolve attempts. The Rust side picks the priority order
/// (so history-aware sorting takes effect) and JS iterates that list.
/// `data-lumo-resolved="1"` is wiped at the start and set on the winner so
/// Rust can pull the element via CSS.
const RESOLVE_JS_TEMPLATE: &str = r#"
((spec) => {
  document.querySelectorAll('[data-lumo-resolved]').forEach((el) => el.removeAttribute('data-lumo-resolved'));
  const escape = (s) => (window.CSS && CSS.escape) ? CSS.escape(String(s)) : String(s).replace(/[^a-zA-Z0-9_-]/g, '\\$&');
  const handlers = {
    id: () => spec.id ? document.getElementById(spec.id) : null,
    data_testid: () => spec.data_testid ? document.querySelector(`[data-testid="${escape(spec.data_testid)}"]`) : null,
    css: () => spec.css ? document.querySelector(spec.css) : null,
    aria_label: () => {
      if (!spec.aria_label) return null;
      const exact = document.querySelector(`[aria-label="${escape(spec.aria_label)}"]`);
      if (exact) return exact;
      return Array.from(document.querySelectorAll('*')).find((el) => el.getAttribute && el.getAttribute('aria-label') === spec.aria_label) || null;
    },
    text_includes: () => {
      if (!spec.text_includes) return null;
      const needle = String(spec.text_includes).trim();
      if (!needle) return null;
      const candidates = document.querySelectorAll('button, a, span, label, div, li, td, th, h1, h2, h3, h4, h5, h6, p');
      for (const el of candidates) {
        const txt = (el.innerText || '').trim();
        if (txt && txt.includes(needle)) return el;
      }
      return null;
    },
    xpath: () => {
      if (!spec.xpath) return null;
      try {
        const r = document.evaluate(spec.xpath, document, null, XPathResult.FIRST_ORDERED_NODE_TYPE, null);
        return r.singleNodeValue;
      } catch (_) { return null; }
    },
  };
  const order = Array.isArray(spec._order) && spec._order.length
    ? spec._order
    : ['id', 'data_testid', 'css', 'aria_label', 'text_includes', 'xpath'];
  const tried = [];
  for (const name of order) {
    const fn = handlers[name];
    if (!fn || !spec[name]) continue;
    tried.push(name);
    try {
      const el = fn();
      if (el && el.nodeType === 1) {
        el.setAttribute('data-lumo-resolved', '1');
        return { ok: true, strategy: name, tried };
      }
    } catch (_) { /* try next */ }
  }
  return { ok: false, tried };
})(__SPEC__)
"#;

#[derive(Debug, Deserialize)]
struct ResolveOutcome {
    ok: bool,
    #[serde(default)]
    strategy: Option<String>,
    #[serde(default)]
    tried: Vec<String>,
}

const KNOWN_STRATEGIES: &[&str] = &[
    "id",
    "data_testid",
    "css",
    "aria_label",
    "text_includes",
    "xpath",
];

fn canonical_strategy(name: &str) -> &'static str {
    for known in KNOWN_STRATEGIES {
        if *known == name {
            return known;
        }
    }
    "unknown"
}

/// Resolve the multi-strategy selector against the current page. Returns the
/// matching `Element` plus the strategy name that found it. The outcome is
/// recorded into the global [`SelectorStats`] so the next resolve at the same
/// fingerprint hash benefits from the learned ordering.
pub async fn resolve_element(
    page: &Page,
    selector: &MultiSelector,
    timeout_ms: u64,
) -> Result<(Element, &'static str), StepError> {
    if selector.is_empty() {
        return Err(StepError::msg(
            "selector spec is empty: provide `selector:` or `selectors: {...}`",
        ));
    }
    let stats = SelectorStats::global();
    let hash = selector.canonical_hash();
    let ordered = selector.ordered_for_runtime(stats);
    let priority: Vec<&str> = ordered.iter().map(|(n, _)| *n).collect();

    let spec_json = serde_json::json!({
        "id": selector.id,
        "data_testid": selector.data_testid,
        "css": selector.css,
        "aria_label": selector.aria_label,
        "text_includes": selector.text_includes,
        "xpath": selector.xpath,
        "_order": priority,
    });
    let js = RESOLVE_JS_TEMPLATE.replace("__SPEC__", &spec_json.to_string());

    let result = tokio::time::timeout(Duration::from_millis(timeout_ms), page.evaluate(js))
        .await
        .map_err(|_| StepError::SelectorNotFound(selector.first_hint()))?
        .map_err(|e| StepError::msg(format!("resolve eval: {e}")))?;

    let outcome: ResolveOutcome = result
        .into_value()
        .map_err(|e| StepError::msg(format!("resolve decode: {e}")))?;

    if !outcome.ok {
        for name in &outcome.tried {
            stats.record(&hash, name, false);
        }
        return Err(StepError::SelectorNotFound(selector.first_hint()));
    }
    let strategy = canonical_strategy(outcome.strategy.as_deref().unwrap_or("unknown"));
    stats.record(&hash, strategy, true);
    // A-14: record the recovery transition. `tried` is ordered; everything
    // before the winner is a failure preceding the success, so the immediate
    // predecessor (if any) gets credit-by-association for what worked next.
    if let Some(last_failed) = outcome
        .tried
        .iter()
        .map(|s| canonical_strategy(s.as_str()))
        .rfind(|s| *s != strategy)
    {
        stats.record_transition(&hash, last_failed, strategy);
    }

    let element = page
        .find_element("[data-lumo-resolved='1']")
        .await
        .map_err(|_| StepError::SelectorNotFound(selector.first_hint()))?;

    Ok((element, strategy))
}

/// Best-effort cleanup of resolver markers. Call when the action is done so
/// later querySelector('[data-lumo-resolved]') from user-injected JS sees
/// nothing stale.
pub async fn clear_marker(page: &Page) {
    let _ = page
        .evaluate(
            "document.querySelectorAll('[data-lumo-resolved]').forEach((el) => el.removeAttribute('data-lumo-resolved'))",
        )
        .await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn fresh_stats(dir: &TempDir) -> SelectorStats {
        SelectorStats::at_path(dir.path().join("stats.json"))
    }

    #[test]
    fn ordered_respects_cost_table() {
        let s = MultiSelector {
            id: Some("a".into()),
            data_testid: Some("b".into()),
            css: Some(".c".into()),
            aria_label: Some("d".into()),
            text_includes: Some("e".into()),
            xpath: Some("//x".into()),
        };
        let names: Vec<_> = s.ordered().into_iter().map(|(n, _)| n).collect();
        assert_eq!(
            names,
            vec![
                "id",
                "data_testid",
                "css",
                "aria_label",
                "text_includes",
                "xpath"
            ]
        );
    }

    #[test]
    fn from_css_only_emits_css() {
        let s = MultiSelector::from_css("#login");
        assert_eq!(s.ordered().len(), 1);
        assert_eq!(s.ordered()[0], ("css", "#login"));
    }

    #[test]
    fn empty_is_empty() {
        assert!(MultiSelector::default().is_empty());
    }

    #[test]
    fn first_hint_picks_lowest_cost_strategy() {
        let s = MultiSelector {
            xpath: Some("//x".into()),
            css: Some(".btn".into()),
            ..Default::default()
        };
        assert_eq!(s.first_hint(), "css=.btn");
    }

    #[test]
    fn deserialize_from_yaml_like_value() {
        let v = serde_json::json!({
            "css": "#login",
            "xpath": "//button[1]",
            "aria_label": "登录"
        });
        let s: MultiSelector = serde_json::from_value(v).unwrap();
        assert_eq!(s.css.as_deref(), Some("#login"));
        assert_eq!(s.aria_label.as_deref(), Some("登录"));
        assert_eq!(s.xpath.as_deref(), Some("//button[1]"));
    }

    #[test]
    fn canonical_hash_is_stable_across_field_orders() {
        let a = MultiSelector {
            css: Some(".btn".into()),
            xpath: Some("//x".into()),
            ..Default::default()
        };
        let b = MultiSelector {
            xpath: Some("//x".into()),
            css: Some(".btn".into()),
            ..Default::default()
        };
        assert_eq!(a.canonical_hash(), b.canonical_hash());
    }

    #[test]
    fn canonical_hash_changes_with_content() {
        let a = MultiSelector::from_css(".btn");
        let b = MultiSelector::from_css(".btn2");
        assert_ne!(a.canonical_hash(), b.canonical_hash());
    }

    #[test]
    fn ordered_for_runtime_demotes_failing_strategy() {
        let dir = TempDir::new().unwrap();
        let stats = fresh_stats(&dir);
        let s = MultiSelector {
            css: Some(".btn".into()),
            xpath: Some("//x".into()),
            ..Default::default()
        };
        // No history: css comes first (base cost 4 < xpath 8).
        let order_before: Vec<_> = s
            .ordered_for_runtime(&stats)
            .into_iter()
            .map(|(n, _)| n)
            .collect();
        assert_eq!(order_before, vec!["css", "xpath"]);
        // Fail css 5 times at this hash; css now penalized 3×, cost becomes 12,
        // higher than xpath's 8 → xpath should bubble up.
        let h = s.canonical_hash();
        for _ in 0..5 {
            stats.record(&h, "css", false);
        }
        let order_after: Vec<_> = s
            .ordered_for_runtime(&stats)
            .into_iter()
            .map(|(n, _)| n)
            .collect();
        assert_eq!(order_after, vec!["xpath", "css"]);
    }

    #[test]
    fn ordered_for_runtime_keeps_clean_winners_first() {
        let dir = TempDir::new().unwrap();
        let stats = fresh_stats(&dir);
        let s = MultiSelector {
            id: Some("login".into()),
            css: Some("#fallback".into()),
            ..Default::default()
        };
        let h = s.canonical_hash();
        // id succeeded twice; css never tried.
        stats.record(&h, "id", true);
        stats.record(&h, "id", true);
        let order: Vec<_> = s
            .ordered_for_runtime(&stats)
            .into_iter()
            .map(|(n, _)| n)
            .collect();
        assert_eq!(order, vec!["id", "css"]);
    }

    #[test]
    fn ordered_for_runtime_promotes_proven_recovery_after_first() {
        // A-14: when id is the cheapest first pick but historically the
        // *recovery* from id is xpath (not css/data_testid), xpath should be
        // promoted to second position despite its higher base cost.
        let dir = TempDir::new().unwrap();
        let stats = fresh_stats(&dir);
        let s = MultiSelector {
            id: Some("login".into()),
            data_testid: Some("login-form".into()),
            css: Some(".btn".into()),
            xpath: Some("//x".into()),
            ..Default::default()
        };
        let h = s.canonical_hash();
        // Without transition history, the natural order is id, data_testid, css, xpath
        // (base costs 1, 2, 4, 8).
        let baseline: Vec<_> = s
            .ordered_for_runtime(&stats)
            .into_iter()
            .map(|(n, _)| n)
            .collect();
        assert_eq!(baseline, vec!["id", "data_testid", "css", "xpath"]);

        // Record 10 successful id → xpath recoveries; xpath becomes the
        // canonical second pick despite higher base cost.
        for _ in 0..10 {
            stats.record_transition(&h, "id", "xpath");
        }
        let after: Vec<_> = s
            .ordered_for_runtime(&stats)
            .into_iter()
            .map(|(n, _)| n)
            .collect();
        assert_eq!(after[0], "id", "id still cheapest first");
        assert_eq!(
            after[1], "xpath",
            "xpath promoted by proven id → xpath recovery"
        );
    }

    #[test]
    fn ordered_for_runtime_transition_does_not_override_first_pick() {
        // Transition score only kicks in for positions 2+. The first slot is
        // still cheapest-base-cost, even when an exotic recovery has lots of
        // observations.
        let dir = TempDir::new().unwrap();
        let stats = fresh_stats(&dir);
        let s = MultiSelector {
            id: Some("a".into()),
            xpath: Some("//x".into()),
            ..Default::default()
        };
        let h = s.canonical_hash();
        // Spurious self-recovery should not affect first slot ordering.
        for _ in 0..50 {
            stats.record_transition(&h, "css", "xpath");
        }
        let order: Vec<_> = s
            .ordered_for_runtime(&stats)
            .into_iter()
            .map(|(n, _)| n)
            .collect();
        assert_eq!(order, vec!["id", "xpath"]);
    }
}
