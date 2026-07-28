use crate::ast::{Flow, Step};
use std::collections::HashSet;
use thiserror::Error;

/// P1-2:`spec.resources.<name>.kind` 的合法取值白名单。
///
/// lumo-dsl 不能依赖 lumo-actions(层次反转),所以这里放一份保守的静态副本,
/// 与各 action crate 的 KIND 常量保持同步:
/// - `browser.rs::BROWSER_KIND` = "chromium.cdp"
/// - `db_ops.rs::{SQLITE_KIND, PG_KIND, MYSQL_KIND}` = "sqlite" / "postgres" / "mysql"
/// - `http.rs::HTTP_KIND` = "http"
/// - `email.rs::SMTP_KIND` = "smtp"
/// - `transfer.rs::FTP_KIND` = "ftp"
///
/// lumo-actions 侧有同步测试(`lib.rs` 的 `resource_kind_sync` 模块)断言两边
/// 一致——新增资源 kind 时该测试会失败,提醒同时更新这份白名单。
pub const KNOWN_RESOURCE_KINDS: &[&str] = &[
    "chromium.cdp",
    "sqlite",
    "postgres",
    "mysql",
    "http",
    "smtp",
    "imap",
    "ftp",
    "xlsx",
];

/// P1-6:`retry.on` 的合法错误 kind 名(snake_case)。
///
/// 是 lumo-core `ErrorKind::as_str()` 的静态快照(lumo-dsl 不能依赖 lumo-core,
/// 否则循环依赖)。lumo-core 侧有同步测试(`error.rs` 的
/// `retry_on_kinds_match_dsl_whitelist`)断言两边一致。
/// 注意:`timeout` 仅在 `on` 里**显式**列出时才可重试——空 `on` 的"任意错误都
/// 重试"不含步级超时,VM 仍按硬中断处理(P0-2)。
pub const RETRY_ON_KINDS: &[&str] = &[
    "selector_not_found",
    "extract_failed",
    "cond_error",
    "capability_denied",
    "budget_exceeded",
    "io",
    "network",
    "timeout",
    "other",
];

/// P1-6:`retry.backoff` 的合法取值。运行期 `compute_backoff` 只识别
/// `exponential`(其余一律按 fixed),这里把值域收紧成显式白名单,
/// 拼错在 validate 期报错而非运行期静默降级为 fixed。
/// lumo-core 侧有同步测试(`vm.rs` 的 `backoff_whitelist_matches_compute_backoff`)。
pub const RETRY_BACKOFF_KINDS: &[&str] = &["fixed", "exponential"];

#[derive(Debug, Error)]
pub enum ValidationError {
    #[error("flow `{flow}` step id collision: `{id}`")]
    DuplicateStepId { flow: String, id: String },

    #[error("flow `{flow}` step `{id}` action is empty")]
    EmptyAction { flow: String, id: String },

    #[error(
        "flow `{flow}` step `{id}` has `do` but action `{action}` is not a control-flow action"
    )]
    StrayBlock {
        flow: String,
        id: String,
        action: String,
    },

    #[error("flow `{flow}` step `{id}` missing required `do` block for action `{action}`")]
    MissingDoBlock {
        flow: String,
        id: String,
        action: String,
    },

    #[error("flow `{flow}` step `{id}` enables AI but `spec.capabilities.llm` is empty")]
    AiMissingLlmCapability { flow: String, id: String },

    #[error(
        "flow `{flow}` step `{id}` references undeclared resource `{resource}` (not in spec.resources)"
    )]
    UnknownResource {
        flow: String,
        id: String,
        resource: String,
    },

    #[error("flow `{flow}` resource `{name}` has unknown kind `{kind}` (known kinds: {known})")]
    UnknownResourceKind {
        flow: String,
        name: String,
        kind: String,
        known: String,
    },

    #[error(
        "flow `{flow}` step `{id}` retry.on has unknown error kind `{value}` (known kinds: {known}; note: `timeout` retries step timeouts only when listed explicitly)"
    )]
    UnknownRetryOnKind {
        flow: String,
        id: String,
        value: String,
        known: String,
    },

    #[error("flow `{flow}` step `{id}` retry.backoff `{value}` is invalid (allowed: {known})")]
    UnknownRetryBackoff {
        flow: String,
        id: String,
        value: String,
        known: String,
    },

    #[error(
        "flow `{flow}` step `{id}` uses `{action}` outside of a loop (it must be nested in the `do:` of control.while / control.for / control.for_each; control.parallel branches are separate scopes)"
    )]
    BreakOutsideLoop {
        flow: String,
        id: String,
        action: String,
    },
}

/// A small set of action ids that may carry `do/else/catch/finally` children.
/// Kept in DSL to avoid a circular dep with `lumo-actions`.
pub fn is_control_action(id: &str) -> bool {
    matches!(
        id,
        "control.if"
            | "control.for"
            | "control.for_each"
            | "control.while"
            | "control.try"
            | "control.parallel"
            | "excel.for_each_row"
            | "browser.for_each"
    )
}

pub fn validate(flow: &Flow) -> Result<(), ValidationError> {
    let mut seen = HashSet::new();
    let flow_ai_enabled = flow.metadata.ai.as_ref().map(|a| a.enabled).unwrap_or(true);
    let has_llm_cap = !flow.spec.capabilities.llm.is_empty();
    let resources: HashSet<&str> = flow.spec.resources.keys().map(String::as_str).collect();
    // P1-2:资源 kind 白名单校验。拼错的 kind(如 `sqllite`)以前能过 validate,
    // 运行期各 action 对不认识的 kind 一律 `_ => None` 静默回退(两个拼错 kind 的
    // 浏览器资源甚至会静默共用 default slot 同一实例)——必须在 validate 期拦下。
    for (name, decl) in &flow.spec.resources {
        if !KNOWN_RESOURCE_KINDS.contains(&decl.kind.as_str()) {
            return Err(ValidationError::UnknownResourceKind {
                flow: flow.metadata.id.clone(),
                name: name.clone(),
                kind: decl.kind.clone(),
                known: KNOWN_RESOURCE_KINDS.join(", "),
            });
        }
    }
    walk(
        &flow.metadata.id,
        &flow.spec.steps,
        &mut seen,
        flow_ai_enabled,
        has_llm_cap,
        &resources,
        false,
    )?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn walk(
    flow: &str,
    steps: &[Step],
    seen: &mut HashSet<String>,
    flow_ai_enabled: bool,
    has_llm_cap: bool,
    resources: &HashSet<&str>,
    in_loop: bool,
) -> Result<(), ValidationError> {
    for s in steps {
        if !seen.insert(s.id.clone()) {
            return Err(ValidationError::DuplicateStepId {
                flow: flow.into(),
                id: s.id.clone(),
            });
        }
        if s.action.trim().is_empty() {
            return Err(ValidationError::EmptyAction {
                flow: flow.into(),
                id: s.id.clone(),
            });
        }
        let has_block = s.do_.is_some()
            || s.else_.is_some()
            || s.catch_.is_some()
            || s.finally_.is_some()
            || s.branches.is_some();
        if has_block && !is_control_action(&s.action) {
            return Err(ValidationError::StrayBlock {
                flow: flow.into(),
                id: s.id.clone(),
                action: s.action.clone(),
            });
        }
        if s.branches.is_some() && s.action != "control.parallel" {
            return Err(ValidationError::StrayBlock {
                flow: flow.into(),
                id: s.id.clone(),
                action: s.action.clone(),
            });
        }
        // control.parallel accepts either `branches:` (multi-step branches) or
        // `do:` (each top-level step becomes a one-step branch).
        let parallel_has_body = s.do_.is_some() || s.branches.is_some();
        if matches!(
            s.action.as_str(),
            "control.for"
                | "control.for_each"
                | "control.while"
                | "control.try"
                | "excel.for_each_row"
                | "browser.for_each"
        ) && s.do_.is_none()
        {
            return Err(ValidationError::MissingDoBlock {
                flow: flow.into(),
                id: s.id.clone(),
                action: s.action.clone(),
            });
        }
        if s.action == "control.parallel" && !parallel_has_body {
            return Err(ValidationError::MissingDoBlock {
                flow: flow.into(),
                id: s.id.clone(),
                action: s.action.clone(),
            });
        }
        // AI enablement requires LLM capability when the flow-level master is on.
        if flow_ai_enabled {
            if let Some(ai) = &s.ai {
                if ai.is_enabled() && !has_llm_cap {
                    return Err(ValidationError::AiMissingLlmCapability {
                        flow: flow.into(),
                        id: s.id.clone(),
                    });
                }
            }
        }
        // T3: a per-step resource reference must resolve to a declared
        // `spec.resources` entry — a typo'd handle could never be opened.
        if let Some(res) = &s.resource {
            if !resources.contains(res.as_str()) {
                return Err(ValidationError::UnknownResource {
                    flow: flow.into(),
                    id: s.id.clone(),
                    resource: res.clone(),
                });
            }
        }
        // P1-6:retry 值域校验。运行期 `retry_matches` 对未知 kind 名静默不匹配
        // (拼错 = 静默不重试)、`compute_backoff` 对未知 backoff 一律按 fixed
        // (拼错 = 静默降级)——必须在 validate 期报错并给出合法值列表。
        if let Some(retry) = &s.retry {
            for value in &retry.on {
                // 与运行期 `retry_matches` 同口径:trim + 忽略大小写。
                let normalized = value.trim().to_ascii_lowercase();
                if !RETRY_ON_KINDS.contains(&normalized.as_str()) {
                    return Err(ValidationError::UnknownRetryOnKind {
                        flow: flow.into(),
                        id: s.id.clone(),
                        value: value.clone(),
                        known: RETRY_ON_KINDS.join(", "),
                    });
                }
            }
            // backoff 用严格相等:运行期 `compute_backoff` 只精确匹配
            // "exponential",写 "Exponential" 也会静默按 fixed,所以这里不放宽。
            if !RETRY_BACKOFF_KINDS.contains(&retry.backoff.as_str()) {
                return Err(ValidationError::UnknownRetryBackoff {
                    flow: flow.into(),
                    id: s.id.clone(),
                    value: retry.backoff.clone(),
                    known: RETRY_BACKOFF_KINDS.join(", "),
                });
            }
        }
        // 指令集 P1:break / continue 的循环祖先链静态检查——必须出现在某个
        // 循环容器 `do:` 块的祖先链内,否则运行期信号会逃逸出循环作用域。
        if matches!(s.action.as_str(), "control.break" | "control.continue") && !in_loop {
            return Err(ValidationError::BreakOutsideLoop {
                flow: flow.into(),
                id: s.id.clone(),
                action: s.action.clone(),
            });
        }
        // 只有运行期真正消化 break / continue 信号的循环容器才算循环祖先:
        // control.while / control.for / control.for_each。excel.for_each_row /
        // browser.for_each 虽带 do 块,但不经 VM 的循环 runner、不消化信号,
        // 保守起见不算(信号会穿透它们继续向上找真正的循环容器)。
        let is_loop_container = matches!(
            s.action.as_str(),
            "control.while" | "control.for" | "control.for_each"
        );
        // control.parallel 的 do / branches 都是新作用域:每个分支独立执行,
        // break / continue 不跨分支,祖先链在分支边界清零。
        let is_parallel = s.action == "control.parallel";
        let child_in_loop = if is_parallel {
            false
        } else {
            in_loop || is_loop_container
        };
        if let Some(d) = &s.do_ {
            walk(
                flow,
                d,
                seen,
                flow_ai_enabled,
                has_llm_cap,
                resources,
                child_in_loop,
            )?;
        }
        // if 的 else、try 的 catch / finally 与所在循环同作用域,继承祖先链。
        for block in [&s.else_, &s.catch_, &s.finally_].into_iter().flatten() {
            walk(
                flow,
                block,
                seen,
                flow_ai_enabled,
                has_llm_cap,
                resources,
                in_loop,
            )?;
        }
        if let Some(bs) = &s.branches {
            for b in bs {
                walk(
                    flow,
                    b,
                    seen,
                    flow_ai_enabled,
                    has_llm_cap,
                    resources,
                    false,
                )?;
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse_str;

    #[test]
    fn unknown_resource_ref_is_rejected() {
        let y = r#"
apiVersion: lumorpa.io/v1
kind: Flow
metadata: { id: t, version: 0.1.0 }
spec:
  steps:
    - id: q
      action: db.query
      resource: ghost
"#;
        let flow = parse_str(y).expect("parses (parse and validate are independent)");
        let err = validate(&flow).expect_err("undeclared resource ref must be rejected");
        assert!(
            matches!(err, ValidationError::UnknownResource { .. }),
            "got: {err}"
        );
    }

    #[test]
    fn declared_resource_ref_is_accepted() {
        let y = r#"
apiVersion: lumorpa.io/v1
kind: Flow
metadata: { id: t, version: 0.1.0 }
spec:
  resources:
    db:
      kind: sqlite
      url: "/tmp/x.db"
  steps:
    - id: q
      action: db.query
      resource: db
"#;
        let flow = parse_str(y).expect("parses");
        assert_eq!(flow.spec.resources.len(), 1);
        assert_eq!(flow.spec.resources["db"].kind, "sqlite");
        validate(&flow).expect("a declared resource ref must validate");
    }

    #[test]
    fn resource_ref_in_nested_block_is_validated() {
        // The check must recurse into control-flow children, not just top level.
        let y = r#"
apiVersion: lumorpa.io/v1
kind: Flow
metadata: { id: t, version: 0.1.0 }
spec:
  steps:
    - id: loop
      action: control.for_each
      with: { items: [1, 2] }
      do:
        - id: inner
          action: db.query
          resource: ghost
"#;
        let flow = parse_str(y).expect("parses");
        match validate(&flow).expect_err("nested undeclared resource ref must be rejected") {
            ValidationError::UnknownResource { resource, id, .. } => {
                assert_eq!(resource, "ghost");
                assert_eq!(id, "inner", "the failing step should be the nested one");
            }
            other => panic!("expected UnknownResource, got: {other}"),
        }
    }

    #[test]
    fn misspelled_resource_kind_is_rejected_with_known_list() {
        // P1-2:`sqllite`(拼错)以前能过 validate,运行期才静默回退。
        let y = r#"
apiVersion: lumorpa.io/v1
kind: Flow
metadata: { id: t, version: 0.1.0 }
spec:
  resources:
    db:
      kind: sqllite
      path: "/tmp/x.db"
  steps:
    - id: a
      action: control.log
      with: { message: x }
"#;
        let flow = parse_str(y).expect("parses (parse and validate are independent)");
        let err = validate(&flow).expect_err("misspelled resource kind must be rejected");
        assert!(
            matches!(err, ValidationError::UnknownResourceKind { .. }),
            "got: {err}"
        );
        let msg = err.to_string();
        assert!(msg.contains("sqllite"), "message names the bad kind: {msg}");
        // 错误信息必须给出合法值列表。
        assert!(
            msg.contains("sqlite") && msg.contains("chromium.cdp"),
            "message lists known kinds: {msg}"
        );
    }

    #[test]
    fn all_known_resource_kinds_are_accepted() {
        for kind in KNOWN_RESOURCE_KINDS {
            let y = format!(
                r#"
apiVersion: lumorpa.io/v1
kind: Flow
metadata: {{ id: t, version: 0.1.0 }}
spec:
  resources:
    r:
      kind: {kind}
  steps:
    - id: a
      action: control.log
      with: {{ message: x }}
"#
            );
            let flow = parse_str(&y).expect("parses");
            validate(&flow).unwrap_or_else(|e| panic!("kind `{kind}` must validate, got: {e}"));
        }
    }

    #[test]
    fn imap_and_xlsx_resource_kinds_are_accepted() {
        for kind in ["imap", "xlsx"] {
            let y = format!(
                r#"
apiVersion: lumorpa.io/v1
kind: Flow
metadata: {{ id: t, version: 0.1.0 }}
spec:
  resources:
    r:
      kind: {kind}
  steps:
    - id: a
      action: control.log
      with: {{ message: x }}
"#
            );
            let flow = parse_str(&y).unwrap();
            validate(&flow).unwrap_or_else(|e| panic!("resource kind `{kind}`: {e}"));
        }
    }

    #[test]
    fn misspelled_retry_on_kind_is_rejected_with_known_list() {
        // P1-6:`selector_notfound`(拼错)以前能过 validate,运行期静默不重试。
        let y = r#"
apiVersion: lumorpa.io/v1
kind: Flow
metadata: { id: t, version: 0.1.0 }
spec:
  steps:
    - id: a
      action: control.log
      with: { message: x }
      retry: { times: 2, on: [selector_notfound] }
"#;
        let flow = parse_str(y).expect("parses");
        let err = validate(&flow).expect_err("misspelled retry.on kind must be rejected");
        assert!(
            matches!(err, ValidationError::UnknownRetryOnKind { .. }),
            "got: {err}"
        );
        let msg = err.to_string();
        assert!(
            msg.contains("selector_notfound"),
            "names the bad value: {msg}"
        );
        assert!(
            msg.contains("selector_not_found") && msg.contains("other"),
            "lists known kinds: {msg}"
        );
    }

    #[test]
    fn retry_on_timeout_is_accepted() {
        // P0-2:`on: [timeout]` 是显式声明"超时也重试"的唯一写法(空 on 不含
        // 超时),validate 必须放行 —— 钉住白名单含 `timeout`,防止回退到
        // f6ec517 时代的"超时不可重试"拦截。
        let y = r#"
apiVersion: lumorpa.io/v1
kind: Flow
metadata: { id: t, version: 0.1.0 }
spec:
  steps:
    - id: a
      action: control.log
      with: { message: x }
      retry: { times: 2, on: [timeout] }
"#;
        let flow = parse_str(y).expect("parses");
        validate(&flow).expect("`on: [timeout]` must validate (P0-2 retryable timeouts)");
    }

    #[test]
    fn retry_on_io_and_network_are_accepted() {
        for kind in ["io", "network"] {
            let y = format!(
                r#"
apiVersion: lumorpa.io/v1
kind: Flow
metadata: {{ id: t, version: 0.1.0 }}
spec:
  steps:
    - id: a
      action: control.log
      with: {{ message: x }}
      retry: {{ times: 2, on: [{kind}] }}
"#
            );
            let flow = parse_str(&y).expect("parses");
            validate(&flow).unwrap_or_else(|e| panic!("`on: [{kind}]` must validate, got: {e}"));
        }
    }

    #[test]
    fn retry_on_matching_is_case_and_whitespace_insensitive_like_runtime() {
        // 与运行期 `retry_matches` 同口径:" Other " 合法。
        let y = r#"
apiVersion: lumorpa.io/v1
kind: Flow
metadata: { id: t, version: 0.1.0 }
spec:
  steps:
    - id: a
      action: control.log
      with: { message: x }
      retry: { times: 2, on: [" Other "] }
"#;
        let flow = parse_str(y).expect("parses");
        validate(&flow).expect("runtime would match ` Other ` so validate must accept it");
    }

    #[test]
    fn misspelled_backoff_is_rejected_with_known_list() {
        // P1-6:`expo`(拼错)以前能过 validate,运行期静默按 fixed。
        let y = r#"
apiVersion: lumorpa.io/v1
kind: Flow
metadata: { id: t, version: 0.1.0 }
spec:
  steps:
    - id: a
      action: control.log
      with: { message: x }
      retry: { times: 2, backoff: expo }
"#;
        let flow = parse_str(y).expect("parses");
        let err = validate(&flow).expect_err("misspelled backoff must be rejected");
        assert!(
            matches!(err, ValidationError::UnknownRetryBackoff { .. }),
            "got: {err}"
        );
        let msg = err.to_string();
        assert!(
            msg.contains("fixed") && msg.contains("exponential"),
            "lists allowed values: {msg}"
        );
    }

    #[test]
    fn capitalized_backoff_is_rejected_because_runtime_matches_exactly() {
        // 运行期 compute_backoff 精确匹配 "exponential","Exponential" 会静默按
        // fixed,所以 validate 必须按严格相等拒绝。
        let y = r#"
apiVersion: lumorpa.io/v1
kind: Flow
metadata: { id: t, version: 0.1.0 }
spec:
  steps:
    - id: a
      action: control.log
      with: { message: x }
      retry: { times: 2, backoff: Exponential }
"#;
        let flow = parse_str(y).expect("parses");
        validate(&flow).expect_err("`Exponential` would silently degrade to fixed at runtime");
    }

    #[test]
    fn valid_retry_spec_is_accepted() {
        let y = r#"
apiVersion: lumorpa.io/v1
kind: Flow
metadata: { id: t, version: 0.1.0 }
spec:
  steps:
    - id: a
      action: control.log
      with: { message: x }
      retry: { times: 2, backoff: exponential, initial_ms: 10, on: [selector_not_found, other] }
"#;
        let flow = parse_str(y).expect("parses");
        validate(&flow).expect("a fully valid retry spec must validate");
    }

    #[test]
    fn retry_in_nested_block_is_validated() {
        // retry 校验必须递归进控制流子块,不能只看顶层。
        let y = r#"
apiVersion: lumorpa.io/v1
kind: Flow
metadata: { id: t, version: 0.1.0 }
spec:
  steps:
    - id: loop
      action: control.for_each
      with: { items: [1] }
      do:
        - id: inner
          action: control.log
          with: { message: x }
          retry: { times: 1, backoff: expo }
"#;
        let flow = parse_str(y).expect("parses");
        match validate(&flow).expect_err("nested bad backoff must be rejected") {
            ValidationError::UnknownRetryBackoff { id, .. } => assert_eq!(id, "inner"),
            other => panic!("expected UnknownRetryBackoff, got: {other}"),
        }
    }

    // ─── 指令集 P1:break / continue 循环祖先链 ─────────────────────────────

    #[test]
    fn break_outside_loop_is_rejected() {
        let y = r#"
apiVersion: lumorpa.io/v1
kind: Flow
metadata: { id: t, version: 0.1.0 }
spec:
  steps:
    - id: out
      action: control.break
"#;
        let flow = parse_str(y).expect("parses");
        let err = validate(&flow).expect_err("top-level break must be rejected");
        assert!(
            matches!(err, ValidationError::BreakOutsideLoop { .. }),
            "got: {err}"
        );
        assert!(err.to_string().contains("control.break"), "got: {err}");
    }

    #[test]
    fn continue_outside_loop_is_rejected() {
        // if 的 do 块不是循环作用域:祖先链里没有循环容器仍然要拦。
        let y = r#"
apiVersion: lumorpa.io/v1
kind: Flow
metadata: { id: t, version: 0.1.0 }
spec:
  steps:
    - id: gate
      action: control.if
      with: { cond: "true" }
      do:
        - id: skip
          action: control.continue
"#;
        let flow = parse_str(y).expect("parses");
        let err = validate(&flow).expect_err("continue without a loop ancestor must be rejected");
        match err {
            ValidationError::BreakOutsideLoop { id, action, .. } => {
                assert_eq!(id, "skip");
                assert_eq!(action, "control.continue");
            }
            other => panic!("expected BreakOutsideLoop, got: {other}"),
        }
    }

    #[test]
    fn break_inside_while_do_is_accepted() {
        let y = r#"
apiVersion: lumorpa.io/v1
kind: Flow
metadata: { id: t, version: 0.1.0 }
spec:
  steps:
    - id: loop
      action: control.while
      with: { cond: "vars.go" }
      do:
        - id: out
          action: control.break
"#;
        let flow = parse_str(y).expect("parses");
        validate(&flow).expect("break inside a while body must validate");
    }

    #[test]
    fn break_inside_try_catch_within_loop_is_accepted() {
        // try 的 do / catch 与所在循环同作用域,祖先链要继承。
        let y = r#"
apiVersion: lumorpa.io/v1
kind: Flow
metadata: { id: t, version: 0.1.0 }
spec:
  steps:
    - id: loop
      action: control.for
      with: { to: 3 }
      do:
        - id: guard
          action: control.try
          with: {}
          do:
            - { id: noop, action: control.log, with: { message: x } }
          catch:
            - id: out
              action: control.break
"#;
        let flow = parse_str(y).expect("parses");
        validate(&flow).expect("break in a catch block inside a loop must validate");
    }

    #[test]
    fn break_in_parallel_branch_without_inner_loop_is_rejected() {
        // parallel 分支是新作用域:即使 parallel 本身嵌在 for 里,分支内没有
        // 自己的循环祖先就必须拦下。
        let y = r#"
apiVersion: lumorpa.io/v1
kind: Flow
metadata: { id: t, version: 0.1.0 }
spec:
  steps:
    - id: outer
      action: control.for
      with: { to: 2 }
      do:
        - id: par
          action: control.parallel
          branches:
            - - id: out
                action: control.break
"#;
        let flow = parse_str(y).expect("parses");
        let err = validate(&flow)
            .expect_err("break in a parallel branch without an in-branch loop must be rejected");
        match err {
            ValidationError::BreakOutsideLoop { id, .. } => assert_eq!(id, "out"),
            other => panic!("expected BreakOutsideLoop, got: {other}"),
        }
    }

    #[test]
    fn break_inside_loop_within_parallel_branch_is_accepted() {
        // 分支内自带循环容器 → break 在分支内被消化,合法。
        let y = r#"
apiVersion: lumorpa.io/v1
kind: Flow
metadata: { id: t, version: 0.1.0 }
spec:
  steps:
    - id: par
      action: control.parallel
      branches:
        - - id: loop
            action: control.for_each
            with: { in: [1, 2] }
            do:
              - id: out
                action: control.break
"#;
        let flow = parse_str(y).expect("parses");
        validate(&flow).expect("break inside a loop within a parallel branch must validate");
    }

    #[test]
    fn while_requires_do_block() {
        let y = r#"
apiVersion: lumorpa.io/v1
kind: Flow
metadata: { id: t, version: 0.1.0 }
spec:
  steps:
    - id: loop
      action: control.while
      with: { cond: "true" }
"#;
        let flow = parse_str(y).expect("parses");
        let err = validate(&flow).expect_err("while without do: must be rejected");
        assert!(
            matches!(err, ValidationError::MissingDoBlock { .. }),
            "got: {err}"
        );
    }

    #[test]
    fn resource_decl_flattens_kind_specific_config() {
        let y = r#"
apiVersion: lumorpa.io/v1
kind: Flow
metadata: { id: t, version: 0.1.0 }
spec:
  resources:
    browser:
      kind: chromium.cdp
      profile: stealth-default
      headless: true
  steps:
    - id: a
      action: control.log
      with: { message: x }
"#;
        let flow = parse_str(y).expect("parses");
        let r = &flow.spec.resources["browser"];
        assert_eq!(r.kind, "chromium.cdp");
        assert_eq!(r.profile.as_deref(), Some("stealth-default"));
        // kind-specific `headless` flattens into `config`, not a hard struct field.
        assert_eq!(
            r.config.get("headless").and_then(|v| v.as_bool()),
            Some(true)
        );
    }
}
