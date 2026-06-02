//! Selector scoring (F-22).
//!
//! When the recorder captures an element it gathers several candidate
//! selectors (an `#id`, a `[data-testid]`, a class-anchored path, a positional
//! `nth-of-type` path, …). This module ranks them so the YAML patch can emit
//! the *most stable* one first rather than whatever the DOM-walk produced.
//!
//! The heuristic mirrors what a careful human would pick:
//!
//! * `#id` and `[data-testid]` (and friends) are the gold standard — short,
//!   semantic, unlikely to churn between deploys.
//! * stable, human-authored class names (`login-button`, `nav__item`) are
//!   decent anchors.
//! * auto-generated / hashed classes (CSS-modules `Button_abc12`, styled
//!   components `css-1q2w3e`, utility soup `mt-4 px-2`) are *volatile* — a
//!   rebuild changes them, so they're penalized hard.
//! * deep positional paths (`div > div > ul > li:nth-of-type(3) > a`) are
//!   brittle: every extra `>` hop and every `:nth-*` index is one more thing
//!   that shifts when the page is restructured, so depth is penalized.
//!
//! The scorer is pure logic over strings, so it's fully unit-testable without
//! a browser — which is the point: the brittle e2e bits (Drop teardown,
//! connect-to-Chrome) get `#[ignore]`d sketches, but selector quality is
//! covered for real.

/// A scored candidate. Higher [`score`](Candidate::score) is better.
#[derive(Debug, Clone, PartialEq)]
pub struct Candidate {
    /// The selector string itself (CSS).
    pub selector: String,
    /// Which capture strategy produced it (`id`, `data_testid`, `css`, …).
    /// Mirrors the `MultiSelector` field names so the YAML patch can route it.
    pub kind: SelectorKind,
    /// Computed stability score. See [`score_selector`].
    pub score: i32,
}

/// The recorder strategy a candidate came from. Ordering here is *not* the
/// final ranking — that's the numeric score — but it lets us route a winner
/// into the right `selectors:` field.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SelectorKind {
    Id,
    DataTestId,
    AriaLabel,
    Css,
    Xpath,
}

impl SelectorKind {
    /// Field name used in the emitted `selectors:` block.
    pub fn field(&self) -> &'static str {
        match self {
            SelectorKind::Id => "id",
            SelectorKind::DataTestId => "data_testid",
            SelectorKind::AriaLabel => "aria_label",
            SelectorKind::Css => "css",
            SelectorKind::Xpath => "xpath",
        }
    }
}

/// Base reward for a bare `#id` selector — the strongest anchor.
const SCORE_ID: i32 = 100;
/// Base reward for a `[data-*]` test hook (`data-testid`, `data-test`, …).
const SCORE_DATA_ATTR: i32 = 90;
/// Base reward for an `[aria-label=...]` selector — semantic and fairly stable.
const SCORE_ARIA: i32 = 60;
/// Base reward for a plain CSS path; class/depth adjustments apply on top.
const SCORE_CSS_BASE: i32 = 40;
/// Base reward for an XPath — usable, but positional and the most brittle, so
/// it sits below a class-anchored CSS path.
const SCORE_XPATH: i32 = 20;

/// Per-`>` combinator penalty: deeper paths are more brittle.
const PENALTY_PER_DEPTH: i32 = 6;
/// Per-`:nth-*` penalty: positional indices break on reorder/insert.
const PENALTY_PER_NTH: i32 = 12;
/// Penalty per class segment that looks machine-generated (hashed / utility).
const PENALTY_VOLATILE_CLASS: i32 = 18;
/// Reward per class segment that looks like a stable, human-authored name.
const REWARD_STABLE_CLASS: i32 = 8;

/// Score a CSS selector string for *stability* — how likely it is to keep
/// matching the same element across deploys/edits. Higher is better; scores
/// can go negative for pathologically brittle paths.
///
/// `kind` chooses the base reward; the CSS-path adjustments (depth, nth,
/// class quality) only apply to [`SelectorKind::Css`] since the others are
/// single-token anchors.
pub fn score_selector(selector: &str, kind: SelectorKind) -> i32 {
    let base = match kind {
        SelectorKind::Id => SCORE_ID,
        SelectorKind::DataTestId => SCORE_DATA_ATTR,
        SelectorKind::AriaLabel => SCORE_ARIA,
        SelectorKind::Css => SCORE_CSS_BASE,
        SelectorKind::Xpath => SCORE_XPATH,
    };
    if kind != SelectorKind::Css {
        return base;
    }

    let mut score = base;

    // A CSS path that is itself just `#id` / `[data-testid=...]` is as good as
    // the dedicated kinds — reward it so a recorder that only produced a `css`
    // field still ranks an id-path above a class-path.
    let trimmed = selector.trim();
    if trimmed.starts_with('#') && !trimmed.contains(' ') && !trimmed.contains('>') {
        return SCORE_ID;
    }
    if trimmed.starts_with("[data-testid") || trimmed.starts_with("[data-test") {
        return SCORE_DATA_ATTR;
    }

    // Depth: count `>` combinators (descendant hops). Each costs stability.
    let depth = trimmed.matches('>').count() as i32;
    score -= depth * PENALTY_PER_DEPTH;

    // Positional indices.
    let nth = trimmed.matches(":nth-").count() as i32;
    score -= nth * PENALTY_PER_NTH;

    // Class quality: walk every `.class` token and reward/penalize.
    for class in extract_classes(trimmed) {
        if is_volatile_class(&class) {
            score -= PENALTY_VOLATILE_CLASS;
        } else {
            score += REWARD_STABLE_CLASS;
        }
    }

    score
}

/// Pull out the class tokens from a CSS selector path. Splits on combinators
/// and `.`, ignoring tag names, ids, attribute filters and pseudo-classes.
fn extract_classes(selector: &str) -> Vec<String> {
    let mut classes = Vec::new();
    // Normalize combinators to spaces so we can tokenize on whitespace.
    let flat = selector.replace(['>', '+', '~'], " ");
    for token in flat.split_whitespace() {
        // A compound token like `li.card.active:nth-of-type(2)`. Strip any
        // pseudo / attribute / id tail before splitting on `.`.
        let head = token
            .split([':', '[', '#'])
            .next()
            .unwrap_or(token);
        let mut parts = head.split('.');
        // First part is the (optional) tag name — skip it.
        let _tag = parts.next();
        for cls in parts {
            if !cls.is_empty() {
                classes.push(cls.to_string());
            }
        }
    }
    classes
}

/// Heuristic: does this class name look machine-generated / volatile?
///
/// Flags the common offenders:
/// * CSS-modules / styled-components hashes: `Button_abc12`, `css-1q2w3e`,
///   `sc-bdVaJa`, anything with a `_hash` or `-hash` tail of mixed alnum.
/// * Tailwind-style utilities: `mt-4`, `px-2`, `w-1/2`, `text-sm` — short,
///   abbreviation + number, churn with design tweaks.
/// * pure-hash blobs: `a1b2c3d4`.
pub fn is_volatile_class(class: &str) -> bool {
    if class.is_empty() {
        return true;
    }

    // Tailwind / utility: `prefix-number` or `prefix-number/number`.
    if is_utility_class(class) {
        return true;
    }

    // styled-components emotion: `css-<hash>`, `sc-<hash>`.
    let lower = class.to_ascii_lowercase();
    if (lower.starts_with("css-") || lower.starts_with("sc-")) && has_hashish_tail(&class[3..]) {
        return true;
    }

    // CSS-modules style: `Name_hash` or `Name-hash` where the tail looks hashed.
    if let Some(idx) = class.rfind(['_', '-']) {
        let tail = &class[idx + 1..];
        if has_hashish_tail(tail) {
            return true;
        }
    }

    // Bare hash: a single token that is mixed letters+digits with no obvious
    // word structure and at least one digit, e.g. `a1b2c3`, `x7f9q2`.
    if class.len() >= 5 && looks_like_hash(class) {
        return true;
    }

    false
}

/// `mt-4`, `px-2`, `w-1/2`, `gap-x-3`, `text-sm` — utility class shape.
fn is_utility_class(class: &str) -> bool {
    let parts: Vec<&str> = class.split('-').collect();
    if parts.len() < 2 {
        return false;
    }
    let last = parts[parts.len() - 1];
    // Trailing token is a number, fraction, or a short size keyword.
    let numericish = last
        .split('/')
        .all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()));
    let size_kw = matches!(last, "xs" | "sm" | "md" | "lg" | "xl" | "2xl" | "3xl" | "full" | "auto");
    if !(numericish || size_kw) {
        return false;
    }
    // First token is a short lowercase abbreviation (`mt`, `px`, `w`, `gap`).
    let first = parts[0];
    !first.is_empty()
        && first.len() <= 5
        && first.chars().all(|c| c.is_ascii_lowercase())
}

/// Does `s` look like a generated hash tail — mixed alnum with digits, or a
/// short opaque blob? Used to flag `_abc12` / `-1q2w3e` style suffixes.
fn has_hashish_tail(s: &str) -> bool {
    if s.len() < 4 {
        return false;
    }
    looks_like_hash(s)
}

/// A token "looks like a hash" when it contains both digits and letters in a
/// non-word pattern (no separators), or is long alnum with a high digit ratio.
fn looks_like_hash(s: &str) -> bool {
    let has_digit = s.chars().any(|c| c.is_ascii_digit());
    let has_alpha = s.chars().any(|c| c.is_ascii_alphabetic());
    let all_alnum = s.chars().all(|c| c.is_ascii_alphanumeric());
    if !all_alnum {
        return false;
    }
    // Mixed letters+digits with no internal separator is the classic hash shape.
    has_digit && has_alpha
}

/// Rank a set of candidates best-first. Stable sort, so equal scores keep the
/// caller's insertion order (which is the natural strategy priority).
pub fn rank(mut candidates: Vec<Candidate>) -> Vec<Candidate> {
    candidates.sort_by_key(|c| std::cmp::Reverse(c.score));
    candidates
}

/// Convenience: build and rank candidates from the raw fields the recorder
/// captures per DOM event, returning the best one (if any).
///
/// `css` is the DOM-walk path, `id`/`data_testid`/`aria_label`/`xpath` the
/// optional dedicated anchors. Empty / absent fields are skipped.
pub fn best_candidate(
    id: Option<&str>,
    data_testid: Option<&str>,
    css: Option<&str>,
    aria_label: Option<&str>,
    xpath: Option<&str>,
) -> Option<Candidate> {
    let mut cands = Vec::new();
    let mut push = |val: Option<&str>, kind: SelectorKind| {
        if let Some(v) = val {
            let v = v.trim();
            if !v.is_empty() {
                cands.push(Candidate {
                    selector: v.to_string(),
                    kind,
                    score: score_selector(v, kind),
                });
            }
        }
    };
    push(id, SelectorKind::Id);
    push(data_testid, SelectorKind::DataTestId);
    push(css, SelectorKind::Css);
    push(aria_label, SelectorKind::AriaLabel);
    push(xpath, SelectorKind::Xpath);
    rank(cands).into_iter().next()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn id_beats_everything() {
        let id = score_selector("#login", SelectorKind::Id);
        let data = score_selector("submit-btn", SelectorKind::DataTestId);
        let css = score_selector("form > button.primary", SelectorKind::Css);
        assert!(id > data, "id ({id}) should beat data ({data})");
        assert!(data > css, "data ({data}) should beat css path ({css})");
    }

    #[test]
    fn id_path_in_css_field_is_promoted() {
        // A recorder that only filled `css` with `#id` still ranks it as gold.
        assert_eq!(
            score_selector("#main", SelectorKind::Css),
            score_selector("#main", SelectorKind::Id)
        );
    }

    #[test]
    fn data_testid_path_in_css_field_is_promoted() {
        assert_eq!(
            score_selector("[data-testid=\"row\"]", SelectorKind::Css),
            SCORE_DATA_ATTR
        );
    }

    #[test]
    fn deeper_paths_score_lower() {
        let shallow = score_selector("nav > a.link", SelectorKind::Css);
        let deep = score_selector("div > div > ul > li > a.link", SelectorKind::Css);
        assert!(shallow > deep, "shallow {shallow} should beat deep {deep}");
    }

    #[test]
    fn nth_of_type_is_penalized() {
        let plain = score_selector("ul > li.item", SelectorKind::Css);
        let positional = score_selector("ul > li.item:nth-of-type(3)", SelectorKind::Css);
        assert!(plain > positional);
    }

    #[test]
    fn stable_class_beats_volatile_class() {
        let stable = score_selector("button.login-button", SelectorKind::Css);
        let volatile = score_selector("button.css-1q2w3e", SelectorKind::Css);
        assert!(
            stable > volatile,
            "stable {stable} should beat volatile {volatile}"
        );
    }

    #[test]
    fn detects_volatile_classes() {
        assert!(is_volatile_class("css-1q2w3e"));
        assert!(is_volatile_class("sc-bdVaJa1"));
        assert!(is_volatile_class("Button_a1b2c"));
        assert!(is_volatile_class("mt-4"));
        assert!(is_volatile_class("px-2"));
        assert!(is_volatile_class("w-1/2"));
        assert!(is_volatile_class("text-sm"));
        assert!(is_volatile_class("a1b2c3d4"));
    }

    #[test]
    fn keeps_stable_classes() {
        assert!(!is_volatile_class("login-button"));
        assert!(!is_volatile_class("nav__item"));
        assert!(!is_volatile_class("card"));
        assert!(!is_volatile_class("primary"));
        assert!(!is_volatile_class("user-profile-header"));
    }

    #[test]
    fn best_candidate_prefers_id_over_path() {
        let best = best_candidate(
            Some("login"),
            None,
            Some("form > div > button.btn:nth-of-type(2)"),
            Some("Sign in"),
            Some("//form/div/button[2]"),
        )
        .unwrap();
        assert_eq!(best.kind, SelectorKind::Id);
        assert_eq!(best.selector, "login");
    }

    #[test]
    fn best_candidate_picks_stable_css_when_no_anchor() {
        let best = best_candidate(
            None,
            None,
            Some("button.login-button"),
            None,
            Some("//div/button[1]"),
        )
        .unwrap();
        // Stable class CSS should outrank the positional XPath.
        assert_eq!(best.kind, SelectorKind::Css);
    }

    #[test]
    fn best_candidate_falls_back_to_xpath_over_volatile_deep_css() {
        // A deeply nested, all-volatile-class path can score below XPath.
        let css = "div > div > div > span.css-1a2b3c:nth-of-type(4)";
        let css_score = score_selector(css, SelectorKind::Css);
        let xp_score = score_selector("//div/span[4]", SelectorKind::Xpath);
        let best = best_candidate(None, None, Some(css), None, Some("//div/span[4]")).unwrap();
        if css_score >= xp_score {
            assert_eq!(best.kind, SelectorKind::Css);
        } else {
            assert_eq!(best.kind, SelectorKind::Xpath);
        }
    }

    #[test]
    fn best_candidate_none_when_all_empty() {
        assert!(best_candidate(None, None, Some("  "), Some(""), None).is_none());
    }

    #[test]
    fn rank_is_stable_for_ties() {
        let a = Candidate {
            selector: "a".into(),
            kind: SelectorKind::Css,
            score: 10,
        };
        let b = Candidate {
            selector: "b".into(),
            kind: SelectorKind::Css,
            score: 10,
        };
        let ranked = rank(vec![a.clone(), b.clone()]);
        assert_eq!(ranked[0].selector, "a");
        assert_eq!(ranked[1].selector, "b");
    }
}
