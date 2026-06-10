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
    ai_hook::{AiCallUsage, AiHookProvider},
    ctx::{CancelToken, StepCtx},
    error::{ErrorKind, ExecError, StepError},
    registry::ActionRegistry,
};
use chrono::Utc;
use lumo_dsl::{AiMode, Capabilities, Flow, Step};
use lumo_storage::{AiCallInsert, FlowRunRow, Repo, StepRunRow};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tracing::Instrument;
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
    /// F-20: when the run paused at a breakpoint / single-step, the path of the
    /// step it paused *before* (which did not execute). `None` for a run that
    /// completed, failed, or was cancelled without hitting a breakpoint.
    pub paused_at: Option<String>,
}

#[derive(Debug)]
pub struct RunHandle {
    pub run_id: String,
}

pub struct FlowVm {
    registry: ActionRegistry,
    repo: Option<Repo>,
    ai_provider: Option<Arc<dyn AiHookProvider>>,
    /// P0-5: nesting depth to seed into the run's `StepCtx`. Sub-flow runners
    /// (`skill.invoke`) bump this so recursion can be bounded.
    skill_depth: u32,
    /// P0-5: when set, replaces `flow.spec.capabilities` for the run. Used by
    /// `skill.invoke` to clamp a sub-flow to the caller's sandbox.
    capability_override: Option<Capabilities>,
    /// P1-1: cooperative cancellation handle for the run.
    cancel: Option<CancelToken>,
    /// P1-1: per-step timeout applied to every leaf action's execution.
    step_timeout: Option<Duration>,
    /// P1-3: optional age identity, threaded into each run's `StepCtx` so
    /// `${{ vault.* }}` can fall back to the encrypted store.
    vault_identity: Option<Arc<lumo_storage::VaultIdentity>>,
    /// F-13: when set, resume from this prior run id — steps it already
    /// completed (matching path + input hash) are replayed from `step_runs`
    /// instead of re-executed. Requires a repo.
    resume_from: Option<String>,
    /// F-20: step paths to pause *before* (breakpoints). Empty ⇒ no breakpoints.
    breakpoints: std::collections::HashSet<String>,
    /// F-20: single-step mode — pause before every executing step. Combined with
    /// `resume_from`, "continue/step" a paused run to the next pause point.
    step_mode: bool,
    /// P0-1（桌面接线）：宿主预生成的 run_id。取消表必须在 run 启动前就按
    /// 最终落库的 run_id 建键，否则 `cancel_run` 无键可查。`None` ⇒ 引擎自行
    /// 生成 ULID（原行为）。注意 FlowVm 是一次性按 run 构建的，复用同一实例
    /// 跑两次会撞 run_id —— 宿主侧每次运行都新建 VM。
    run_id_override: Option<String>,
    /// X-07（时光回溯）：artifacts 落盘根目录。设置后 `StepCtx::attach_artifact`
    /// 才真正写盘 + 落库；不设保持 no-op，无头冒烟不会产生垃圾文件。
    artifacts_dir: Option<std::path::PathBuf>,
    /// P1（人机交互）：宿主注入的 human prompt 通道，播种进每个 run 的
    /// `StepCtx`。`None`（默认）⇒ `human.*` 动作报"宿主不支持人机交互"。
    human_prompter: Option<Arc<dyn crate::human::HumanPrompter>>,
}

impl FlowVm {
    pub fn new(registry: ActionRegistry, repo: Option<Repo>) -> Self {
        Self {
            registry,
            repo,
            ai_provider: None,
            skill_depth: 0,
            capability_override: None,
            cancel: None,
            step_timeout: None,
            vault_identity: None,
            resume_from: None,
            breakpoints: std::collections::HashSet::new(),
            step_mode: false,
            run_id_override: None,
            artifacts_dir: None,
            human_prompter: None,
        }
    }

    /// P1（人机交互）：注入宿主的 [`crate::human::HumanPrompter`]，`human.*`
    /// 动作经 `StepCtx` 取用。`None` 保持默认（宿主不支持人机交互）。
    pub fn with_human_prompter(
        mut self,
        prompter: Option<Arc<dyn crate::human::HumanPrompter>>,
    ) -> Self {
        self.human_prompter = prompter;
        self
    }

    /// P0-1：宿主预生成 run_id（语义见字段注释）。`None` 保持引擎自生成。
    pub fn with_run_id(mut self, run_id: Option<String>) -> Self {
        self.run_id_override = run_id;
        self
    }

    /// X-07：设置 artifacts 落盘根目录（语义见字段注释）。`None` 保持 no-op。
    pub fn with_artifacts_dir(mut self, dir: Option<std::path::PathBuf>) -> Self {
        self.artifacts_dir = dir;
        self
    }

    /// Attach an AI hook provider so step-level / flow-level `ai:` blocks
    /// can activate selector heal / extract visual / decide fallbacks.
    pub fn with_ai_provider(mut self, provider: Arc<dyn AiHookProvider>) -> Self {
        self.ai_provider = Some(provider);
        self
    }

    /// Seed the run's `skill.invoke` nesting depth (P0-5).
    pub fn with_skill_depth(mut self, depth: u32) -> Self {
        self.skill_depth = depth;
        self
    }

    /// Override the run's capability sandbox (P0-5). `skill.invoke` passes the
    /// caller's capabilities clamped to the skill's declared set.
    pub fn with_capability_override(mut self, caps: Capabilities) -> Self {
        self.capability_override = Some(caps);
        self
    }

    /// Attach a cancellation handle (P1-1). Hold a clone of the same
    /// [`CancelToken`] elsewhere and call `cancel()` to stop the run.
    pub fn with_cancel(mut self, cancel: CancelToken) -> Self {
        self.cancel = Some(cancel);
        self
    }

    /// Set a per-step timeout (P1-1). Each leaf action that runs longer than
    /// this fails the run with [`ExecError::Timeout`].
    pub fn with_step_timeout(mut self, timeout: Duration) -> Self {
        self.step_timeout = Some(timeout);
        self
    }

    /// Attach the age identity for `${{ vault.* }}` store decryption (P1-3).
    /// `None` keeps resolution env-only.
    pub fn with_vault(mut self, identity: Option<Arc<lumo_storage::VaultIdentity>>) -> Self {
        self.vault_identity = identity;
        self
    }

    /// Resume a prior run by id (F-13): steps it already completed are replayed
    /// from `step_runs` instead of re-executed. Requires a repo; without one the
    /// request is ignored (with a warning) and the flow runs fresh.
    pub fn with_resume_from(mut self, run_id: Option<String>) -> Self {
        self.resume_from = run_id;
        self
    }

    /// F-20: pause the run *before* each step whose path is in `paths`
    /// (breakpoint debugging). Combine with [`Self::with_resume_from`] to
    /// continue a paused run to the next breakpoint.
    pub fn with_breakpoints(mut self, paths: std::collections::HashSet<String>) -> Self {
        self.breakpoints = paths;
        self
    }

    /// F-20: single-step mode — pause before every executing step. On a resumed
    /// run the step being stepped off executes first, then the next one pauses.
    pub fn with_step_mode(mut self, on: bool) -> Self {
        self.step_mode = on;
        self
    }

    /// F-13: build the replay memo from a prior run's `step_runs`. `None` when
    /// not resuming, no repo is configured, or the prior run has no reusable
    /// (terminal-success) steps — any of which simply runs the flow fresh.
    fn load_resume_memo(&self) -> Option<Arc<crate::ctx::ResumeMemo>> {
        let prior = self.resume_from.as_ref()?;
        let Some(repo) = &self.repo else {
            tracing::warn!("resume requested ({prior}) but no repo is configured; running fresh");
            return None;
        };
        let rows = match repo.list_steps(prior) {
            Ok(rows) => rows,
            Err(e) => {
                tracing::warn!("resume: cannot read prior run {prior}: {e}; running fresh");
                return None;
            }
        };
        // `list_steps` is ordered by seq, so a later successful attempt for a
        // path overrides an earlier one. Only terminal-success states carry a
        // reusable output; failed / retrying / timeout / cancelled steps are
        // intentionally omitted so resume re-executes them.
        let mut memo = crate::ctx::ResumeMemo::default();
        for r in rows {
            if matches!(r.state.as_str(), "ok" | "ai_healed" | "cached") {
                if let Some(output) = r.output_json {
                    memo.record(r.path, r.input_hash, output);
                }
            }
        }
        if memo.is_empty() {
            tracing::warn!("resume: prior run {prior} has no reusable steps; running fresh");
            return None;
        }
        tracing::info!(
            "resume: replaying {} completed step(s) from run {prior}",
            memo.len()
        );
        Some(Arc::new(memo))
    }

    pub fn registry(&self) -> &ActionRegistry {
        &self.registry
    }

    /// F-20: build the debug controller for this run, or `None` when no
    /// breakpoints are set and single-step is off (zero overhead). Armed for a
    /// fresh run (a breakpoint may fire on the first step); disarmed for a
    /// resume so the step we paused on last time executes before the next pause.
    fn build_debug_controller(&self) -> Option<crate::ctx::DebugController> {
        if !self.step_mode && self.breakpoints.is_empty() {
            return None;
        }
        Some(crate::ctx::DebugController::new(
            self.breakpoints.clone(),
            self.step_mode,
            self.resume_from.is_none(),
        ))
    }

    pub async fn run(&self, flow: &Flow, opts: RunOptions) -> Result<RunReport, ExecError> {
        let run_id = self
            .run_id_override
            .clone()
            .unwrap_or_else(|| Ulid::new().to_string());
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
            self.capability_override
                .clone()
                .unwrap_or_else(|| flow.spec.capabilities.clone()),
            flow.spec.vault.clone(),
        )
        .with_ai(self.ai_provider.clone(), flow.metadata.ai.clone())
        .with_skill_depth(self.skill_depth)
        .with_cancel(self.cancel.clone())
        .with_step_timeout(self.step_timeout)
        .with_vault(self.vault_identity.clone())
        .with_resume_memo(self.load_resume_memo())
        .with_debug(self.build_debug_controller())
        .with_human_prompter(self.human_prompter.clone())
        .with_resources(flow.spec.resources.clone());
        // X-07：仅在宿主显式开启时给 ctx 接上 artifacts 目录——StepCtx 的
        // builder 收 PathBuf 而非 Option，这里用 if-let 保持未开启路径零开销。
        if let Some(dir) = &self.artifacts_dir {
            ctx = ctx.with_artifacts_dir(dir.clone());
        }

        let total = count_steps(&flow.spec.steps);
        let result = run_block_inline(&mut ctx, &flow.spec.steps).await;

        // P1-2: reclaim run-scoped external resources (e.g. a launched browser
        // process) whether the flow succeeded or failed. Action crates register
        // teardown hooks; each is handed this run's id so it drops only its own
        // state. Runs before the error is propagated below so a failing flow
        // can't leak a headless Chrome.
        for hook in self.registry.teardowns() {
            hook.teardown(&run_id).await;
        }

        let ok = result.is_ok();
        let cancelled = matches!(result, Err(ExecError::Cancelled));
        // F-20: a breakpoint / single-step pause. The authoritative "where" is
        // the ctx's debug controller (set when the breakpoint fired), not the
        // error variant — `control.try`'s no-catch path re-wraps the pause error
        // as `Other`, so we must not rely on matching `ExecError::Paused` here.
        let paused_at = ctx.debug_paused_at();
        let outputs = if ok {
            Some(ctx.outputs_snapshot())
        } else {
            None
        };
        if let Some(repo) = &self.repo {
            // X-10: aggregate every ai_calls row from this run into the
            // flow_runs cost columns before we close the run. After this the
            // CLI / Studio can show "this run cost $0.012 / 1.2k tokens"
            // without re-scanning ai_calls every render.
            let _ = repo.rollup_run_cost(&run_id);
            let state = if paused_at.is_some() {
                "paused"
            } else if ok {
                "ok"
            } else if cancelled {
                "cancelled"
            } else {
                "failed"
            };
            let _ = repo.finish_run(&run_id, state, outputs.as_ref());
        }
        // A paused run is not an error to the caller — it's surfaced via the
        // report's `paused_at`. Non-paused errors (cancel, uncaught failure)
        // still propagate as before.
        if paused_at.is_none() {
            result?;
        }
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
            paused_at,
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

/// Outcome of running one action attempt under cancel/timeout limits (P1-1).
enum StepOutcome {
    Done(Result<ActionResult, StepError>),
    Cancelled,
    TimedOut,
}

/// Resolves when the (optional) cancel token fires; never resolves when no
/// token is attached, so it idles harmlessly inside `select!`.
async fn wait_cancel(cancel: &Option<CancelToken>) {
    match cancel {
        Some(c) => c.cancelled().await,
        None => std::future::pending::<()>().await,
    }
}

/// Resolves after the (optional) per-step timeout elapses; never resolves when
/// no timeout is set.
async fn wait_timeout(limit: Option<Duration>) {
    match limit {
        Some(d) => tokio::time::sleep(d).await,
        None => std::future::pending::<()>().await,
    }
}

async fn execute_step(
    ctx: &mut StepCtx,
    step: &Step,
    idx: i64,
    path: String,
    parent_path: Option<String>,
    depth: i64,
) -> Result<(), ExecError> {
    // P1-1: stop before doing any work (including `when` evaluation and
    // control-flow recursion) if the run was cancelled. The first step to
    // observe cancellation persists a `cancelled` row and aborts; the error
    // propagates up so no further steps run.
    if ctx.is_cancelled() {
        let now = Utc::now();
        persist_step(
            ctx,
            StepPersist {
                step_id: &step.id,
                path: &path,
                parent_path: parent_path.as_deref(),
                depth,
                idx,
                state: "cancelled",
                attempt: 1,
                input_hash: &[],
                output: None,
                error: Some("run cancelled".into()),
                started_at: now,
                finished_at: now,
            },
        )
        .await;
        return Err(ExecError::Cancelled);
    }

    // F-20: once a breakpoint has fired this run, every subsequent step short-
    // circuits (sticky) — no work, no persisted row — so the pause unwinds the
    // whole step tree cleanly, including out through `control.try` / `parallel`
    // wrappers that would otherwise catch or re-wrap the pause error.
    if ctx.is_debug_paused() {
        return Err(ExecError::Paused);
    }

    if let Some(cond) = &step.when {
        // B1 (F-14): evaluate `when` as a boolean expression (operators +
        // identifier paths); `{{ }}` template mode is preserved for back-compat.
        if !lumo_dsl::eval_predicate(cond, &ctx.template_ctx())? {
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
            )
            .await;
            return Ok(());
        }
    }

    // ── Control-flow short-circuits ─────────────────────────────────────────
    match step.action.as_str() {
        "control.if" => return run_if(ctx, step, idx, path, parent_path, depth).await,
        "control.for" => return run_for(ctx, step, idx, path, parent_path, depth).await,
        "control.for_each" => return run_for_each(ctx, step, idx, path, parent_path, depth).await,
        "control.while" => return run_while(ctx, step, idx, path, parent_path, depth).await,
        "control.try" => return run_try(ctx, step, idx, path, parent_path, depth).await,
        "control.parallel" => return run_parallel(ctx, step, idx, path, parent_path, depth).await,
        // 指令集 P1:break / continue 是循环控制信号,以 `Err` 向上 unwind,由最近
        // 的循环容器(while / for / for_each)消化。与 Paused 一样不落 step 行——
        // 它不是一次"执行"而是一次跳转;可观测痕迹是循环容器行的 iterations。
        // `when:` 在进入本 match 前已求值,因此 `when + control.break` 天然支持
        // 条件跳出。循环外使用由 lumo-dsl validate 静态拦截,这里的信号若一路
        // 逃逸到 flow 顶层,会以 "used outside of a loop" 作为运行期兜底错误。
        "control.break" => {
            return Err(ExecError::Break {
                step: step.id.clone(),
            })
        }
        "control.continue" => {
            return Err(ExecError::Continue {
                step: step.id.clone(),
            })
        }
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
    // F-13 (durable resume): if a prior run already completed this exact step
    // (same path *and* same rendered-input hash), replay its persisted output
    // instead of re-executing. Record it into ctx so downstream templates /
    // binds observe the same value a fresh execution would have produced.
    if let Some(output) = ctx.resume_hit(&path, &input_hash) {
        ctx.record_step_output(&step.id, &output);
        if let Some(bind) = &step.bind {
            ctx.set_var(bind, output.clone());
        }
        let now = Utc::now();
        persist_step(
            ctx,
            StepPersist {
                step_id: &step.id,
                path: &path,
                parent_path: parent_path.as_deref(),
                depth,
                idx,
                state: "cached",
                attempt: 1,
                input_hash: &input_hash,
                output: Some(&output),
                error: None,
                started_at: now,
                finished_at: now,
            },
        )
        .await;
        return Ok(());
    }
    // F-20: pause *before* executing this step when a breakpoint is set on its
    // path or we're single-stepping. Placed past the resume replay above so a
    // resumed run fast-forwards through already-completed steps without
    // re-triggering breakpoints, and "steps off" the step it last paused on.
    // The step is left un-persisted (it never ran), so a subsequent resume
    // replays everything before it and re-executes from here.
    if ctx.debug_should_pause(&path) {
        ctx.debug_mark_paused(&path);
        return Err(ExecError::Paused);
    }
    // B2 (F-17): validate the rendered `with:` against the action's schema
    // before dispatch — a missing / typo'd / mistyped param fails fast with a
    // clear message instead of surfacing as a confusing error inside the action.
    if let Err(msg) = crate::schema::validate_input(action.schema(), &rendered_input) {
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
                error: Some(format!("schema validation failed: {msg}")),
                started_at: now,
                finished_at: now,
            },
        )
        .await;
        return Err(ExecError::Step {
            step: step.id.clone(),
            source: StepError::msg(format!("invalid `with`: {msg}")),
        });
    }
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
            )
            .await;
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
    // B3 (F-16): error-kind filter for retries. Empty ⇒ retry on any error.
    let retry_on = step
        .retry
        .as_ref()
        .map(|r| r.on.clone())
        .unwrap_or_default();

    // Make the step id visible to the action so cost / OTel rows can be
    // attributed correctly (X-10). Also expose the full nested path so
    // `attach_artifact` (X-07 time-travel) lines blobs up against the
    // step_runs path column.
    ctx.set_current_step(&step.id);
    ctx.set_current_step_path(&path);
    // T3: bind this step to its declared resource (if any) so a resource-aware
    // action can resolve the shared, run-scoped handle. `None` ⇒ unchanged
    // per-step behavior.
    ctx.set_current_resource(step.resource.as_deref());

    let mut attempt: u32 = 1;
    loop {
        let try_input = action_input.clone();
        let started_at = Utc::now();
        let t0 = Instant::now();
        // X-05: OTel GenAI semconv — wrap each action execution in a tracing
        // span carrying the canonical `otel.*` / `step.*` / `flow.run_id`
        // fields. `tracing` spans are the OpenTelemetry data source any
        // subscriber/exporter consumes; we use `Instrument` (not `enter()`)
        // because the span must stay attached across the `.await` boundary.
        let exec_span = tracing::info_span!(
            "lumo.step.execute",
            "otel.name" = %format!("lumo.step {}", step.id),
            "step.id" = %step.id,
            "step.action" = %step.action,
            "step.path" = %path,
            "flow.run_id" = %ctx.run_id(),
        );
        // P1-1: run the action under the run's cancel token + per-step timeout.
        // The future borrows `ctx` mutably, so resolve it inside its own scope
        // and carry only an owned outcome out — freeing `ctx` for the persist
        // calls below. `biased` makes cancel/timeout win deterministically.
        let cancel = ctx.cancel_token();
        let limit = ctx.step_timeout();
        // P0-2:为本 attempt 装填步级中断位。`select!` 超时/取消只能 drop 动作
        // future,动作里 `spawn_blocking` 的阻塞任务不会随之停止;判死后对该位
        // 置 true,阻塞闭包在循环/写回(commit)边界检查它提前退出,副作用不落地。
        let interrupt = ctx.arm_step_interrupt();
        let outcome = {
            let exec_fut = action.execute(ctx, try_input).instrument(exec_span);
            tokio::pin!(exec_fut);
            tokio::select! {
                biased;
                _ = wait_cancel(&cancel) => StepOutcome::Cancelled,
                _ = wait_timeout(limit) => StepOutcome::TimedOut,
                r = &mut exec_fut => StepOutcome::Done(r),
            }
        };
        let exec_result = match outcome {
            StepOutcome::Cancelled => {
                // P0-2:future 已 drop,但其孤儿阻塞任务可能还在跑 —— 翻中断位
                // 让它在下一个检查点退出(运行级 CancelToken 也已翻转,这里补翻
                // 步级位保持两条路径行为一致)。
                interrupt.store(true, std::sync::atomic::Ordering::SeqCst);
                persist_step(
                    ctx,
                    StepPersist {
                        step_id: &step.id,
                        path: &path,
                        parent_path: parent_path.as_deref(),
                        depth,
                        idx,
                        state: "cancelled",
                        attempt: attempt as i64,
                        input_hash: &input_hash,
                        output: None,
                        error: Some("run cancelled".into()),
                        started_at,
                        finished_at: Utc::now(),
                    },
                )
                .await;
                return Err(ExecError::Cancelled);
            }
            StepOutcome::TimedOut => {
                // P0-2:先翻中断位再走任一分支 —— 被 drop 的 future 留下的孤儿
                // 阻塞任务必须停在下一个检查点,事务/写回不得在判死后落地。
                interrupt.store(true, std::sync::atomic::Ordering::SeqCst);
                let ms = limit.map(|d| d.as_millis() as u64).unwrap_or(0);
                // P0-2 配套:当且仅当 `retry.on` **显式**列出 `timeout`(空 on
                // 的"任意错误都重试"不含超时)且还有重试预算时,把超时降级成
                // 可重试错误流入下方重试臂;否则保持既有硬中断语义不变。
                if attempt <= times
                    && !retry_on.is_empty()
                    && retry_matches(&retry_on, ErrorKind::Timeout)
                {
                    Err(StepError::Timeout { ms })
                } else {
                    persist_step(
                        ctx,
                        StepPersist {
                            step_id: &step.id,
                            path: &path,
                            parent_path: parent_path.as_deref(),
                            depth,
                            idx,
                            state: "timeout",
                            attempt: attempt as i64,
                            input_hash: &input_hash,
                            output: None,
                            error: Some(format!("timed out after {ms}ms")),
                            started_at,
                            finished_at: Utc::now(),
                        },
                    )
                    .await;
                    return Err(ExecError::Timeout {
                        step: step.id.clone(),
                        ms,
                    });
                }
            }
            StepOutcome::Done(r) => r,
        };
        match exec_result {
            Ok(result) => {
                let finished_at = Utc::now();
                let _elapsed_ms = t0.elapsed().as_millis() as i64;
                ctx.record_step_output(&step.id, &result.output);
                // P1-4: a successful action may have resolved an element via the
                // `vision_locate` hook (the resolver has no `ctx` to book it
                // itself); drain + record that spend, attributed to this step.
                if let Some(provider) = ctx.ai_provider().cloned() {
                    persist_ai_usage(ctx, &provider.take_usage()).await;
                }
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
                )
                .await;
                return Ok(());
            }
            Err(e) if attempt <= times && retry_matches(&retry_on, e.kind()) => {
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
                )
                .await;
                tracing::warn!(
                    "step `{}` failed attempt {}/{}: {}",
                    step.id,
                    attempt,
                    times + 1,
                    error
                );
                let delay = compute_backoff(&backoff, initial_ms, attempt);
                // P1-1: race the backoff sleep against the run's cancel token so a
                // cancellation during the wait wins immediately instead of stalling
                // for the full backoff. Mirrors the Cancelled path above.
                let cancel = ctx.cancel_token();
                let cancelled = {
                    let sleep = tokio::time::sleep(std::time::Duration::from_millis(delay));
                    tokio::pin!(sleep);
                    tokio::select! {
                        biased;
                        _ = wait_cancel(&cancel) => true,
                        _ = &mut sleep => false,
                    }
                };
                if cancelled {
                    persist_step(
                        ctx,
                        StepPersist {
                            step_id: &step.id,
                            path: &path,
                            parent_path: parent_path.as_deref(),
                            depth,
                            idx,
                            state: "cancelled",
                            attempt: attempt as i64,
                            input_hash: &input_hash,
                            output: None,
                            error: Some("run cancelled".into()),
                            started_at,
                            finished_at: Utc::now(),
                        },
                    )
                    .await;
                    return Err(ExecError::Cancelled);
                }
                attempt += 1;
            }
            Err(e) => {
                let finished_at = Utc::now();
                let _elapsed_ms = t0.elapsed().as_millis() as i64;
                // P1-4: even on failure the action may have already incurred AI
                // spend via the `vision_locate`/`ocr` hooks before erroring. Drain
                // it here — mirroring the Ok branch — so it is attributed to this
                // failing step instead of bleeding into the next successful one.
                // Done before `try_ai_recovery` so recovery's own spend stays
                // separate.
                if let Some(provider) = ctx.ai_provider().cloned() {
                    persist_ai_usage(ctx, &provider.take_usage()).await;
                }
                let ai_mode = effective_ai_mode(ctx, step);
                let try_ai = matches!(ai_mode, AiMode::Fallback | AiMode::Primary)
                    && matches!(
                        e.kind(),
                        ErrorKind::SelectorNotFound | ErrorKind::ExtractFailed
                    );
                if try_ai {
                    match try_ai_recovery(ctx, step, &action, &action_input, &e).await {
                        Ok(Some((result, ai_trace))) => {
                            let now = Utc::now();
                            ctx.record_step_output(&step.id, &result.output);
                            ctx.record_step_ai(&step.id, ai_trace);
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
                            )
                            .await;
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
                )
                .await;
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
    // B1 (F-14): a raw-string `cond` is evaluated as a boolean expression via
    // `eval_cond`; a non-string `cond` falls back to plain truthiness. The raw
    // (pre-render) string is used so `{{ }}` and bare-expression forms both work.
    let raw_cond = step.with.get("cond").and_then(|c| c.as_str());
    let cond = rendered.get("cond").cloned().unwrap_or(Value::Null);
    let ai_mode = effective_ai_mode(ctx, step);
    let need_ai = matches!(ai_mode, AiMode::Primary)
        || (matches!(ai_mode, AiMode::Fallback) && cond.is_null());
    let mut ai_trace: Option<Value> = None;
    let truthy = if need_ai {
        match try_ai_decide(ctx, step).await {
            Ok(Some((decision, usage))) => {
                let mut trace = serde_json::json!({
                    "used": true,
                    "helper": "decide",
                    "model": effective_ai_model(ctx, step),
                    "confidence": decision.confidence,
                    "reasoning": decision.reasoning,
                });
                if let Some(agg) = ai_usage_aggregate(&usage) {
                    trace["usage"] = agg;
                }
                ai_trace = Some(trace);
                decision.result
            }
            _ => eval_cond(raw_cond, &cond, ctx)?,
        }
    } else {
        eval_cond(raw_cond, &cond, ctx)?
    };
    ctx.record_step_output(&step.id, &Value::Bool(truthy));
    if let Some(trace) = ai_trace {
        ctx.record_step_ai(&step.id, trace);
    }
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
    // P1-1 / F-20: a hard interrupt (cancel / timeout / breakpoint pause) inside
    // the taken branch unwinds the run — rethrow without recording this `if` as
    // a `failed` step.
    // 指令集 P1:break / continue 信号同理向上穿透 if 容器(交给最近的循环容器
    // 消化),不把这个 if 记成 failed。
    if matches!(&result, Err(e) if is_control_signal(e) || is_loop_signal(e)) {
        return result;
    }
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
    )
    .await;
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
        // 指令集 P1:break / continue 信号在最近的循环容器处消化——break 终止
        // 整个循环,continue 结束本轮、照常推进游标与轮次;二者都不算循环失败。
        match &result {
            Err(ExecError::Break { .. }) => {
                result = Ok(());
                break;
            }
            Err(ExecError::Continue { .. }) => {
                result = Ok(());
                i += stp;
                iters += 1;
                continue;
            }
            Err(_) => break,
            Ok(()) => {}
        }
        i += stp;
        iters += 1;
    }
    // P1-1 / F-20: a hard interrupt (cancel / timeout / breakpoint pause) inside
    // the loop body unwinds the run — rethrow without recording the loop as a
    // `failed` step (the offending leaf already persisted its own row, or a pause
    // is intentionally left un-persisted).
    if matches!(&result, Err(e) if is_control_signal(e)) {
        return result;
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
    )
    .await;
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
        // 指令集 P1:break / continue 信号在最近的循环容器处消化(语义同
        // run_for)——break 终止迭代,continue 进入下一个元素。
        match &result {
            Err(ExecError::Break { .. }) => {
                result = Ok(());
                break;
            }
            Err(ExecError::Continue { .. }) => {
                result = Ok(());
                iters += 1;
                continue;
            }
            Err(_) => break,
            Ok(()) => {}
        }
        iters += 1;
    }
    // P1-1 / F-20: a hard interrupt (cancel / timeout / breakpoint pause) inside
    // the loop body unwinds the run — rethrow without recording the loop as a
    // `failed` step (the offending leaf already persisted its own row, or a pause
    // is intentionally left un-persisted).
    if matches!(&result, Err(e) if is_control_signal(e)) {
        return result;
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
    )
    .await;
    result
}

async fn run_while(
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
    // 指令集 P1:cond 与 control.if 走同一求值器(F-14)——raw 字符串进表达式
    // 求值器(`{{ }}` 模板模式保留),非字符串字面量按真值判断。与 run_if 不同
    // 的是 cond 每轮都要基于最新上下文重新求值,所以这里只取 raw,渲染在轮内做。
    if step.with.get("cond").is_none() {
        return Err(ExecError::Other(anyhow::anyhow!(
            "control.while requires `cond`"
        )));
    }
    let raw_cond = step.with.get("cond").and_then(|c| c.as_str());
    // 防呆死循环:默认 1000 轮上限;到达上限且 cond 仍为真即报错。
    let max_iterations = rendered
        .get("max_iterations")
        .and_then(Value::as_u64)
        .unwrap_or(1000);
    let body = step
        .do_
        .as_ref()
        .ok_or_else(|| ExecError::Other(anyhow::anyhow!("control.while requires `do:` block")))?;

    let mut iters = 0u64;
    let mut result = Ok(());
    loop {
        let truthy = match raw_cond {
            Some(s) => lumo_dsl::eval_predicate(s, &ctx.template_ctx())?,
            None => {
                // 非字符串 cond(如 YAML 字面量 bool/数字):每轮重渲染再取真值。
                let re = render_value_inline(ctx, &step.with)?;
                is_truthy(re.get("cond").unwrap_or(&Value::Null))
            }
        };
        if !truthy {
            break;
        }
        if iters >= max_iterations {
            result = Err(ExecError::Other(anyhow::anyhow!(
                "control.while `{}` hit max_iterations={max_iterations} with cond still true — \
                 疑似死循环:请修正退出条件,或显式调大 `max_iterations`",
                step.id
            )));
            break;
        }
        // 循环绑定:`index` 为轮次(从 0 开始),与 for / for_each 对齐。
        ctx.push_binding("index", Value::from(iters as i64));
        result = run_block_boxed(ctx, body, Some(format!("{path}[{iters}]")), depth + 1).await;
        ctx.clear_binding("index");
        // 指令集 P1:break / continue 信号在最近的循环容器处消化。
        match &result {
            Err(ExecError::Break { .. }) => {
                result = Ok(());
                break;
            }
            Err(ExecError::Continue { .. }) => {
                result = Ok(());
                iters += 1;
                continue;
            }
            Err(_) => break,
            Ok(()) => {}
        }
        iters += 1;
    }
    // P1-1 / F-20:硬中断(cancel / timeout / 断点暂停)向上 unwind,不把循环
    // 记成 failed(与 run_for / run_for_each 同约定)。
    if matches!(&result, Err(e) if is_control_signal(e)) {
        return result;
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
    )
    .await;
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
        // P1-1 / F-20: a cancel / per-step timeout / breakpoint pause inside the
        // `do:` block is a hard interrupt, NOT a catchable failure. Rethrow it
        // before the catch/finally machinery so the run unwinds — otherwise a
        // `catch:` would silently "recover" from a timeout (defeating the hard
        // per-step ceiling) or swallow a breakpoint pause. The offending leaf
        // step already persisted its own cancelled/timeout row (a pause is
        // intentionally left un-persisted), so we also skip recording this `try`
        // as a step — matching how the pause unwinds through `execute_step`.
        Err(e) if is_control_signal(&e) => return Err(e),
        // 指令集 P1:break / continue 同样不可被 catch 捕获——但它是正常控制流
        // 而非硬中断,finally(清理语义)仍要执行,然后继续上抛给最近的循环
        // 容器消化。finally 自身失败时以 `?` 让 finally 的错误优先上抛。
        Err(e) if is_loop_signal(&e) => {
            if let Some(f) = &step.finally_ {
                run_block_boxed(ctx, f, Some(format!("{path}/finally")), depth + 1).await?;
            }
            return Err(e);
        }
        Err(e) => Some(e.to_string()),
    };
    let mut final_result = Ok(());
    // P1:`error` 是 try 作用域变量 —— 注入前保存旧值,try 结束后还原(嵌套
    // try 各还原各的),不再永久污染 vars 命名空间。模板路径保持 `vars.error`
    // 不变(随发行版的 examples/control-flow 即用此路径),catch 与 finally
    // 块内都可见。`Some(prev)` 表示注入过,需要还原;`None` 表示没 caught,
    // 从未注入。
    let prev_error = caught.as_ref().map(|err| {
        let prev = ctx.remove_var("error");
        ctx.set_var("error", Value::String(err.clone()));
        prev
    });
    if let Some(err) = &caught {
        if let Some(c) = &step.catch_ {
            final_result = run_block_boxed(ctx, c, Some(format!("{path}/catch")), depth + 1).await;
        } else {
            // No catch block: rethrow after finally.
            let mut error = err.clone();
            if let Some(f) = &step.finally_ {
                if let Err(e) =
                    run_block_boxed(ctx, f, Some(format!("{path}/finally")), depth + 1).await
                {
                    // P1:finally 失败不得覆盖根因 —— do 块的原始错误是调用方
                    // 排障的主线索,finally(清理)的错误追加在后,两者都可见。
                    error = format!("{error}; finally failed: {e}");
                }
            }
            restore_error_var(ctx, prev_error);
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
            )
            .await;
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
    restore_error_var(ctx, prev_error);
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
    )
    .await;
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
    // D-10: concurrent branch execution. Each branch runs on an *isolated*
    // fork of the context (P0-4) so concurrent branches can't corrupt each
    // other's vars / loop bindings; only the persisted `seq` counter is shared
    // (so step rows stay uniquely keyed). We use `futures::future::join_all` to
    // drive branches cooperatively on the current task — async concurrency for
    // I/O-bound work (browser, http, file) without needing inner state to be Send.
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
        )
        .await;
        return Ok(());
    }

    // Materialize per-branch forked state on the stack so the futures can borrow it.
    let mut branch_state: Vec<(StepCtx, Vec<Step>, String)> = branches
        .into_iter()
        .enumerate()
        .map(|(i, body)| (ctx.fork(), body, format!("{path}/branch[{i}]")))
        .collect();

    let futs: Vec<_> = branch_state
        .iter_mut()
        .map(|(c, body, branch_path)| {
            run_block_boxed(c, body.as_slice(), Some(branch_path.clone()), depth + 1)
        })
        .collect();

    let results = futures::future::join_all(futs).await;

    // P0-4: fold each branch's isolated state back into the parent, in branch
    // order for deterministic last-writer-wins on any colliding keys.
    for (branch_ctx, _, _) in &branch_state {
        ctx.merge_branch(branch_ctx);
    }

    // First failure wins; everything else still completes.
    let first_err = results.into_iter().find_map(|r| r.err());
    // 指令集 P1:parallel 分支是独立作用域——分支内逃逸出来的 break / continue
    // 不跨分支、也不能交给 parallel 外层的循环消化(否则一个分支会"替"外层
    // 循环做决定)。validate 已静态拦截,这里做运行期兜底:降级为普通错误,
    // 让 parallel 按 failed 记账并向上传播。
    let first_err = first_err.map(|e| {
        if is_loop_signal(&e) {
            ExecError::Other(anyhow::anyhow!(
                "{e}; control.parallel 分支是独立作用域,break/continue 不能跨出分支"
            ))
        } else {
            e
        }
    });
    // P1-1 / F-20: a hard interrupt (cancel / timeout / breakpoint pause) in any
    // branch unwinds the run — rethrow without recording the parallel block as a
    // normal `failed` step (the offending leaf already persisted its own row, or
    // a pause is intentionally left un-persisted). Branch state was already
    // merged back above, so the parent context still reflects what each branch
    // accomplished before the interrupt.
    if matches!(&first_err, Some(e) if is_control_signal(e)) {
        return Err(first_err.expect("matched Some"));
    }
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
    )
    .await;
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

/// P1-1 / F-20: whether an error is a control-flow *signal* — a cooperative
/// cancel, a per-step timeout, or a breakpoint pause — rather than an ordinary
/// step failure. These are *hard interrupts*: they must unwind the whole run,
/// so `control.try` must NOT catch them and `control.for`/`for_each`/`if`/
/// `parallel` must NOT record themselves as a normal `failed` step on the way
/// out. Ordinary failures (`ExecError::Step`, validation, etc.) are handled by
/// the catch/loop machinery as before.
fn is_control_signal(e: &ExecError) -> bool {
    matches!(
        e,
        ExecError::Cancelled | ExecError::Timeout { .. } | ExecError::Paused
    )
}

/// 指令集 P1:是否为循环控制信号(`control.break` / `control.continue`)。
/// 与硬中断([`is_control_signal`])一样不可被 `control.try` 捕获、不把途经的
/// 容器记成 failed;但语义不同——它向上 unwind 到**最近的循环容器**
/// (while / for / for_each)即被消化,不再外溢。逃逸出循环作用域
/// (parallel 分支边界 / flow 顶层)则按运行期错误兜底处理。
fn is_loop_signal(e: &ExecError) -> bool {
    matches!(e, ExecError::Break { .. } | ExecError::Continue { .. })
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
async fn persist_control_result(
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
    )
    .await;
}

async fn persist_step(ctx: &StepCtx, row: StepPersist<'_>) {
    ctx.mark_step_state(row.state);
    // A1: clone the (Arc-backed) repo handle out so the blocking SQLite write
    // can move onto tokio's blocking pool. `seq` is assigned and the owned row
    // built *here*, synchronously, before the hand-off — so rows keep their
    // execution-order seq even when concurrent (`control.parallel`) branches
    // race to persist; only the physical write completes off-thread.
    let Some(repo) = ctx.repo().cloned() else {
        return;
    };
    let step_id = row.step_id.to_string();
    let stored = StepRunRow {
        flow_run_id: ctx.run_id().to_string(),
        seq: ctx.next_step_seq(),
        path: row.path.to_string(),
        parent_path: row.parent_path.map(ToString::to_string),
        depth: row.depth,
        step_id: step_id.clone(),
        idx: row.idx,
        state: row.state.to_string(),
        attempt: row.attempt,
        input_hash: row.input_hash.to_vec(),
        output_json: row.output.cloned(),
        error: row.error,
        started_at: Some(row.started_at),
        finished_at: Some(row.finished_at),
        span_id: None,
        vars_json: Some(ctx.vars_snapshot()),
    };
    // The parking_lot Mutex<Connection> + SQLite write (which can block up to
    // `busy_timeout` under contention) runs on a blocking thread; the async
    // worker stays free to drive other steps / parallel branches meanwhile.
    match tokio::task::spawn_blocking(move || repo.insert_step(&stored)).await {
        Ok(Ok(())) => {}
        Ok(Err(e)) => tracing::warn!("persist_step `{step_id}`: {e}"),
        Err(e) => tracing::warn!("persist_step `{step_id}` join: {e}"),
    }
}

// ─── helpers ────────────────────────────────────────────────────────────────

/// P1:还原 `control.try` 注入的 try 作用域 `error` 变量。`prev` 的三态:
/// `None` = 本次没 caught、从未注入,什么都不动;`Some(Some(v))` = 注入前
/// 已有同名变量(嵌套 try 的外层 error),还原旧值;`Some(None)` = 注入前
/// 不存在,直接移除 —— try 结束后 vars 命名空间与进入前逐键一致。
fn restore_error_var(ctx: &StepCtx, prev: Option<Option<Value>>) {
    match prev {
        None => {}
        Some(Some(v)) => ctx.set_var("error", v),
        Some(None) => {
            ctx.remove_var("error");
        }
    }
}

/// B1 (F-14): evaluate a `control.if` condition. A raw-string `cond` goes
/// through the expression evaluator (operators + identifier paths, with `{{ }}`
/// template mode preserved); a non-string `cond` (literal bool/number/…) falls
/// back to plain truthiness.
fn eval_cond(raw: Option<&str>, rendered: &Value, ctx: &StepCtx) -> Result<bool, ExecError> {
    match raw {
        Some(s) => Ok(lumo_dsl::eval_predicate(s, &ctx.template_ctx())?),
        None => Ok(is_truthy(rendered)),
    }
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

/// B3 (F-16): whether a failed attempt is eligible for retry under the step's
/// `retry.on` filter. An empty `on` retries on any error (back-compat); a
/// non-empty `on` retries only when the error's kind name (snake_case, matching
/// [`ErrorKind::as_str`]) appears in the list (case- and whitespace-insensitive).
fn retry_matches(on: &[String], kind: ErrorKind) -> bool {
    on.is_empty()
        || on
            .iter()
            .any(|s| s.trim().eq_ignore_ascii_case(kind.as_str()))
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

/// P1-4: persist one `ai_calls` ledger row per usage record the provider
/// accumulated for the current step. Best-effort — a failed insert never blocks
/// the run, and a run with no repo (e.g. ad-hoc `lumo run` without persistence)
/// simply skips the write.
async fn persist_ai_usage(ctx: &StepCtx, usage: &[AiCallUsage]) {
    if usage.is_empty() {
        return;
    }
    let Some(repo) = ctx.repo().cloned() else {
        return;
    };
    let run_id = ctx.run_id().to_string();
    let step_id = ctx.current_step_id();
    // A1: own the usage records + ids, then write the `ai_calls` ledger rows on
    // the blocking pool so the inserts never block the async worker thread.
    let usage = usage.to_vec();
    let res = tokio::task::spawn_blocking(move || {
        for u in &usage {
            let _ = repo.record_ai_call(AiCallInsert {
                flow_run_id: &run_id,
                step_id: step_id.as_deref(),
                helper: &u.helper,
                provider: &u.provider,
                model: &u.model,
                input_tokens: u.input_tokens as i64,
                output_tokens: u.output_tokens as i64,
                latency_ms: u.latency_ms,
                cost_usd_micro: u.cost_usd_micro,
            });
        }
    })
    .await;
    if let Err(e) = res {
        tracing::warn!("persist_ai_usage join: {e}");
    }
}

/// P1-4: fold a step's accumulated AI usage into the `{input_tokens,
/// output_tokens, latency_ms, cost_usd_micro}` object attached to its `_ai`
/// trace, so `steps.<id>._ai.usage` and the Studio timeline can show token/cost
/// per hook. `None` when no metered calls were made.
fn ai_usage_aggregate(usage: &[AiCallUsage]) -> Option<Value> {
    if usage.is_empty() {
        return None;
    }
    let mut input_tokens = 0i64;
    let mut output_tokens = 0i64;
    let mut latency_ms = 0i64;
    let mut cost_usd_micro = 0i64;
    for u in usage {
        input_tokens += u.input_tokens as i64;
        output_tokens += u.output_tokens as i64;
        latency_ms += u.latency_ms;
        cost_usd_micro += u.cost_usd_micro;
    }
    Some(serde_json::json!({
        "input_tokens": input_tokens,
        "output_tokens": output_tokens,
        "latency_ms": latency_ms,
        "cost_usd_micro": cost_usd_micro,
    }))
}

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
/// `Ok(Some((result, ai_trace)))` if AI produced a usable outcome, where
/// `ai_trace` is the runtime-only `_ai` metadata recorded next to the step's
/// `result` (helper name, confidence, healed selector, …).
async fn try_ai_recovery(
    ctx: &mut StepCtx,
    step: &Step,
    action: &ActionRef,
    action_input: &Value,
    error: &StepError,
) -> Result<Option<(ActionResult, Value)>, StepError> {
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
            let mut usage = provider.take_usage();
            let Some(new_sel) = healed.css.clone().or_else(|| healed.xpath.clone()) else {
                // Heal still cost an LLM call even though it gave nothing usable.
                persist_ai_usage(ctx, &usage).await;
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
                obj.insert("selector".into(), Value::String(new_sel.clone()));
            }
            let result = action.execute(ctx, new_input).await?;
            // The healed re-run may itself trigger a vision hook; fold it in.
            usage.extend(provider.take_usage());
            persist_ai_usage(ctx, &usage).await;
            let mut trace = serde_json::json!({
                "used": true,
                "helper": "heal_selector",
                "model": model,
                "confidence": healed.confidence,
                "healed_selector": new_sel,
            });
            if let Some(agg) = ai_usage_aggregate(&usage) {
                trace["usage"] = agg;
            }
            Ok(Some((result, trace)))
        }
        ErrorKind::ExtractFailed => {
            let target = action_input
                .get("target")
                .and_then(Value::as_str)
                .map(str::to_string)
                .unwrap_or_else(|| prompt.clone());
            // Browser actions stash a page screenshot before surfacing
            // ExtractFailed; passing it makes extraction truly multimodal.
            let screenshot = ctx.take_screenshot();
            let used_image = screenshot.is_some();
            let value = provider
                .extract_visual(screenshot, &target, None, None, model.as_deref())
                .await?;
            let usage = provider.take_usage();
            persist_ai_usage(ctx, &usage).await;
            tracing::info!(
                "step `{}`: AI extract_visual produced value (image={})",
                step.id,
                used_image
            );
            let mut trace = serde_json::json!({
                "used": true,
                "helper": "extract_visual",
                "model": model,
                "multimodal": used_image,
            });
            if let Some(agg) = ai_usage_aggregate(&usage) {
                trace["usage"] = agg;
            }
            Ok(Some((ActionResult::from(value), trace)))
        }
        _ => Ok(None),
    }
}

/// Call AI decide for a control.if step. Returns `Ok(Some(decision))` on
/// success so the caller can branch on `decision.result` and record the
/// `_ai` trace (helper/model/confidence/reasoning).
async fn try_ai_decide(
    ctx: &mut StepCtx,
    step: &Step,
) -> Result<Option<(crate::ai_hook::Decision, Vec<AiCallUsage>)>, StepError> {
    let Some(provider) = ctx.ai_provider().cloned() else {
        return Ok(None);
    };
    let model = effective_ai_model(ctx, step);
    let prompt = effective_ai_prompt(step);
    let vars = ctx.vars_snapshot();
    let decision = provider.decide(&vars, &prompt, model.as_deref()).await?;
    let usage = provider.take_usage();
    persist_ai_usage(ctx, &usage).await;
    tracing::info!(
        "step `{}`: AI decide → {} (confidence {:.2}) — {}",
        step.id,
        decision.result,
        decision.confidence,
        decision.reasoning
    );
    Ok(Some((decision, usage)))
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
    let outcome = provider
        .diagnose(&step.id, &step.action, error, model.as_deref())
        .await;
    // diagnose has no `_ai` trace of its own, but it still spent budget — book it.
    persist_ai_usage(ctx, &provider.take_usage()).await;
    match outcome {
        Ok(s) if !s.trim().is_empty() => Some(s),
        Ok(_) => None,
        Err(e) => {
            tracing::warn!("diagnose for step `{}` failed: {}", step.id, e);
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn usage(input: u32, output: u32, latency_ms: i64, cost: i64) -> AiCallUsage {
        AiCallUsage {
            helper: "heal_selector".into(),
            provider: "p".into(),
            model: "m".into(),
            input_tokens: input,
            output_tokens: output,
            latency_ms,
            cost_usd_micro: cost,
        }
    }

    #[test]
    fn ai_usage_aggregate_is_none_without_calls() {
        assert!(ai_usage_aggregate(&[]).is_none());
    }

    #[test]
    fn ai_usage_aggregate_sums_tokens_latency_and_cost() {
        let agg = ai_usage_aggregate(&[usage(10, 20, 5, 100), usage(1, 2, 3, 7)])
            .expect("some usage folds into a trace object");
        assert_eq!(agg["input_tokens"], 11);
        assert_eq!(agg["output_tokens"], 22);
        assert_eq!(agg["latency_ms"], 8);
        assert_eq!(agg["cost_usd_micro"], 107);
    }

    #[test]
    fn retry_matches_empty_on_retries_any_kind() {
        assert!(retry_matches(&[], ErrorKind::Other));
        assert!(retry_matches(&[], ErrorKind::SelectorNotFound));
    }

    #[test]
    fn retry_matches_filters_by_kind_name() {
        let on = vec!["selector_not_found".to_string()];
        assert!(retry_matches(&on, ErrorKind::SelectorNotFound));
        assert!(!retry_matches(&on, ErrorKind::Other));
    }

    #[test]
    fn retry_matches_is_case_and_whitespace_insensitive() {
        let on = vec![" Other ".to_string()];
        assert!(retry_matches(&on, ErrorKind::Other));
    }

    /// P1-6:lumo-dsl 的 `RETRY_BACKOFF_KINDS` 白名单必须覆盖且仅覆盖
    /// `compute_backoff` 实际区分的策略(精确匹配 "exponential",其余按 fixed)。
    /// 漂移时在这里炸,而不是 validate 拒绝运行期支持的值(或放过会静默降级的值)。
    #[test]
    fn backoff_whitelist_matches_compute_backoff() {
        assert_eq!(lumo_dsl::RETRY_BACKOFF_KINDS, &["fixed", "exponential"]);
        // fixed:每次都等 initial_ms。
        assert_eq!(compute_backoff("fixed", 100, 1), 100);
        assert_eq!(compute_backoff("fixed", 100, 3), 100);
        // exponential:initial_ms * 2^(attempt-1)。
        assert_eq!(compute_backoff("exponential", 100, 1), 100);
        assert_eq!(compute_backoff("exponential", 100, 3), 400);
    }
}
