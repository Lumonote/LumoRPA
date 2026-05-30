//! Template rendering using `minijinja` (Jinja2-compatible).
//!
//! A `TemplateCtx` is a JSON-shaped namespace; templates use dotted
//! identifiers like `{{ inputs.x }}`, `{{ steps.greet.result }}`,
//! `{{ env.HOME }}`, `{{ vault.smtp }}` (vault values are *placeholders*
//! at render time and JIT-resolved by the runtime).

use minijinja::{Environment, Value};
use serde::{Deserialize, Serialize};
use serde_json::Value as Json;
use std::sync::Arc;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum TemplateError {
    #[error("template: {0}")]
    Render(#[from] minijinja::Error),
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TemplateCtx {
    #[serde(default)]
    pub inputs: Json,
    #[serde(default)]
    pub steps: Json,
    #[serde(default)]
    pub vars: Json,
    #[serde(default)]
    pub env: Json,
    /// Vault placeholders (`{{ vault.smtp.user }}`) render to the literal
    /// string `${{ vault.smtp.user }}` so that secrets never appear in
    /// step input snapshots, logs, or LLM prompts. The runtime substitutes
    /// the real value just-in-time during action dispatch.
    #[serde(default)]
    pub vault: Vec<String>,
    /// Loop bindings injected by for/for_each.
    #[serde(default)]
    pub bindings: Json,
}

/// Render any string field. Non-string scalars / objects are returned as-is.
pub fn render(input: &Json, ctx: &TemplateCtx) -> Result<Json, TemplateError> {
    let env = build_env();
    render_inner(&env, input, ctx)
}

fn build_env() -> Environment<'static> {
    let mut env = Environment::new();
    env.set_keep_trailing_newline(false);
    // P1-10: undefined variables (`{{ missing }}`) must raise a render error
    // instead of silently producing an empty string. SemiStrict (not Strict)
    // keeps `{{ x is defined }}` and `default(...)` guards usable.
    env.set_undefined_behavior(minijinja::UndefinedBehavior::SemiStrict);
    // Re-register `tojson` defensively so flows don't break across minijinja
    // versions / feature-flag matrices. `add_filter` overrides any builtin.
    env.add_filter(
        "tojson",
        |v: minijinja::Value| -> Result<String, minijinja::Error> {
            serde_json::to_string(&v).map_err(|e| {
                minijinja::Error::new(minijinja::ErrorKind::InvalidOperation, e.to_string())
            })
        },
    );
    env
}

fn render_inner(env: &Environment, input: &Json, ctx: &TemplateCtx) -> Result<Json, TemplateError> {
    match input {
        Json::String(s) if s.contains("{{") || s.contains("{%") => {
            // Fast-path 1: `{{ a.b.c }}` — pure variable lookup, no filters/ops.
            // Return the original JSON value without any string round-trip,
            // preserving arrays/objects/numbers exactly.
            if let Some(path) = pure_lookup_path(s) {
                if let Some(v) = lookup_path(ctx, &path) {
                    return Ok(v);
                }
            }
            let rendered = render_string(env, s, ctx)?;
            // Fast-path 2: short interpolation that round-trips to a scalar
            // (e.g. `{{ inputs.n }}` evaluating to `42`). Filtered/piped
            // expressions intentionally do NOT get re-parsed, so
            // `{{ x | tojson }}` keeps its string form.
            if is_single_expression(s) && !s.contains('|') {
                if let Ok(json) = serde_json::from_str::<Json>(&rendered) {
                    if matches!(json, Json::Bool(_) | Json::Number(_) | Json::Null) {
                        return Ok(json);
                    }
                }
            } else if let Ok(json) = serde_json::from_str::<Json>(&rendered) {
                if matches!(json, Json::Bool(_) | Json::Number(_) | Json::Null) {
                    return Ok(json);
                }
            }
            Ok(Json::String(rendered))
        }
        Json::Array(arr) => Ok(Json::Array(
            arr.iter()
                .map(|v| render_inner(env, v, ctx))
                .collect::<Result<_, _>>()?,
        )),
        Json::Object(map) => {
            let mut out = serde_json::Map::with_capacity(map.len());
            for (k, v) in map {
                out.insert(k.clone(), render_inner(env, v, ctx)?);
            }
            Ok(Json::Object(out))
        }
        other => Ok(other.clone()),
    }
}

/// True iff `s` is exactly one `{{ ... }}` block with no surrounding text.
fn is_single_expression(s: &str) -> bool {
    let t = s.trim();
    if !(t.starts_with("{{") && t.ends_with("}}")) {
        return false;
    }
    let inner = &t[2..t.len() - 2];
    if inner.contains("{{") || inner.contains("}}") {
        return false;
    }
    if t.contains("{%") {
        return false;
    }
    true
}

/// If `s` is exactly `{{ <ident.path> }}` (pure dotted lookup, no filters,
/// no operators), return the path components.
fn pure_lookup_path(s: &str) -> Option<Vec<String>> {
    if !is_single_expression(s) {
        return None;
    }
    let t = s.trim();
    let inner = t[2..t.len() - 2].trim();
    if inner.is_empty() {
        return None;
    }
    if inner
        .chars()
        .any(|c| !(c.is_alphanumeric() || c == '.' || c == '_'))
    {
        return None;
    }
    Some(inner.split('.').map(|p| p.to_string()).collect())
}

/// Walk `path` against the template context's namespaces. The first segment
/// is one of the reserved roots (`inputs`/`steps`/`vars`/`env`) or any loop
/// binding name (`row`/`item`/`index` or a custom `bind:` like `n`).
fn lookup_path(ctx: &TemplateCtx, path: &[String]) -> Option<Json> {
    if path.is_empty() {
        return None;
    }
    let head = &path[0];
    let root: Json = match head.as_str() {
        "inputs" => ctx.inputs.clone(),
        "steps" => ctx.steps.clone(),
        "vars" => ctx.vars.clone(),
        "env" => ctx.env.clone(),
        // Any other head may be a loop binding (row/item/index or a custom
        // `bind:` name). Resolve it from bindings; unknown heads fall through
        // to minijinja (which errors under SemiStrict for truly-undefined vars).
        other => ctx.bindings.get(other).cloned()?,
    };
    let mut cur = root;
    for seg in &path[1..] {
        cur = match cur {
            Json::Object(mut m) => m.remove(seg).unwrap_or(Json::Null),
            _ => return None,
        };
    }
    Some(cur)
}

fn render_string(env: &Environment, src: &str, ctx: &TemplateCtx) -> Result<String, TemplateError> {
    // Replace vault placeholders BEFORE rendering so they survive untouched.
    // i.e. `{{ vault.smtp.user }}` -> literal `${{ vault.smtp.user }}` token.
    let pre = preprocess_vault(src, &ctx.vault);
    // Build the render scope: every loop binding (row/item/index AND any custom
    // `bind:` name) is exposed as a top-level name, then the reserved
    // namespaces are layered on last so they can never be shadowed by a bind.
    let mut root = serde_json::Map::new();
    if let Json::Object(binds) = &ctx.bindings {
        for (k, v) in binds {
            root.insert(k.clone(), v.clone());
        }
    }
    root.insert("inputs".into(), ctx.inputs.clone());
    root.insert("steps".into(), ctx.steps.clone());
    root.insert("vars".into(), ctx.vars.clone());
    root.insert("env".into(), ctx.env.clone());
    let tmpl_ctx = Value::from_serialize(Json::Object(root));
    let rendered = Arc::new(env).template_from_str(&pre)?.render(tmpl_ctx)?;
    Ok(rendered)
}

fn preprocess_vault(src: &str, vault_names: &[String]) -> String {
    if vault_names.is_empty() || !src.contains("vault.") {
        return src.to_string();
    }
    // Keep secret references OUT of the live template engine. Each
    // `{{ vault.PATH }}` (or `{{vault.PATH}}`) is rewritten to a raw-wrapped
    // literal `{% raw %}${{ vault.PATH }}{% endraw %}`, which minijinja renders
    // verbatim to the token `${{ vault.PATH }}`. The runtime
    // (`StepCtx::resolve_vault_placeholders`) substitutes the real value
    // just-in-time at action dispatch, so secrets never enter rendered step
    // snapshots, logs, or LLM prompts. Non-vault expressions are left untouched
    // for minijinja to evaluate. A naive `{{`→`${{` replace does NOT work: the
    // expression stays live and errors (vault is not in the render scope).
    let mut out = String::with_capacity(src.len() + 24);
    let mut rest = src;
    while let Some(open) = rest.find("{{") {
        out.push_str(&rest[..open]);
        let after = &rest[open + 2..];
        let Some(close) = after.find("}}") else {
            // Unterminated `{{` — emit verbatim and stop; minijinja will report it.
            out.push_str(&rest[open..]);
            return out;
        };
        let inner = after[..close].trim();
        if let Some(path) = inner.strip_prefix("vault.") {
            out.push_str("{% raw %}${{ vault.");
            out.push_str(path);
            out.push_str(" }}{% endraw %}");
        } else {
            // Non-vault expression block — pass through unchanged.
            out.push_str(&rest[open..open + 2 + close + 2]);
        }
        rest = &after[close + 2..];
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vault_placeholder_survives_render_as_literal_token() {
        // P1-3: `{{ vault.X }}` must render to the literal `${{ vault.X }}`
        // (NOT be evaluated), so the runtime can JIT-resolve it later.
        let ctx = TemplateCtx {
            vault: vec!["smtp".into()],
            ..Default::default()
        };
        let out = render(&Json::String("{{ vault.smtp.user }}".into()), &ctx).unwrap();
        assert_eq!(out, Json::String("${{ vault.smtp.user }}".into()));
    }

    #[test]
    fn vault_placeholder_mixed_with_live_expression() {
        let ctx = TemplateCtx {
            inputs: serde_json::json!({ "who": "bob" }),
            vault: vec!["smtp".into()],
            ..Default::default()
        };
        let out = render(
            &Json::String("hi {{ inputs.who }} pass={{ vault.smtp.pass }}".into()),
            &ctx,
        )
        .unwrap();
        assert_eq!(
            out,
            Json::String("hi bob pass=${{ vault.smtp.pass }}".into())
        );
    }

    #[test]
    fn vault_placeholder_no_spaces_normalizes() {
        let ctx = TemplateCtx {
            vault: vec!["smtp".into()],
            ..Default::default()
        };
        let out = render(&Json::String("{{vault.smtp.user}}".into()), &ctx).unwrap();
        assert_eq!(out, Json::String("${{ vault.smtp.user }}".into()));
    }

    #[test]
    fn scalar_vault_placeholder_survives() {
        let ctx = TemplateCtx {
            vault: vec!["token".into()],
            ..Default::default()
        };
        let out = render(&Json::String("{{ vault.token }}".into()), &ctx).unwrap();
        assert_eq!(out, Json::String("${{ vault.token }}".into()));
    }

    #[test]
    fn custom_loop_binding_resolves_as_bare_name() {
        // P1-10 regression: a custom for_each `bind:` name (here `n`) must
        // resolve as a bare `{{ n }}`. Before, render only surfaced the
        // hard-coded row/item/index binds, so `{{ n }}` silently rendered ""
        // (and errors outright under SemiStrict). row/index must still work.
        let ctx = TemplateCtx {
            bindings: serde_json::json!({ "n": "hello", "row": "hello", "index": 2 }),
            ..Default::default()
        };
        let out = render(
            &Json::String("n={{ n }} row={{ row }} i={{ index }}".into()),
            &ctx,
        )
        .unwrap();
        assert_eq!(out, Json::String("n=hello row=hello i=2".into()));
    }

    #[test]
    fn bare_custom_binding_single_expr_resolves() {
        // The pure-lookup fast path must also resolve a custom bind name.
        let ctx = TemplateCtx {
            bindings: serde_json::json!({ "n": "solo" }),
            ..Default::default()
        };
        let out = render(&Json::String("{{ n }}".into()), &ctx).unwrap();
        assert_eq!(out, Json::String("solo".into()));
    }
}
