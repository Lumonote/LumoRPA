//! Flow VM — durable, step-based executor.
//!
//! Step execution semantics:
//!   * Inputs are template-rendered first.
//!   * Control-flow actions (`control.if`, `control.for`, `control.for_each`,
//!     `control.try`, `control.parallel`) are dispatched inline by the VM
//!     using `Step.do_/else_/catch_/finally_` blocks; their `Action` body
//!     is a no-op marker for schema/registry purposes.
//!   * Regular actions go through `ActionRegistry::get(&id).execute(...)`.
//!   * Every step's outcome is persisted to `step_runs` so that
//!     `lumo runs show <id>` reconstructs the run.

use crate::{
    action::{ActionRef, ActionResult},
    ai_hook::AiHookProvider,
    ctx::StepCtx,
    error::{ErrorKind, ExecError, StepError},
    registry::ActionRegistry,
};
use chrono::Utc;
use lumo_dsl::{AiMode, Flow, Step};
use lumo_storage::{FlowRunRow, Repo, StepRunRow};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::sync::Arc;
use std::time::Instant;
use ulid::Ulid;

#[derive(Debug, Clone)]
pub struct RunOptions {
    pub inputs: Value,
    pub trigger_kind: String,
}

impl Default for RunOptions {
    fn default() -> Self {
        Self {
            inputs: Value::Null,
            trigger_kind: "manual".into(),
        }
    }
}

#[derive(Debug)]
pub struct RunReport {
    pub run_id: String,
    pub success: bool,
    pub steps_total: usize,
    pub steps_ok: usize,
    pub steps_executed: usize,
    pub steps_failed: usize,
    pub steps_skipped: usize,
    pub steps_retried: usize,
    pub steps_caught: usize,
    pub duration_ms: u128,
    pub outputs: Option<Value>,
}

#[derive(Debug)]
pub struct RunHandle {
    pub run_id: String,
}

pub struct FlowVm {
    registry: ActionRegistry,
    repo: Option<Repo>,
    ai_provider: Option<Arc<dyn AiHookProvider>>,
}

impl FlowVm {
    pub fn new(registry: ActionRegistry, repo: Option<Repo>) -> Self {
        Self {
            registry,
            repo,
            ai_provider: None,
        }
    }

    /// Attach an AI hook provider so step-level / flow-level `ai:` blocks
    /// can activate selector heal / extract visual / decide fallbacks.
    pub fn with_ai_provider(mut self, provider: Arc<dyn AiHookProvider>) -> Self {
        self.ai_provider = Some(provider);
        self
    }

    pub fn registry(&self) -> &ActionRegistry {
        &self.registry
    }

    pub async fn run(&self, flow: &Flow, opts: RunOptions) -> Result<RunReport, ExecError> {
        let run_id = Ulid::new().to_string();
        let started = Instant::now();
        let now = Utc::now();

        let inputs = merge_input_defaults(&flow.spec.inputs, opts.inputs.clone())?;

        if let Some(repo) = &self.repo {
            let yaml = serde_yaml::to_string(flow).unwrap_or_default();
            let hash = Sha256::digest(yaml.as_bytes()).to_vec();
            let _ = repo.upsert_flow(
                &flow.metadata.id,
                &flow.metadata.version,
                &yaml,
                &hash,
                &flow.metadata.tags,
            );
            repo.create_run(&FlowRunRow {
                id: run_id.clone(),
                flow_id: flow.metadata.id.clone(),
                flow_version: flow.metadata.version.clone(),
                trigger_kind: opts.trigger_kind.clone(),
                inputs: inputs.clone(),
                outputs: None,
                state: "running".into(),
                worker_id: None,
                started_at: Some(now),
                finished_at: None,
                cost_token: 0,
                cost_usd_micro: 0,
                trace_id: None,
            })?;
        }

        let mut ctx = StepCtx::new(
            run_id.clone(),
            flow.metadata.id.clone(),
            self.registry.clone(),
            self.repo.clone(),
            inputs,
            flow.spec.capabilities.clone(),
            flow.spec.vault.clone(),
        )
        .with_ai(self.ai_provider.clone(), flow.metadata.ai.clone());

        let total = count_steps(&flow.spec.steps);
        let result = run_block_inline(&mut ctx, &flow.spec.steps).await;

        let ok = result.is_ok();
        let outputs = if ok {
            Some(ctx.outputs_snapshot())
        } else {
            None
        };
        if let Some(repo) = &self.repo {
            let _ = repo.finish_run(&run_id, if ok { "ok" } else { "failed" }, outputs.as_ref());
        }
        result?;
        let stats = ctx.stats();

        Ok(RunReport {
            run_id,
            success: ok,
            steps_total: total,
            steps_ok: stats.ok,
            steps_executed: stats.executed,
            steps_failed: stats.failed,
            steps_skipped: stats.skipped,
            steps_retried: stats.retried,
            steps_caught: stats.caught,
            duration_ms: started.elapsed().as_millis(),
            outputs,
        })
    }
}

/// Execute a list of steps inline within an existing context.
pub async fn run_block_inline(ctx: &mut StepCtx, steps: &[Step]) -> Result<(), ExecError> {
    run_block_at(ctx, steps, None, 0).await
}

async fn run_block_at(
    ctx: &mut StepCtx,
    steps: &[Step],
    parent_path: Option<String>,
    depth: i64,
) -> Result<(), ExecError> {
    for (idx, step) in steps.iter().enumerate() {
        let path = match &parent_path {
            Some(parent) => format!("{parent}/{}", step.id),
            None => step.id.clone(),
        };
        execute_step(ctx, step, idx as i64, path, parent_path.clone(), depth).await?;
    }
    Ok(())
}

async fn execute_step(
    ctx: &mut StepCtx,
    step: &Step,
    idx: i64,
    path: String,
    parent_path: Option<String>,
    depth: i64,
) -> Result<(), ExecError> {
    if let Some(cond) = &step.when {
        let rendered = render_string(ctx, cond)?;
        if !is_truthy_str(&rendered) {
            tracing::debug!("step `{}` skipped by when clause", step.id);
            let now = Utc::now();
            persist_step(
                ctx,
                StepPersist {
                    step_id: &step.id,
                    path: &path,
                    parent_path: parent_path.as_deref(),
                    depth,
                    idx,
                    state: "skipped",
                    attempt: 1,
                    input_hash: &[],
                    output: Some(&Value::Null),
                    error: None,
                    started_at: now,
                    finished_at: now,
                },
            );
            return Ok(());
        }
    }

    // ── Control-flow short-circuits ─────────────────────────────────────────
    match step.action.as_str() {
        "control.if" => return run_if(ctx, step, idx, path, parent_path, depth).await,
        "control.for" => return run_for(ctx, step, idx, path, parent_path, depth).await,
        "control.for_each" => return run_for_each(ctx, step, idx, path, parent_path, depth).await,
        "control.try" => return run_try(ctx, step, idx, path, parent_path, depth).await,
        "control.parallel" => return run_parallel(ctx, step, idx, path, parent_path, depth).await,
        _ => {}
    }

    // ── Regular action dispatch ─────────────────────────────────────────────
    let action: ActionRef = ctx
        .lookup_action(&step.action)
        .ok_or_else(|| ExecError::UnknownAction(step.action.clone()))?;

    let raw_input = serde_json::to_value(&step.with).unwrap_or(Value::Null);
    let tc = ctx.template_ctx();
    let rendered_input = lumo_dsl::render(&raw_input, &tc)?;
    let input_hash = Sha256::digest(rendered_input.to_string().as_bytes()).to_vec();
    let action_input = match ctx.resolve_vault_placeholders(&rendered_input) {
        Ok(v) => v,
        Err(e) => {
            let now = Utc::now();
            persist_step(
                ctx,
                StepPersist {
                    step_id: &step.id,
                    path: &path,
                    parent_path: parent_path.as_deref(),
                    depth,
                    idx,
                    state: "failed",
                    attempt: 1,
                    input_hash: &input_hash,
                    output: None,
                    error: Some(e.to_string()),
                    started_at: now,
                    finished_at: now,
                },
            );
            return Err(ExecError::Step {
                step: step.id.clone(),
                source: e,
            });
        }
    };

    let times = step.retry.as_ref().map(|r| r.times).unwrap_or(0);
    let backoff = step
        .retry
        .as_ref()
        .map(|r| r.backoff.clone())
        .unwrap_or_else(|| "fixed".into());
    let initial_ms = step.retry.as_ref().map(|r| r.initial_ms).unwrap_or(500);

    let mut attempt: u32 = 1;
    loop {
        let try_input = action_input.clone();
        let started_at = Utc::now();
        let t0 = Instant::now();
        match action.execute(ctx, try_input).await {
            Ok(result) => {
                let finished_at = Utc::now();
                let _elapsed_ms = t0.elapsed().as_millis() as i64;
                ctx.record_step_output(&step.id, &result.output);
                if let Some(bind) = &step.bind {
                    ctx.set_var(bind, result.output.clone());
                }
                persist_step(
                    ctx,
                    StepPersist {
                        step_id: &step.id,
                        path: &path,
                        parent_path: parent_path.as_deref(),
                        depth,
                        idx,
                        state: "ok",
                        attempt: attempt as i64,
                        input_hash: &input_hash,
                        output: Some(&result.output),
                        error: None,
                        started_at,
                        finished_at,
                    },
                );
                return Ok(());
            }
            Err(e) if attempt <= times => {
                let finished_at = Utc::now();
                let _elapsed_ms = t0.elapsed().as_millis() as i64;
                let error = e.to_string();
                persist_step(
                    ctx,
                    StepPersist {
                        step_id: &step.id,
                        path: &path,
                        parent_path: parent_path.as_deref(),
                        depth,
                        idx,
                        state: "retrying",
                        attempt: attempt as i64,
                        input_hash: &input_hash,
                        output: None,
                        error: Some(error.clone()),
                        started_at,
                        finished_at,
                    },
                );
                tracing::warn!(
                    "step `{}` failed attempt {}/{}: {}",
                    step.id,
                    attempt,
                    times + 1,
                    error
                );
                let delay = compute_backoff(&backoff, initial_ms, attempt);
                tokio::time::sleep(std::time::Duration::from_millis(delay)).await;
                attempt += 1;
            }
            Err(e) => {
                let finished_at = Utc::now();
                let _elapsed_ms = t0.elapsed().as_millis() as i64;
                let ai_mode = effective_ai_mode(ctx, step);
                let try_ai = matches!(ai_mode, AiMode::Fallback | AiMode::Primary)
                    && matches!(
                        e.kind(),
                        ErrorKind::SelectorNotFound | ErrorKind::ExtractFailed
                    );
                if try_ai {
                    match try_ai_recovery(ctx, step, &action, &action_input, &e).await {
                        Ok(Some(result)) => {
                            let now = Utc::now();
                            ctx.record_step_output(&step.id, &result.output);
                            if let Some(bind) = &step.bind {
                                ctx.set_var(bind, result.output.clone());
                            }
                            persist_step(
                                ctx,
                                StepPersist {
                                    step_id: &step.id,
                                    path: &path,
                                    parent_path: parent_path.as_deref(),
                                    depth,
                                    idx,
                                    state: "ai_healed",
                                    attempt: attempt as i64,
                                    input_hash: &input_hash,
                                    output: Some(&result.output),
                                    error: None,
                                    started_at,
                                    finished_at: now,
                                },
                            );
                            return Ok(());
                        }
                        Ok(None) => {
                            tracing::debug!("step `{}`: AI recovery returned no result", step.id);
                        }
                        Err(ai_err) => {
                            tracing::warn!(
                                "step `{}`: AI recovery itself failed: {}",
                                step.id,
                                ai_err
                            );
                        }
                    }
                }
                let mut error_msg = e.to_string();
                if let Some(diag) = maybe_diagnose(ctx, step, &error_msg).await {
                    error_msg = format!("{error_msg}\n[ai-diagnose] {diag}");
                }
                persist_step(
                    ctx,
                    StepPersist {
                        step_id: &step.id,
                        path: &path,
                        parent_path: parent_path.as_deref(),
                        depth,
                        idx,
                        state: "failed",
                        attempt: attempt as i64,
                        input_hash: &input_hash,
                        output: None,
                        error: Some(error_msg),
                        started_at,
                        finished_at,
                    },
                );
                return Err(ExecError::Step {
                    step: step.id.clone(),
                    source: e,
                });
            }
        }
    }
}

// ─── Control-flow inline runners ────────────────────────────────────────────

fn render_value_inline(ctx: &StepCtx, raw: &serde_yaml::Value) -> Result<Value, ExecError> {
    let v = serde_json::to_value(raw).unwrap_or(Value::Null);
    let tc = ctx.template_ctx();
    Ok(lumo_dsl::render(&v, &tc)?)
}

async fn run_if(
    ctx: &mut StepCtx,
    step: &Step,
    idx: i64,
    path: String,
    parent_path: Option<String>,
    depth: i64,
) -> Result<(), ExecError> {
    let started_at = Utc::now();
    let rendered = render_value_inline(ctx, &step.with)?;
    let input_hash = Sha256::digest(rendered.to_string().as_bytes()).to_vec();
    let cond = rendered.get("cond").cloned().unwrap_or(Value::Null);
    let ai_mode = effective_ai_mode(ctx, step);
    let need_ai = matches!(ai_mode, AiMode::Primary)
        || (matches!(ai_mode, AiMode::Fallback) && cond.is_null());
    let truthy = if need_ai {
        match try_ai_decide(ctx, step).await {
            Ok(Some(b)) => b,
            _ => is_truthy(&cond),
        }
    } else {
        is_truthy(&cond)
    };
    ctx.record_step_output(&step.id, &Value::Bool(truthy));
    let result = if truthy {
        if let Some(body) = &step.do_ {
            run_block_boxed(ctx, body, Some(format!("{path}/do")), depth + 1).await
        } else {
            Ok(())
        }
    } else if let Some(body) = &step.else_ {
        run_block_boxed(ctx, body, Some(format!("{path}/else")), depth + 1).await
    } else {
        Ok(())
    };
    let finished_at = Utc::now();
    persist_step(
        ctx,
        StepPersist {
            step_id: &step.id,
            path: &path,
            parent_path: parent_path.as_deref(),
            depth,
            idx,
            state: if result.is_ok() { "ok" } else { "failed" },
            attempt: 1,
            input_hash: &input_hash,
            output: Some(&Value::Bool(truthy)),
            error: result.as_ref().err().map(ToString::to_string),
            started_at,
            finished_at,
        },
    );
    result
}

async fn run_for(
    ctx: &mut StepCtx,
    step: &Step,
    idx: i64,
    path: String,
    parent_path: Option<String>,
    depth: i64,
) -> Result<(), ExecError> {
    let started_at = Utc::now();
    let rendered = render_value_inline(ctx, &step.with)?;
    let input_hash = Sha256::digest(rendered.to_string().as_bytes()).to_vec();
    let from = rendered.get("from").and_then(Value::as_i64).unwrap_or(0);
    let to = rendered
        .get("to")
        .and_then(Value::as_i64)
        .ok_or_else(|| ExecError::Other(anyhow::anyhow!("control.for requires `to`")))?;
    let stp = rendered
        .get("step")
        .and_then(Value::as_i64)
        .unwrap_or(1)
        .max(1);
    let bind = rendered
        .get("bind")
        .and_then(Value::as_str)
        .unwrap_or("index")
        .to_string();

    let body = step
        .do_
        .as_ref()
        .ok_or_else(|| ExecError::Other(anyhow::anyhow!("control.for requires `do:` block")))?;

    let mut i = from;
    let mut iters = 0u64;
    let mut result = Ok(());
    while i < to {
        ctx.push_binding(&bind, Value::from(i));
        ctx.push_binding("index", Value::from(iters as i64));
        result = run_block_boxed(ctx, body, Some(format!("{path}[{}]", iters)), depth + 1).await;
        ctx.clear_binding(&bind);
        ctx.clear_binding("index");
        if result.is_err() {
            break;
        }
        i += stp;
        iters += 1;
    }
    let output = serde_json::json!({ "iterations": iters });
    ctx.record_step_output(&step.id, &output);
    let finished_at = Utc::now();
    persist_step(
        ctx,
        StepPersist {
            step_id: &step.id,
            path: &path,
            parent_path: parent_path.as_deref(),
            depth,
            idx,
            state: if result.is_ok() { "ok" } else { "failed" },
            attempt: 1,
            input_hash: &input_hash,
            output: Some(&output),
            error: result.as_ref().err().map(ToString::to_string),
            started_at,
            finished_at,
        },
    );
    result
}

async fn run_for_each(
    ctx: &mut StepCtx,
    step: &Step,
    idx: i64,
    path: String,
    parent_path: Option<String>,
    depth: i64,
) -> Result<(), ExecError> {
    let started_at = Utc::now();
    let rendered = render_value_inline(ctx, &step.with)?;
    let input_hash = Sha256::digest(rendered.to_string().as_bytes()).to_vec();
    let items = rendered
        .get("in")
        .cloned()
        .ok_or_else(|| ExecError::Other(anyhow::anyhow!("control.for_each requires `in`")))?;
    let bind = rendered
        .get("bind")
        .and_then(Value::as_str)
        .unwrap_or("item")
        .to_string();

    let body = step.do_.as_ref().ok_or_else(|| {
        ExecError::Other(anyhow::anyhow!("control.for_each requires `do:` block"))
    })?;

    let arr: Vec<Value> = match items {
        Value::Array(a) => a,
        Value::Null => Vec::new(),
        other => {
            return Err(ExecError::Other(anyhow::anyhow!(
                "control.for_each `in` must be array, got {}",
                short_kind(&other)
            )))
        }
    };

    let mut iters = 0u64;
    let mut result = Ok(());
    for (idx, item) in arr.iter().enumerate() {
        ctx.push_binding(&bind, item.clone());
        // Also expose as `row` so flow authors can use the more readable
        // `{{ row.field }}` even when the binding name is `item`.
        ctx.push_binding("row", item.clone());
        ctx.push_binding("index", Value::from(idx as i64));
        result = run_block_boxed(ctx, body, Some(format!("{path}[{idx}]")), depth + 1).await;
        ctx.clear_binding(&bind);
        ctx.clear_binding("row");
        ctx.clear_binding("index");
        if result.is_err() {
            break;
        }
        iters += 1;
    }
    let output = serde_json::json!({ "iterations": iters });
    ctx.record_step_output(&step.id, &output);
    let finished_at = Utc::now();
    persist_step(
        ctx,
        StepPersist {
            step_id: &step.id,
            path: &path,
            parent_path: parent_path.as_deref(),
            depth,
            idx,
            state: if result.is_ok() { "ok" } else { "failed" },
            attempt: 1,
            input_hash: &input_hash,
            output: Some(&output),
            error: result.as_ref().err().map(ToString::to_string),
            started_at,
            finished_at,
        },
    );
    result
}

async fn run_try(
    ctx: &mut StepCtx,
    step: &Step,
    idx: i64,
    path: String,
    parent_path: Option<String>,
    depth: i64,
) -> Result<(), ExecError> {
    let started_at = Utc::now();
    let rendered = render_value_inline(ctx, &step.with)?;
    let input_hash = Sha256::digest(rendered.to_string().as_bytes()).to_vec();
    let body = step
        .do_
        .as_ref()
        .ok_or_else(|| ExecError::Other(anyhow::anyhow!("control.try requires `do:` block")))?;
    let result = run_block_boxed(ctx, body, Some(format!("{path}/try")), depth + 1).await;
    let caught = match result {
        Ok(()) => None,
        Err(e) => Some(e.to_string()),
    };
    let mut final_result = Ok(());
    if let Some(err) = &caught {
        ctx.set_var("error", Value::String(err.clone()));
        if let Some(c) = &step.catch_ {
            final_result = run_block_boxed(ctx, c, Some(format!("{path}/catch")), depth + 1).await;
        } else {
            // No catch block: rethrow after finally.
            let mut error = err.clone();
            if let Some(f) = &step.finally_ {
                if let Err(e) =
                    run_block_boxed(ctx, f, Some(format!("{path}/finally")), depth + 1).await
                {
                    error = e.to_string();
                }
            }
            let output = serde_json::json!({ "caught": caught });
            persist_control_result(
                ctx,
                step,
                &path,
                parent_path.as_deref(),
                depth,
                idx,
                "failed",
                &input_hash,
                &output,
                Some(error.clone()),
                started_at,
            );
            return Err(ExecError::Other(anyhow::anyhow!(error)));
        }
    }
    if let Some(f) = &step.finally_ {
        let finally_result =
            run_block_boxed(ctx, f, Some(format!("{path}/finally")), depth + 1).await;
        if final_result.is_ok() {
            final_result = finally_result;
        }
    }
    let output = serde_json::json!({ "caught": caught });
    ctx.record_step_output(&step.id, &output);
    let state = if final_result.is_err() {
        "failed"
    } else if caught.is_some() {
        "caught"
    } else {
        "ok"
    };
    persist_control_result(
        ctx,
        step,
        &path,
        parent_path.as_deref(),
        depth,
        idx,
        state,
        &input_hash,
        &output,
        final_result.as_ref().err().map(ToString::to_string),
        started_at,
    );
    final_result
}

async fn run_parallel(
    ctx: &mut StepCtx,
    step: &Step,
    idx: i64,
    path: String,
    parent_path: Option<String>,
    depth: i64,
) -> Result<(), ExecError> {
    // D-10: concurrent branch execution. StepCtx is Clone with Arc<Mutex<inner>>
    // so each branch sees and updates the same step bindings / vars. We use
    // `futures::future::join_all` to drive branches cooperatively on the
    // current task — that gives async concurrency for I/O-bound work
    // (browser, http, file) without needing the inner state to be Send.
    let started_at = Utc::now();
    let rendered = render_value_inline(ctx, &step.with)?;
    let input_hash = Sha256::digest(rendered.to_string().as_bytes()).to_vec();

    // Branches come from either `branches: [[...], [...]]` or — for back-compat
    // and one-step branches — from `do: [...]` where each entry is its own
    // single-step branch.
    let branches: Vec<Vec<Step>> = if let Some(b) = &step.branches {
        b.clone()
    } else if let Some(d) = &step.do_ {
        d.iter().map(|s| vec![s.clone()]).collect()
    } else {
        return Err(ExecError::Other(anyhow::anyhow!(
            "control.parallel requires `branches:` (Vec<Vec<Step>>) or `do:` (each step = one branch)"
        )));
    };

    if branches.is_empty() {
        ctx.record_step_output(&step.id, &Value::Null);
        persist_control_result(
            ctx,
            step,
            &path,
            parent_path.as_deref(),
            depth,
            idx,
            "ok",
            &input_hash,
            &Value::Null,
            None,
            started_at,
        );
        return Ok(());
    }

    // Materialize per-branch state on the stack so the futures can borrow into it.
    let mut branch_state: Vec<(StepCtx, Vec<Step>, String)> = branches
        .into_iter()
        .enumerate()
        .map(|(i, body)| (ctx.clone(), body, format!("{path}/branch[{i}]")))
        .collect();

    let futs: Vec<_> = branch_state
        .iter_mut()
        .map(|(c, body, branch_path)| {
            run_block_boxed(c, body.as_slice(), Some(branch_path.clone()), depth + 1)
        })
        .collect();

    let results = futures::future::join_all(futs).await;

    // First failure wins; everything else still completes.
    let first_err = results.into_iter().find_map(|r| r.err());
    let state = if first_err.is_some() { "failed" } else { "ok" };

    ctx.record_step_output(&step.id, &Value::Null);
    persist_control_result(
        ctx,
        step,
        &path,
        parent_path.as_deref(),
        depth,
        idx,
        state,
        &input_hash,
        &Value::Null,
        first_err.as_ref().map(ToString::to_string),
        started_at,
    );
    match first_err {
        Some(e) => Err(e),
        None => Ok(()),
    }
}

fn run_block_boxed<'a>(
    ctx: &'a mut StepCtx,
    steps: &'a [Step],
    parent_path: Option<String>,
    depth: i64,
) -> futures::future::BoxFuture<'a, Result<(), ExecError>> {
    Box::pin(run_block_at(ctx, steps, parent_path, depth))
}

// ─── persistence ────────────────────────────────────────────────────────────

struct StepPersist<'a> {
    step_id: &'a str,
    path: &'a str,
    parent_path: Option<&'a str>,
    depth: i64,
    idx: i64,
    state: &'a str,
    attempt: i64,
    input_hash: &'a [u8],
    output: Option<&'a Value>,
    error: Option<String>,
    started_at: chrono::DateTime<Utc>,
    finished_at: chrono::DateTime<Utc>,
}

#[allow(clippy::too_many_arguments)]
fn persist_control_result(
    ctx: &StepCtx,
    step: &Step,
    path: &str,
    parent_path: Option<&str>,
    depth: i64,
    idx: i64,
    state: &str,
    input_hash: &[u8],
    output: &Value,
    error: Option<String>,
    started_at: chrono::DateTime<Utc>,
) {
    persist_step(
        ctx,
        StepPersist {
            step_id: &step.id,
            path,
            parent_path,
            depth,
            idx,
            state,
            attempt: 1,
            input_hash,
            output: Some(output),
            error,
            started_at,
            finished_at: Utc::now(),
        },
    );
}

fn persist_step(ctx: &StepCtx, row: StepPersist<'_>) {
    ctx.mark_step_state(row.state);
    let Some(repo) = ctx.repo() else {
        return;
    };
    let stored = StepRunRow {
        flow_run_id: ctx.run_id().to_string(),
        seq: ctx.next_step_seq(),
        path: row.path.to_string(),
        parent_path: row.parent_path.map(ToString::to_string),
        depth: row.depth,
        step_id: row.step_id.to_string(),
        idx: row.idx,
        state: row.state.to_string(),
        attempt: row.attempt,
        input_hash: row.input_hash.to_vec(),
        output_json: row.output.cloned(),
        error: row.error,
        started_at: Some(row.started_at),
        finished_at: Some(row.finished_at),
        span_id: None,
    };
    if let Err(e) = repo.insert_step(&stored) {
        tracing::warn!("persist_step `{}`: {}", row.step_id, e);
    }
}

// ─── helpers ────────────────────────────────────────────────────────────────

fn render_string(ctx: &StepCtx, src: &str) -> Result<String, ExecError> {
    let tc = ctx.template_ctx();
    let v = lumo_dsl::render(&Value::String(src.to_string()), &tc)?;
    Ok(match v {
        Value::String(s) => s,
        other => other.to_string(),
    })
}

fn is_truthy_str(s: &str) -> bool {
    let t = s.trim();
    !matches!(
        t.to_ascii_lowercase().as_str(),
        "" | "false" | "0" | "null" | "none" | "no"
    )
}

fn is_truthy(v: &Value) -> bool {
    match v {
        Value::Bool(b) => *b,
        Value::Null => false,
        Value::Number(n) => n.as_f64().map(|f| f != 0.0).unwrap_or(false),
        Value::String(s) => is_truthy_str(s),
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
    }
}

fn short_kind(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "bool",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

fn compute_backoff(strategy: &str, initial_ms: u64, attempt: u32) -> u64 {
    match strategy {
        "exponential" => initial_ms.saturating_mul(2u64.saturating_pow(attempt - 1)),
        _ => initial_ms,
    }
}

fn count_steps(steps: &[Step]) -> usize {
    let mut n = 0usize;
    for s in steps {
        n += 1;
        for child in s.children() {
            n += count_steps(child);
        }
    }
    n
}

fn merge_input_defaults(decls: &[lumo_dsl::IoDecl], provided: Value) -> Result<Value, ExecError> {
    let mut out = match provided {
        Value::Object(m) => m,
        Value::Null => serde_json::Map::new(),
        other => {
            let mut m = serde_json::Map::new();
            m.insert("_raw".into(), other);
            m
        }
    };
    for d in decls {
        if out.contains_key(&d.name) {
            continue;
        }
        if let Some(def) = &d.default {
            let v = serde_json::to_value(def).unwrap_or(Value::Null);
            out.insert(d.name.clone(), v);
        }
    }
    for d in decls {
        let value = out.get(&d.name);
        if d.required && value.map(Value::is_null).unwrap_or(true) {
            return Err(ExecError::Other(anyhow::anyhow!(
                "missing required input `{}`",
                d.name
            )));
        }
        if let Some(value) = value {
            if !input_type_matches(&d.ty, value) {
                return Err(ExecError::Other(anyhow::anyhow!(
                    "input `{}` expected type `{}`, got {}",
                    d.name,
                    d.ty,
                    short_kind(value)
                )));
            }
        }
    }
    Ok(Value::Object(out))
}

fn input_type_matches(ty: &str, value: &Value) -> bool {
    if value.is_null() {
        return true;
    }
    match ty {
        "string" | "file" | "path" => value.is_string(),
        "number" => value.is_number(),
        "integer" | "int" => value.as_i64().is_some() || value.as_u64().is_some(),
        "boolean" | "bool" => value.is_boolean(),
        "array" | "list" => value.is_array(),
        "object" | "map" => value.is_object(),
        _ => true,
    }
}

// ─── AI hook dispatch ───────────────────────────────────────────────────────

fn effective_ai_mode(ctx: &StepCtx, step: &Step) -> AiMode {
    if ctx.ai_provider().is_none() {
        return AiMode::Off;
    }
    let flow_enabled = ctx.flow_ai().map(|f| f.enabled).unwrap_or(true);
    if !flow_enabled {
        return AiMode::Off;
    }
    step.ai.as_ref().map(|a| a.mode).unwrap_or(AiMode::Off)
}

fn effective_ai_model(ctx: &StepCtx, step: &Step) -> Option<String> {
    step.ai
        .as_ref()
        .and_then(|a| a.model.clone())
        .or_else(|| ctx.flow_ai().and_then(|f| f.model.clone()))
}

fn effective_ai_prompt(step: &Step) -> String {
    step.ai
        .as_ref()
        .and_then(|a| a.prompt.clone())
        .unwrap_or_else(|| format!("{}: {}", step.action, step.id))
}

/// Map a failed action error onto an AI helper and (where applicable) re-run
/// the deterministic action with the AI-suggested input. Returns
/// `Ok(Some(result))` if AI produced a usable outcome.
async fn try_ai_recovery(
    ctx: &mut StepCtx,
    step: &Step,
    action: &ActionRef,
    action_input: &Value,
    error: &StepError,
) -> Result<Option<ActionResult>, StepError> {
    let Some(provider) = ctx.ai_provider().cloned() else {
        return Ok(None);
    };
    let model = effective_ai_model(ctx, step);
    let prompt = effective_ai_prompt(step);

    match error.kind() {
        ErrorKind::SelectorNotFound => {
            let failed_selector = action_input
                .get("selector")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            let healed = provider
                .heal_selector(&failed_selector, &prompt, None, model.as_deref())
                .await?;
            let Some(new_sel) = healed.css.clone().or_else(|| healed.xpath.clone()) else {
                return Ok(None);
            };
            tracing::info!(
                "step `{}`: AI heal_selector → `{}` (confidence {:.2})",
                step.id,
                new_sel,
                healed.confidence
            );
            let mut new_input = action_input.clone();
            if let Some(obj) = new_input.as_object_mut() {
                obj.insert("selector".into(), Value::String(new_sel));
            }
            let result = action.execute(ctx, new_input).await?;
            Ok(Some(result))
        }
        ErrorKind::ExtractFailed => {
            let target = action_input
                .get("target")
                .and_then(Value::as_str)
                .map(str::to_string)
                .unwrap_or_else(|| prompt.clone());
            let value = provider
                .extract_visual(&target, None, None, model.as_deref())
                .await?;
            tracing::info!("step `{}`: AI extract_visual produced value", step.id);
            Ok(Some(ActionResult::from(value)))
        }
        _ => Ok(None),
    }
}

/// Call AI decide for a control.if step. Returns `Ok(Some(bool))` on success.
async fn try_ai_decide(ctx: &mut StepCtx, step: &Step) -> Result<Option<bool>, StepError> {
    let Some(provider) = ctx.ai_provider().cloned() else {
        return Ok(None);
    };
    let model = effective_ai_model(ctx, step);
    let prompt = effective_ai_prompt(step);
    let vars = ctx.vars_snapshot();
    let decision = provider.decide(&vars, &prompt, model.as_deref()).await?;
    tracing::info!(
        "step `{}`: AI decide → {} (confidence {:.2}) — {}",
        step.id,
        decision.result,
        decision.confidence,
        decision.reasoning
    );
    Ok(Some(decision.result))
}

/// Attach an LLM diagnostic when `metadata.ai.diagnose_on_failure: true`.
/// Returns `None` on any path that is unwanted or unavailable (best-effort).
async fn maybe_diagnose(ctx: &StepCtx, step: &Step, error: &str) -> Option<String> {
    let provider = ctx.ai_provider()?.clone();
    let flow_ai = ctx.flow_ai()?;
    if !flow_ai.enabled || !flow_ai.diagnose_on_failure {
        return None;
    }
    let model = effective_ai_model(ctx, step);
    match provider
        .diagnose(&step.id, &step.action, error, model.as_deref())
        .await
    {
        Ok(s) if !s.trim().is_empty() => Some(s),
        Ok(_) => None,
        Err(e) => {
            tracing::warn!("diagnose for step `{}` failed: {}", step.id, e);
            None
        }
    }
}
