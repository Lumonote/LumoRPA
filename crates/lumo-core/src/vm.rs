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
    ctx::StepCtx,
    error::{ExecError, StepError},
    registry::ActionRegistry,
};
use chrono::Utc;
use lumo_dsl::{Flow, Step};
use lumo_storage::{FlowRunRow, Repo, StepRunRow};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::time::Instant;
use ulid::Ulid;

#[derive(Debug, Clone)]
pub struct RunOptions {
    pub inputs: Value,
    pub trigger_kind: String,
}

impl Default for RunOptions {
    fn default() -> Self {
        Self { inputs: Value::Null, trigger_kind: "manual".into() }
    }
}

#[derive(Debug)]
pub struct RunReport {
    pub run_id: String,
    pub success: bool,
    pub steps_total: usize,
    pub steps_ok: usize,
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
}

impl FlowVm {
    pub fn new(registry: ActionRegistry, repo: Option<Repo>) -> Self {
        Self { registry, repo }
    }

    pub fn registry(&self) -> &ActionRegistry { &self.registry }

    pub async fn run(&self, flow: &Flow, opts: RunOptions) -> Result<RunReport, ExecError> {
        let run_id = Ulid::new().to_string();
        let started = Instant::now();
        let now = Utc::now();

        let inputs = merge_input_defaults(&flow.spec.inputs, opts.inputs.clone());

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
        );

        let total = count_steps(&flow.spec.steps);
        let result = run_block_inline(&mut ctx, &flow.spec.steps).await;

        let ok = result.is_ok();
        let outputs = if ok { Some(ctx.outputs_snapshot()) } else { None };
        if let Some(repo) = &self.repo {
            let _ = repo.finish_run(
                &run_id,
                if ok { "ok" } else { "failed" },
                outputs.as_ref(),
            );
        }
        result?;

        Ok(RunReport {
            run_id,
            success: ok,
            steps_total: total,
            steps_ok: total,
            duration_ms: started.elapsed().as_millis(),
            outputs,
        })
    }
}

/// Execute a list of steps inline within an existing context.
pub async fn run_block_inline(
    ctx: &mut StepCtx,
    steps: &[Step],
) -> Result<(), ExecError> {
    for (idx, step) in steps.iter().enumerate() {
        execute_step(ctx, step, idx as i64).await?;
    }
    Ok(())
}

async fn execute_step(
    ctx: &mut StepCtx,
    step: &Step,
    idx: i64,
) -> Result<(), ExecError> {
    if let Some(cond) = &step.when {
        let rendered = render_string(ctx, cond)?;
        if !is_truthy_str(&rendered) {
            tracing::debug!("step `{}` skipped by when clause", step.id);
            return Ok(());
        }
    }

    // ── Control-flow short-circuits ─────────────────────────────────────────
    match step.action.as_str() {
        "control.if"       => return run_if(ctx, step, idx).await,
        "control.for"      => return run_for(ctx, step, idx).await,
        "control.for_each" => return run_for_each(ctx, step, idx).await,
        "control.try"      => return run_try(ctx, step, idx).await,
        "control.parallel" => return run_parallel(ctx, step, idx).await,
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
    let started_at = Utc::now();
    let t0 = Instant::now();

    let times = step.retry.as_ref().map(|r| r.times).unwrap_or(0);
    let backoff = step.retry.as_ref().map(|r| r.backoff.clone()).unwrap_or_else(|| "fixed".into());
    let initial_ms = step.retry.as_ref().map(|r| r.initial_ms).unwrap_or(500);

    let mut attempt: u32 = 1;
    let outcome: Result<ActionResult, StepError> = loop {
        let try_input = rendered_input.clone();
        match action.execute(ctx, try_input).await {
            Ok(r) => break Ok(r),
            Err(e) if attempt <= times => {
                tracing::warn!(
                    "step `{}` failed attempt {}/{}: {}", step.id, attempt, times + 1, e
                );
                let delay = compute_backoff(&backoff, initial_ms, attempt);
                tokio::time::sleep(std::time::Duration::from_millis(delay)).await;
                attempt += 1;
            }
            Err(e) => break Err(e),
        }
    };

    let finished_at = Utc::now();
    let _elapsed_ms = t0.elapsed().as_millis() as i64;

    match outcome {
        Ok(result) => {
            ctx.record_step_output(&step.id, &result.output);
            if let Some(bind) = &step.bind {
                ctx.set_var(bind, result.output.clone());
            }
            persist_step(ctx, &step.id, idx, "ok", attempt as i64,
                &input_hash, Some(&result.output), None, started_at, finished_at);
            Ok(())
        }
        Err(e) => {
            persist_step(ctx, &step.id, idx, "failed", attempt as i64,
                &input_hash, None, Some(e.to_string()), started_at, finished_at);
            Err(ExecError::Step { step: step.id.clone(), source: e })
        }
    }
}

// ─── Control-flow inline runners ────────────────────────────────────────────

fn render_value_inline(ctx: &StepCtx, raw: &serde_yaml::Value) -> Result<Value, ExecError> {
    let v = serde_json::to_value(raw).unwrap_or(Value::Null);
    let tc = ctx.template_ctx();
    Ok(lumo_dsl::render(&v, &tc)?)
}

async fn run_if(ctx: &mut StepCtx, step: &Step, _idx: i64) -> Result<(), ExecError> {
    let rendered = render_value_inline(ctx, &step.with)?;
    let cond = rendered.get("cond").cloned().unwrap_or(Value::Null);
    let truthy = is_truthy(&cond);
    ctx.record_step_output(&step.id, &Value::Bool(truthy));
    if truthy {
        if let Some(body) = &step.do_ { run_block_inline_boxed(ctx, body).await?; }
    } else if let Some(body) = &step.else_ {
        run_block_inline_boxed(ctx, body).await?;
    }
    Ok(())
}

async fn run_for(ctx: &mut StepCtx, step: &Step, _idx: i64) -> Result<(), ExecError> {
    let rendered = render_value_inline(ctx, &step.with)?;
    let from = rendered.get("from").and_then(Value::as_i64).unwrap_or(0);
    let to   = rendered.get("to").and_then(Value::as_i64)
        .ok_or_else(|| ExecError::Other(anyhow::anyhow!("control.for requires `to`")))?;
    let stp  = rendered.get("step").and_then(Value::as_i64).unwrap_or(1).max(1);
    let bind = rendered.get("bind").and_then(Value::as_str).unwrap_or("index").to_string();

    let body = step.do_.as_ref()
        .ok_or_else(|| ExecError::Other(anyhow::anyhow!("control.for requires `do:` block")))?;

    let mut i = from;
    let mut iters = 0u64;
    while i < to {
        ctx.push_binding(&bind, Value::from(i));
        ctx.push_binding("index", Value::from(iters as i64));
        run_block_inline_boxed(ctx, body).await?;
        ctx.clear_binding(&bind);
        ctx.clear_binding("index");
        i += stp;
        iters += 1;
    }
    ctx.record_step_output(&step.id, &serde_json::json!({ "iterations": iters }));
    Ok(())
}

async fn run_for_each(ctx: &mut StepCtx, step: &Step, _idx: i64) -> Result<(), ExecError> {
    let rendered = render_value_inline(ctx, &step.with)?;
    let items = rendered.get("in").cloned()
        .ok_or_else(|| ExecError::Other(anyhow::anyhow!("control.for_each requires `in`")))?;
    let bind = rendered.get("bind").and_then(Value::as_str).unwrap_or("item").to_string();

    let body = step.do_.as_ref()
        .ok_or_else(|| ExecError::Other(anyhow::anyhow!("control.for_each requires `do:` block")))?;

    let arr: Vec<Value> = match items {
        Value::Array(a) => a,
        Value::Null => Vec::new(),
        other => return Err(ExecError::Other(anyhow::anyhow!(
            "control.for_each `in` must be array, got {}", short_kind(&other)
        ))),
    };

    let mut iters = 0u64;
    for (idx, item) in arr.iter().enumerate() {
        ctx.push_binding(&bind, item.clone());
        // Also expose as `row` so flow authors can use the more readable
        // `{{ row.field }}` even when the binding name is `item`.
        ctx.push_binding("row", item.clone());
        ctx.push_binding("index", Value::from(idx as i64));
        run_block_inline_boxed(ctx, body).await?;
        ctx.clear_binding(&bind);
        ctx.clear_binding("row");
        ctx.clear_binding("index");
        iters += 1;
    }
    ctx.record_step_output(&step.id, &serde_json::json!({ "iterations": iters }));
    Ok(())
}

async fn run_try(ctx: &mut StepCtx, step: &Step, _idx: i64) -> Result<(), ExecError> {
    let body = step.do_.as_ref()
        .ok_or_else(|| ExecError::Other(anyhow::anyhow!("control.try requires `do:` block")))?;
    let result = run_block_inline_boxed(ctx, body).await;
    let caught = match result {
        Ok(()) => None,
        Err(e) => Some(e.to_string()),
    };
    if let Some(err) = &caught {
        ctx.set_var("error", Value::String(err.clone()));
        if let Some(c) = &step.catch_ {
            run_block_inline_boxed(ctx, c).await?;
        } else {
            // No catch block: rethrow after finally.
            if let Some(f) = &step.finally_ { run_block_inline_boxed(ctx, f).await?; }
            return Err(ExecError::Other(anyhow::anyhow!(err.clone())));
        }
    }
    if let Some(f) = &step.finally_ { run_block_inline_boxed(ctx, f).await?; }
    ctx.record_step_output(&step.id, &serde_json::json!({
        "caught": caught,
    }));
    Ok(())
}

async fn run_parallel(ctx: &mut StepCtx, step: &Step, _idx: i64) -> Result<(), ExecError> {
    // M1 simplification: run children in declaration order (sequential).
    // Real parallelism lands in M3 once StepCtx is Send-clonable per branch.
    let body = step.do_.as_ref()
        .ok_or_else(|| ExecError::Other(anyhow::anyhow!("control.parallel requires `do:` block")))?;
    run_block_inline_boxed(ctx, body).await?;
    ctx.record_step_output(&step.id, &Value::Null);
    Ok(())
}

fn run_block_inline_boxed<'a>(
    ctx: &'a mut StepCtx,
    steps: &'a [Step],
) -> futures::future::BoxFuture<'a, Result<(), ExecError>> {
    Box::pin(run_block_inline(ctx, steps))
}

// ─── persistence ────────────────────────────────────────────────────────────

#[allow(clippy::too_many_arguments)]
fn persist_step(
    ctx: &StepCtx,
    step_id: &str,
    idx: i64,
    state: &str,
    attempt: i64,
    input_hash: &[u8],
    output: Option<&Value>,
    error: Option<String>,
    started_at: chrono::DateTime<Utc>,
    finished_at: chrono::DateTime<Utc>,
) {
    let Some(repo) = ctx.repo() else { return; };
    let row = StepRunRow {
        flow_run_id: ctx.run_id().to_string(),
        step_id: step_id.to_string(),
        idx,
        state: state.to_string(),
        attempt,
        input_hash: input_hash.to_vec(),
        output_json: output.cloned(),
        error,
        started_at: Some(started_at),
        finished_at: Some(finished_at),
        span_id: None,
    };
    if let Err(e) = repo.insert_step(&row) {
        tracing::warn!("persist_step `{}`: {}", step_id, e);
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

fn merge_input_defaults(
    decls: &[lumo_dsl::IoDecl],
    provided: Value,
) -> Value {
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
        if out.contains_key(&d.name) { continue; }
        if let Some(def) = &d.default {
            let v = serde_json::to_value(def).unwrap_or(Value::Null);
            out.insert(d.name.clone(), v);
        }
    }
    Value::Object(out)
}
