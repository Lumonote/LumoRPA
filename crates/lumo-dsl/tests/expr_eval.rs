//! B1 (F-14): boolean expression evaluation for `when` / `control.if`.
//!
//! Two modes, chosen from the RAW (pre-render) source:
//!   * Template mode — source has `{{ }}`/`{% %}`: render via minijinja, then
//!     apply truthiness (full back-compat with the old render+truthy path).
//!   * Expression mode — otherwise: parse a small boolean expression and
//!     evaluate it against the context.

use lumo_dsl::{eval_predicate, TemplateCtx};
use serde_json::json;
use std::sync::Arc;

fn ctx() -> TemplateCtx {
    TemplateCtx {
        inputs: Arc::new(json!({ "count": 5, "name": "Alice", "tags": ["a", "b"] })),
        steps: TemplateCtx::ns(json!({ "fetch": { "result": "done" } })),
        vars: TemplateCtx::ns(json!({ "flag": true, "ratio": 0 })),
        bindings: TemplateCtx::ns(json!({ "index": 3 })),
        ..Default::default()
    }
}

fn t(expr: &str) -> bool {
    eval_predicate(expr, &ctx()).expect("eval")
}

#[test]
fn literals() {
    assert!(t("true"));
    assert!(!t("false"));
    assert!(!t("null"));
    assert!(t("1"));
    assert!(!t("0"));
}

#[test]
fn comparisons_numeric() {
    assert!(t("3 > 2"));
    assert!(!t("2 > 3"));
    assert!(t("3 >= 3"));
    assert!(t("2 <= 3"));
    assert!(t("5 == 5"));
    assert!(t("5 != 6"));
    assert!(t("5 == 5.0")); // numeric coercion: int literal vs float literal
}

#[test]
fn comparisons_string() {
    assert!(t("'a' == 'a'"));
    assert!(t("'a' != 'b'"));
    assert!(t("'a' < 'b'")); // lexicographic
    assert!(t(r#""double" == "double""#));
}

#[test]
fn logical_ops() {
    assert!(t("true && true"));
    assert!(!t("true && false"));
    assert!(t("true || false"));
    assert!(!t("false || false"));
    assert!(t("!false"));
    assert!(!t("!true"));
    assert!(t("!(1 > 2)"));
}

#[test]
fn precedence() {
    assert!(t("1 < 2 && 3 < 4"));
    assert!(t("1 > 2 || 3 < 4"));
    assert!(!t("1 > 2 && 3 < 4"));
    assert!(t("(1 > 2 || 3 < 4) && 5 == 5"));
}

#[test]
fn identifier_paths_resolve() {
    assert!(t("inputs.count > 3"));
    assert!(t("inputs.count == 5"));
    assert!(!t("inputs.count < 3"));
    assert!(t("steps.fetch.result == 'done'"));
    assert!(t("vars.flag")); // bare truthy bool
    assert!(!t("vars.ratio")); // 0 → falsy
    assert!(t("index == 3")); // loop binding resolves as a bare name
}

#[test]
fn membership_in() {
    assert!(t("'a' in inputs.tags"));
    assert!(!t("'z' in inputs.tags"));
    assert!(t("'li' in 'Alice'")); // substring (string in string)
    assert!(t("'ce' in 'Alice'"));
}

#[test]
fn bare_unresolved_token_is_string_literal() {
    // A bare word that's not a namespace/binding is a string literal, so plain
    // truthy strings still behave (back-compat with the old is_truthy_str).
    assert!(t("yes"));
    assert!(!t("no"));
    assert!(!t("false"));
}

#[test]
fn template_mode_backcompat() {
    // Anything with `{{ }}` renders via minijinja first (full back-compat).
    assert!(t("{{ inputs.count }}")); // "5" → truthy
    assert!(t("{{ inputs.count > 3 }}")); // minijinja evaluates → true
    assert!(!t("{{ inputs.count > 9 }}")); // → false
    assert!(t("{{ vars.flag }}"));
}

#[test]
fn empty_is_false() {
    assert!(!t(""));
    assert!(!t("   "));
}

#[test]
fn malformed_falls_back_to_string_truthiness() {
    // An unparseable expression must not hard-error — it falls back to plain
    // string truthiness so a flow that worked before keeps working.
    assert!(t("a >")); // leftover non-empty string → truthy
}
