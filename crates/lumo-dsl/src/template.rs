//! Template rendering using `minijinja` (Jinja2-compatible).
//!
//! A `TemplateCtx` is a JSON-shaped namespace; templates use dotted
//! identifiers like `{{ inputs.x }}`, `{{ steps.greet.result }}`,
//! `{{ env.HOME }}`, `{{ vault.smtp }}` (vault values are *placeholders*
//! at render time and JIT-resolved by the runtime).

use minijinja::{Environment, Value};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value as Json};
use std::sync::{Arc, OnceLock};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum TemplateError {
    #[error("template: {0}")]
    Render(#[from] minijinja::Error),
}

/// 模板渲染上下文(P1-3 性能税修复:命名空间全部 `Arc` 持有)。
///
/// 步骤输出全部累积在 `steps` 里(http 响应上限 100MiB),而 vm.rs 每个步骤
/// 至少构造/克隆本结构 2-3 次。字段 `Arc` 化后,克隆只是引用计数 +1(O(1)),
/// 真正的数据由调用方(`StepCtx`)以写时复制(`Arc::make_mut`)维护。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TemplateCtx {
    #[serde(default)]
    pub inputs: Arc<Json>,
    #[serde(default)]
    pub steps: Arc<Map<String, Json>>,
    #[serde(default)]
    pub vars: Arc<Map<String, Json>>,
    #[serde(default)]
    pub env: Arc<Json>,
    /// Vault placeholders (`{{ vault.smtp.user }}`) render to the literal
    /// string `${{ vault.smtp.user }}` so that secrets never appear in
    /// step input snapshots, logs, or LLM prompts. The runtime substitutes
    /// the real value just-in-time during action dispatch.
    #[serde(default)]
    pub vault: Vec<String>,
    /// Loop bindings injected by for/for_each.
    #[serde(default)]
    pub bindings: Arc<Map<String, Json>>,
    /// minijinja 渲染作用域缓存:同一个上下文快照(及其克隆,经 `Arc` 共享)
    /// 首次走完整字符串渲染时构建一次,之后复用 —— 把"每个字符串字段一次
    /// 全量 serialize"降为"每个状态代数一次"。字段保留 `pub` 仅为允许跨 crate
    /// 以字面量 + `..Default::default()` 构造,外部不应直接读写。
    #[doc(hidden)]
    #[serde(skip)]
    pub scope: Arc<OnceLock<Value>>,
}

impl TemplateCtx {
    /// 便捷构造:把一个 JSON 对象包成命名空间 map(非对象 ⇒ 空 map)。
    /// 供测试与调用方以 `serde_json::json!` 字面量填充 `steps`/`vars`/`bindings`。
    pub fn ns(v: Json) -> Arc<Map<String, Json>> {
        match v {
            Json::Object(m) => Arc::new(m),
            _ => Arc::new(Map::new()),
        }
    }

    /// minijinja 渲染作用域:loop 绑定平铺为顶层名字,保留命名空间
    /// (`inputs`/`steps`/`vars`/`env`)压顶、不可被绑定遮蔽(与旧实现
    /// "绑定先插、命名空间后插覆盖"语义等价)。整体转换只做一次并缓存;
    /// minijinja `Value` 克隆是廉价的引用计数操作。
    fn scope_value(&self) -> Value {
        self.scope
            .get_or_init(|| {
                let mut pairs: Vec<(String, Value)> = Vec::with_capacity(self.bindings.len() + 4);
                for (k, v) in self.bindings.iter() {
                    if matches!(k.as_str(), "inputs" | "steps" | "vars" | "env") {
                        continue; // 保留命名空间不可被 loop 绑定遮蔽
                    }
                    pairs.push((k.clone(), Value::from_serialize(v)));
                }
                pairs.push(("inputs".into(), Value::from_serialize(&*self.inputs)));
                pairs.push(("steps".into(), Value::from_serialize(&*self.steps)));
                pairs.push(("vars".into(), Value::from_serialize(&*self.vars)));
                pairs.push(("env".into(), Value::from_serialize(&*self.env)));
                Value::from_iter(pairs)
            })
            .clone()
    }
}

/// Render any string field. Non-string scalars / objects are returned as-is.
pub fn render(input: &Json, ctx: &TemplateCtx) -> Result<Json, TemplateError> {
    render_inner(global_env(), input, ctx)
}

/// 全局共享的 minijinja `Environment`:配置是纯静态的(无每上下文状态),
/// 进程内构建一次即可,省掉每次 `render` 的环境搭建开销。
fn global_env() -> &'static Environment<'static> {
    static ENV: OnceLock<Environment<'static>> = OnceLock::new();
    ENV.get_or_init(build_env)
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
///
/// P1-3:全程按引用下钻,只克隆最终命中的叶子值 —— 旧实现先把整个命名空间
/// (含累积的全部步骤输出)深拷贝出来再逐段 `remove`,是每次纯路径查找的
/// O(累积体积) 放大点。
pub(crate) fn lookup_path(ctx: &TemplateCtx, path: &[String]) -> Option<Json> {
    const JSON_NULL: Json = Json::Null;
    if path.is_empty() {
        return None;
    }
    let head = &path[0];
    // 解析根:inputs/env 是任意 JSON 值;steps/vars 是 map(整体引用时包一层
    // 对象,仅此一处发生整 map 克隆,实际模板里几乎不会写裸 `{{ steps }}`)。
    let (mut cur, rest): (&Json, &[String]) = match head.as_str() {
        "inputs" => (&ctx.inputs, &path[1..]),
        "env" => (&ctx.env, &path[1..]),
        "steps" | "vars" => {
            let map = if head == "steps" {
                &ctx.steps
            } else {
                &ctx.vars
            };
            match path.get(1) {
                None => return Some(Json::Object((**map).clone())),
                // 与旧语义一致:对象缺键视作 Null 继续下钻(尾段为空 ⇒ Some(Null),
                // 还有剩余段 ⇒ None,交还 minijinja 在 SemiStrict 下报未定义)。
                Some(k) => (map.get(k).unwrap_or(&JSON_NULL), &path[2..]),
            }
        }
        // Any other head may be a loop binding (row/item/index or a custom
        // `bind:` name). Resolve it from bindings; unknown heads fall through
        // to minijinja (which errors under SemiStrict for truly-undefined vars).
        other => (ctx.bindings.get(other)?, &path[1..]),
    };
    for seg in rest {
        cur = match cur {
            Json::Object(m) => m.get(seg).unwrap_or(&JSON_NULL),
            _ => return None,
        };
    }
    Some(cur.clone())
}

fn render_string(env: &Environment, src: &str, ctx: &TemplateCtx) -> Result<String, TemplateError> {
    // Replace vault placeholders BEFORE rendering so they survive untouched.
    // i.e. `{{ vault.smtp.user }}` -> literal `${{ vault.smtp.user }}` token.
    let pre = preprocess_vault(src, &ctx.vault);
    // 渲染作用域整体转换一次后缓存在上下文快照里(见 `TemplateCtx::scope_value`),
    // 同一代数内的后续字符串渲染直接复用,不再逐串克隆 + serialize。
    let scope = ctx.scope_value();
    let rendered = env.template_from_str(&pre)?.render(scope)?;
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
    // Only `{{ ... }}` expression blocks are handled. A `vault.` reference inside a
    // `{% ... %}` statement block or carrying whitespace-control markers (`{{- -}}`)
    // is left for minijinja; since `vault` is never in the render scope it errors
    // (fail-closed) rather than leaking — vault refs are only supported in `{{ }}`.
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
    fn vault_multiple_placeholders_in_one_string() {
        let ctx = TemplateCtx {
            vault: vec!["smtp".into()],
            ..Default::default()
        };
        let out = render(
            &Json::String("{{ vault.smtp.user }}:{{ vault.smtp.pass }}".into()),
            &ctx,
        )
        .unwrap();
        assert_eq!(
            out,
            Json::String("${{ vault.smtp.user }}:${{ vault.smtp.pass }}".into())
        );
    }

    #[test]
    fn non_vault_expression_with_vault_substring_is_not_literalized() {
        // A string literal that merely contains "vault." must be evaluated by
        // minijinja, NOT mistaken for a vault placeholder.
        let ctx = TemplateCtx {
            vault: vec!["smtp".into()],
            ..Default::default()
        };
        let out = render(&Json::String(r#"{{ "vault.x" }}"#.into()), &ctx).unwrap();
        assert_eq!(out, Json::String("vault.x".into()));
    }

    #[test]
    fn vault_reference_in_statement_block_fails_closed() {
        // Vault refs are only literalized inside `{{ }}`. A `vault.` ref in a
        // `{% %}` statement block is NOT literalized; `vault` is never in the
        // render scope, so it errors (fail-closed, no value leak).
        let ctx = TemplateCtx {
            vault: vec!["smtp".into()],
            ..Default::default()
        };
        let res = render(
            &Json::String("{% if vault.smtp.user %}x{% endif %}".into()),
            &ctx,
        );
        assert!(res.is_err(), "vault in a statement block must not evaluate");
    }

    #[test]
    fn vault_placeholder_with_multibyte_neighbors() {
        // Guards the byte-offset arithmetic against multibyte text around the block.
        let ctx = TemplateCtx {
            vault: vec!["smtp".into()],
            ..Default::default()
        };
        let out = render(&Json::String("密码={{ vault.smtp.pass }}".into()), &ctx).unwrap();
        assert_eq!(out, Json::String("密码=${{ vault.smtp.pass }}".into()));
    }

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
            inputs: Arc::new(serde_json::json!({ "who": "bob" })),
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
            bindings: TemplateCtx::ns(
                serde_json::json!({ "n": "hello", "row": "hello", "index": 2 }),
            ),
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
            bindings: TemplateCtx::ns(serde_json::json!({ "n": "solo" })),
            ..Default::default()
        };
        let out = render(&Json::String("{{ n }}".into()), &ctx).unwrap();
        assert_eq!(out, Json::String("solo".into()));
    }
}
