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
/// must be one of: `inputs`, `steps`, `vars`, `env`, `row`, `item`, `index`.
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
        "row" => ctx.bindings.get("row").cloned().unwrap_or(Json::Null),
        "item" => ctx.bindings.get("item").cloned().unwrap_or(Json::Null),
        "index" => ctx.bindings.get("index").cloned().unwrap_or(Json::Null),
        _ => return None,
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
    let tmpl_ctx = Value::from_serialize(serde_json::json!({
        "inputs": ctx.inputs,
        "steps":  ctx.steps,
        "vars":   ctx.vars,
        "env":    ctx.env,
        // Loop bindings: merge as top-level for convenience: `{{ row.x }}`
        "row":    ctx.bindings.get("row").cloned().unwrap_or(Json::Null),
        "item":   ctx.bindings.get("item").cloned().unwrap_or(Json::Null),
        "index":  ctx.bindings.get("index").cloned().unwrap_or(Json::Null),
    }));
    let rendered = Arc::new(env).template_from_str(&pre)?.render(tmpl_ctx)?;
    Ok(rendered)
}

fn preprocess_vault(src: &str, vault_names: &[String]) -> String {
    if vault_names.is_empty() || !src.contains("vault.") {
        return src.to_string();
    }
    // Crude but safe: `{{ vault.X.* }}` -> `${{ vault.X.* }}`
    // Real implementation in M2 will use a proper Jinja AST walk.
    src.replace("{{ vault.", "${{ vault.")
        .replace("{{vault.", "${{vault.")
}
