//! Per-step execution context.

use crate::action::ActionRef;
use crate::ai_hook::AiHookProvider;
use crate::error::{CapKind, StepError};
use crate::registry::ActionRegistry;
use lumo_dsl::{Capabilities, FlowAi, ResourceDecl, Step, TemplateCtx};
use lumo_storage::{ArtifactRow, Repo};
use parking_lot::Mutex;
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use ulid::Ulid;

/// P2-1:run 级日志缓冲的环形上限(条数)。超出丢最旧并累计 dropped 计数,
/// 防止长循环流程里 `ctx.log`(如 `control.log`)无界增长吃内存。
pub const LOG_BUFFER_MAX: usize = 1000;

/// P2-1:带上限的日志环。`log_buffer` 原为无界 `Vec<String>` 且只写不读;
/// 收敛为定容环 —— 超上限丢最旧、`dropped` 计数,宿主可据此提示"日志有截断"。
#[derive(Default)]
struct LogBuffer {
    entries: std::collections::VecDeque<String>,
    dropped: u64,
}

impl LogBuffer {
    fn push(&mut self, line: String) {
        if self.entries.len() >= LOG_BUFFER_MAX {
            self.entries.pop_front();
            self.dropped += 1;
        }
        self.entries.push_back(line);
    }
}

/// P2-7(进度事件,引擎侧 API):经 [`crate::FlowVm::with_on_step`] 注册的
/// 观察者收到的事件。
///
/// 发出时机:
///   * `StepStarted` —— 步骤通过取消/断点闸门后、`when` 求值前(控制流容器与
///     叶子动作都会发);
///   * `StepFinished` —— 步骤结果持久化点(`persist_step`),`state` 与
///     step_runs 行一致(ok/failed/skipped/retrying/cached/ai_healed/caught/
///     cancelled/timeout);无 repo 时事件照发,只是不落库;
///   * `Log` —— 每次 [`StepCtx::log`](如 `control.log` 动作)。
///
/// 边界:run 取消时,首个观察到取消的步骤只发 `StepFinished(cancelled)` 不发
/// `StepStarted`;断点暂停的步骤两者皆不发(该步未执行、也不落库)。
/// `control.break` / `control.continue` 是跳转信号,只发 `StepStarted`。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StepEvent {
    StepStarted {
        run_id: String,
        path: String,
        step_id: String,
        action: String,
    },
    StepFinished {
        run_id: String,
        path: String,
        step_id: String,
        state: String,
        attempt: i64,
        error: Option<String>,
    },
    Log {
        run_id: String,
        step_path: Option<String>,
        line: String,
    },
}

/// P2-7:on_step 观察者。引擎执行线程上同步调用,必须快速返回(重活请自行
/// 转队列/通道);`Arc` 包裹以便跨 `control.parallel` 分支 fork 共享。
pub type StepObserver = Arc<dyn Fn(&StepEvent) + Send + Sync>;

/// P1-3(vars_json 治理):vars 状态的全局单调版本号。取代"每 fork 一条计数
/// 线"的方案:任何一次 vars 变更(set_var / remove_var / merge_branch)都领取
/// 一个进程内唯一的新版本号,因此**版本号相等 ⇔ vars 状态确同**(fork 复制
/// 版本号时双方经 `Arc` 共享同一张 map)——持久化去重可以只比版本号。
static VARS_REV: AtomicU64 = AtomicU64::new(1);

fn next_vars_rev() -> u64 {
    VARS_REV.fetch_add(1, Ordering::Relaxed)
}

/// Cooperative cancellation handle for a run (P1-1). Clone it, hand one clone to
/// [`crate::FlowVm::with_cancel`], and call [`CancelToken::cancel`] from
/// anywhere to ask the VM to stop: it checks before each step and interrupts an
/// in-flight step via [`CancelToken::cancelled`].
#[derive(Clone)]
pub struct CancelToken {
    flag: Arc<AtomicBool>,
    tx: Arc<tokio::sync::watch::Sender<bool>>,
}

impl Default for CancelToken {
    fn default() -> Self {
        Self::new()
    }
}

impl CancelToken {
    pub fn new() -> Self {
        let (tx, _rx) = tokio::sync::watch::channel(false);
        Self {
            flag: Arc::new(AtomicBool::new(false)),
            tx: Arc::new(tx),
        }
    }

    /// Request cancellation. Idempotent; wakes anything awaiting [`Self::cancelled`].
    pub fn cancel(&self) {
        self.flag.store(true, Ordering::SeqCst);
        let _ = self.tx.send(true);
    }

    /// Whether cancellation has been requested.
    pub fn is_cancelled(&self) -> bool {
        self.flag.load(Ordering::SeqCst)
    }

    /// Resolves once cancelled. Race-free: re-checks the current value on
    /// subscribe, so a `cancel()` that landed before this call still wakes it.
    pub async fn cancelled(&self) {
        if self.is_cancelled() {
            return;
        }
        let mut rx = self.tx.subscribe();
        let _ = rx.wait_for(|&v| v).await;
    }
}

/// P0-2:步级协作中断信号,发给 `spawn_blocking` 闭包的取消句柄。
///
/// 背景:VM 的 `select!` 在超时/取消时只能 drop 动作 future,而动作内部
/// `spawn_blocking` 的阻塞任务不会随 future drop 停止 —— 超时后 db 事务照样
/// commit、文件照样写回(步骤状态却已记 timeout,重跑即重复写入)。本句柄把
/// "这次 attempt 已被引擎判死"传进阻塞闭包:引擎在 `select!` 返回超时/取消后
/// 翻转步级标志位;闭包在循环边界 / 写回(commit)前检查,命中即提前返回错误,
/// 副作用不落地。运行级取消(`CancelToken`)一并纳入,动作侧只需查本句柄。
#[derive(Clone, Default)]
pub struct StepInterrupt {
    /// 本次 attempt 的中断位。每个 attempt 换新 `Arc`(见
    /// [`StepCtx::arm_step_interrupt`]),旧 attempt 的孤儿阻塞任务持旧位、
    /// 保持已中断,不会被新 attempt 误"复活"。
    flag: Arc<AtomicBool>,
    /// 运行级取消令牌(与 VM 的 `CancelToken` 同源),为 `None` 时只看步级位。
    cancel: Option<CancelToken>,
}

impl StepInterrupt {
    /// Mark this attempt as dead. Action-internal deadlines use the same flag
    /// as the VM so orphaned blocking work observes the timeout at its next
    /// checkpoint and cannot commit a side effect after the action returned.
    pub fn interrupt(&self) {
        self.flag.store(true, Ordering::SeqCst);
    }

    /// 本次 attempt 是否已被引擎判定超时,或整个运行已被取消。
    pub fn is_interrupted(&self) -> bool {
        self.flag.load(Ordering::SeqCst)
            || self.cancel.as_ref().is_some_and(CancelToken::is_cancelled)
    }
}

/// F-20 (breakpoint debugging): cooperative pause handle for a run. Like
/// [`CancelToken`] it is `Clone` + `Arc`-backed, so every `fork()`ed
/// `control.parallel` branch shares the same `armed`/`paused`/`paused_at`
/// state — a breakpoint hit in one branch latches for the siblings too.
///
/// `breakpoints` are step *paths* to pause *before*; `step_mode` pauses before
/// every executing step (single-step). The VM consults [`Self::should_pause`]
/// for each step about to actually execute (past resume replay) and, when it
/// fires, [`Self::mark_paused`] latches a sticky flag so the rest of the run
/// short-circuits and unwinds.
/// 去掉路径每段末尾的 `[N]` 序号（循环迭代 `loop[3]`、并行分支 `branch[1]`），
/// 得到与 Studio 静态 step 链可比的形态。
fn normalize_debug_path(path: &str) -> String {
    path.split('/')
        .map(strip_index_suffix)
        .collect::<Vec<_>>()
        .join("/")
}

fn strip_index_suffix(segment: &str) -> &str {
    let Some(open) = segment.rfind('[') else {
        return segment;
    };
    match segment[open + 1..].strip_suffix(']') {
        Some(digits) if !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()) => {
            &segment[..open]
        }
        _ => segment,
    }
}

#[derive(Clone)]
pub struct DebugController {
    breakpoints: Arc<std::collections::HashSet<String>>,
    step_mode: bool,
    /// `false` on a resumed run so the first step actually executed (the one we
    /// paused on last time) "steps off" without immediately re-pausing; `true`
    /// on a fresh run so the first matching step can pause.
    armed: Arc<AtomicBool>,
    /// Sticky: a breakpoint has fired; every subsequent step short-circuits.
    paused: Arc<AtomicBool>,
    paused_at: Arc<Mutex<Option<String>>>,
}

impl DebugController {
    /// `armed` should be `true` for a fresh run (a breakpoint may fire on the
    /// very first step) and `false` for a resumed run (step off the step we
    /// paused on last time before pausing again).
    pub fn new(
        breakpoints: std::collections::HashSet<String>,
        step_mode: bool,
        armed: bool,
    ) -> Self {
        Self {
            breakpoints: Arc::new(breakpoints),
            step_mode,
            armed: Arc::new(AtomicBool::new(armed)),
            paused: Arc::new(AtomicBool::new(false)),
            paused_at: Arc::new(Mutex::new(None)),
        }
    }

    /// Whether a breakpoint has already fired this run (sticky).
    pub fn is_paused(&self) -> bool {
        self.paused.load(Ordering::SeqCst)
    }

    /// The step path the run paused before, once a breakpoint has fired.
    pub fn paused_at(&self) -> Option<String> {
        self.paused_at.lock().clone()
    }

    /// Decide whether to pause *before* a step (identified by `path`) that is
    /// about to actually execute. The first call on a resumed run arms the
    /// controller and returns `false` (steps off the prior breakpoint); after
    /// that it pauses in single-step mode or when `path` is a breakpoint.
    fn should_pause(&self, path: &str) -> bool {
        if !self.armed.swap(true, Ordering::SeqCst) {
            return false; // resume step-off: let the step we paused on last run proceed
        }
        self.step_mode || self.matches_breakpoint(path)
    }

    /// 断点命中策略（宽进）：① 运行期路径完全相等；② 叶子 step id 相等
    /// （Studio 断点存的是裸 id）；③ 双方每段去掉 `[N]` 迭代/分支序号后
    /// 相等（`loop[3]/click` ↔ `loop/click`）。序号剥离会让同名步骤在不同
    /// 迭代/分支上都命中——调试工具宁可多停，不可从不停。
    fn matches_breakpoint(&self, path: &str) -> bool {
        if self.breakpoints.contains(path) {
            return true;
        }
        if let Some(leaf) = path.rsplit('/').next() {
            if self.breakpoints.contains(leaf) {
                return true;
            }
        }
        let normalized = normalize_debug_path(path);
        self.breakpoints
            .iter()
            .any(|breakpoint| normalize_debug_path(breakpoint) == normalized)
    }

    /// Latch the sticky pause at `path` (first writer wins).
    fn mark_paused(&self, path: &str) {        self.paused.store(true, Ordering::SeqCst);
        let mut at = self.paused_at.lock();
        if at.is_none() {
            *at = Some(path.to_string());
        }
    }
}

/// F-13 (durable resume): a prior run's reusable leaf-step outputs, keyed by
/// step path. The VM builds this from `step_runs` when a run is resumed and
/// consults it per leaf step, so an already-completed step whose rendered-input
/// hash is unchanged is replayed from its persisted output instead of re-run.
#[derive(Default)]
pub struct ResumeMemo {
    entries: std::collections::HashMap<String, ResumeEntry>,
}

struct ResumeEntry {
    input_hash: Vec<u8>,
    output: Value,
}

impl ResumeMemo {
    /// Record a completed step's output. Called in `seq` order while loading a
    /// prior run, so a later successful attempt for a path overrides an earlier.
    pub fn record(&mut self, path: String, input_hash: Vec<u8>, output: Value) {
        self.entries
            .insert(path, ResumeEntry { input_hash, output });
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// The replayable output for `path`, iff a recorded entry exists *and* its
    /// input hash matches — so a step whose (rendered) inputs changed since the
    /// prior run is re-executed rather than replayed with stale output.
    fn hit(&self, path: &str, input_hash: &[u8]) -> Option<&Value> {
        self.entries
            .get(path)
            .filter(|e| e.input_hash == input_hash)
            .map(|e| &e.output)
    }
}

#[derive(Clone)]
pub struct StepCtx {
    pub run_id: String,
    pub flow_id: String,
    pub registry: ActionRegistry,
    capabilities: Capabilities,
    vault_names: Vec<String>,
    /// T3: flow-level resource declarations (`spec.resources`), seeded by the VM
    /// from `FlowVm::run` via [`StepCtx::with_resources`]. Held as a thin `Arc`'d
    /// map for ref validation ([`StepCtx::resource_decl`]) and config lookup
    /// only — the *live* handles live in each action family's own
    /// per-`(run_id, name)` storage, never here (keeps `!Send` handles like a CDP
    /// `Page` out of the fork-able context).
    resources: Arc<BTreeMap<String, ResourceDecl>>,
    repo: Option<Repo>,
    /// Root directory for blob artifacts (screenshots, DOM snapshots, ...).
    /// When unset, `attach_artifact` is a no-op so headless smoke tests don't
    /// have to mount a temp dir.
    artifacts_dir: Option<PathBuf>,
    ai_provider: Option<Arc<dyn AiHookProvider>>,
    flow_ai: Option<FlowAi>,
    /// P1-3: optional age identity for decrypting `${{ vault.* }}` from the
    /// encrypted store when an env var isn't set. `None` ⇒ env-only (graceful
    /// degrade when no identity file exists).
    vault_identity: Option<Arc<lumo_storage::VaultIdentity>>,
    /// P0-5: how many `skill.invoke` levels deep this context is. The skill
    /// action rejects invocations past a fixed ceiling to stop runaway / cyclic
    /// recursion (stack overflow / OOM).
    skill_depth: u32,
    /// P1-1: cooperative cancellation handle, seeded by the VM from
    /// `FlowVm::with_cancel`. Checked before each step and used to interrupt an
    /// in-flight step.
    cancel: Option<CancelToken>,
    /// P1-1: per-step timeout, seeded by the VM from `FlowVm::with_step_timeout`.
    step_timeout: Option<Duration>,
    /// Persisted step sequence counter. Shared (via `Arc`) across parallel
    /// branch forks so the `step_runs (flow_run_id, seq)` primary key stays
    /// unique even when branches persist concurrently (P0-4).
    seq: Arc<AtomicI64>,
    /// F-13: prior-run step outputs to replay on resume. Shared (`Arc`) across
    /// parallel branch forks so resumed branches reuse outputs too. `None` for a
    /// normal (non-resumed) run.
    resume_memo: Option<Arc<ResumeMemo>>,
    /// F-20: breakpoint / single-step debug controller, seeded by the VM from
    /// `FlowVm::with_breakpoints` / `with_step_mode`. `Arc`-backed internally so
    /// the pause state is shared across parallel branch forks. `None` ⇒ a normal
    /// (non-debugged) run with zero per-step overhead.
    debug: Option<DebugController>,
    /// P1（人机交互）：宿主注入的 human prompt 通道，`human.*` 动作经
    /// [`StepCtx::human_prompter`] 取用。`None` ⇒ 宿主不支持人机交互，
    /// 动作立刻报错而非静默挂起。
    human: Option<Arc<dyn crate::human::HumanPrompter>>,
    /// P2-7:宿主注册的进度观察者(由 VM 从 `FlowVm::with_on_step` 播种)。
    /// fork 共享同一 `Arc`,parallel 分支的事件汇入同一通道。`None` ⇒ 零开销。
    on_step: Option<StepObserver>,
    /// P1-3(vars_json 治理):持久化 vars 去重游标 —— 记录最近一条已持久化
    /// step 行的 vars 版本号。经 `Arc` 跨 parallel 分支 fork 共享(与 `seq`
    /// 同理):seq 分配与去重判定在 [`StepCtx::next_seq_with_vars`] 的同一临界
    /// 区完成,保证"NULL = 与前一条 seq 行的 vars 相同"即使分支交错持久化也
    /// 成立。
    vars_persist: Arc<Mutex<Option<u64>>>,
    inner: Arc<Mutex<CtxInner>>,
}

struct CtxInner {
    inputs: Arc<Value>,
    steps: Arc<Map<String, Value>>,
    vars: Arc<Map<String, Value>>,
    bindings: Arc<Map<String, Value>>,
    /// P1-3(全局性能税):`template_ctx()` 的代数缓存。steps/vars/bindings 任一
    /// 变更点(record_step_output / set_var / push_binding / merge_branch …)把它
    /// 置 `None` 即"代数 +1";同代数内(vm.rs 每步 when 判断、输入渲染、控制流
    /// inline 渲染共 2-3 次)直接克隆缓存——`TemplateCtx` 字段全是 `Arc`,克隆是
    /// O(1),且克隆间共享内部的 minijinja scope 缓存,重复 serialize 一并消除。
    tpl_cache: Option<TemplateCtx>,
    /// P1-3:vars 状态版本号(全局唯一,见 [`next_vars_rev`])。set_var /
    /// remove_var / merge_branch 换新;fork 原样复制(此刻 vars 经 Arc 共享,
    /// 状态确同)。持久化侧只比版本号即可判断"vars 自上一条持久化行以来没变"。
    vars_rev: u64,
    log_buffer: LogBuffer,
    stats: RunStats,
    /// Step id currently being executed. Set by the VM right before
    /// `Action::execute`; lets actions (e.g. `ai.chat`) attribute cost rows
    /// to the right step without changing the trait signature.
    current_step_id: Option<String>,
    /// Full nested path (e.g. `loop/item.3/click`) of the current step.
    /// Used by `attach_artifact` so X-07 time-travel rows line up against
    /// the step_runs path column exactly.
    current_step_path: Option<String>,
    /// T3: the resource name (`spec.resources.<name>`) the current step binds to
    /// via `Step.resource`, set by the VM right before `Action::execute` (like
    /// `current_step_id`). `None` ⇒ the step opens its own connection as before.
    current_resource: Option<String>,
    /// Last page screenshot stashed by a browser action right before it
    /// surfaced an `ExtractFailed` error. The VM's `extract_visual` AI hook
    /// picks it up so the LLM can *see* the page (true multimodal extraction)
    /// instead of falling back to text-only. Cleared after each consume.
    last_screenshot: Option<bytes::Bytes>,
    /// P0-2:当前 leaf attempt 的步级中断位(见 [`StepInterrupt`])。VM 每个
    /// attempt 前经 `arm_step_interrupt` 换新;放 `CtxInner` 是因为同一上下文
    /// 同一时刻只有一个 leaf 在跑(control 容器不进 `select!` 路径),而
    /// parallel 分支各自 fork 出独立的 `CtxInner`,互不串扰。
    step_interrupt: Arc<AtomicBool>,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct RunStats {
    pub executed: usize,
    pub ok: usize,
    pub failed: usize,
    pub skipped: usize,
    pub retried: usize,
    pub caught: usize,
}

impl StepCtx {
    pub fn new(
        run_id: String,
        flow_id: String,
        registry: ActionRegistry,
        repo: Option<Repo>,
        inputs: Value,
        capabilities: Capabilities,
        vault_names: Vec<String>,
    ) -> Self {
        Self {
            run_id,
            flow_id,
            registry,
            capabilities,
            vault_names,
            resources: Arc::new(BTreeMap::new()),
            repo,
            artifacts_dir: None,
            ai_provider: None,
            flow_ai: None,
            vault_identity: None,
            skill_depth: 0,
            cancel: None,
            step_timeout: None,
            seq: Arc::new(AtomicI64::new(0)),
            resume_memo: None,
            debug: None,
            human: None,
            on_step: None,
            vars_persist: Arc::new(Mutex::new(None)),
            inner: Arc::new(Mutex::new(CtxInner {
                inputs: Arc::new(inputs),
                steps: Arc::new(Map::new()),
                vars: Arc::new(Map::new()),
                bindings: Arc::new(Map::new()),
                tpl_cache: None,
                vars_rev: next_vars_rev(),
                log_buffer: LogBuffer::default(),
                stats: RunStats::default(),
                current_step_id: None,
                current_step_path: None,
                current_resource: None,
                last_screenshot: None,
                step_interrupt: Arc::new(AtomicBool::new(false)),
            })),
        }
    }

    /// Attach the AI hook provider and flow-level AI policy. Called by
    /// `FlowVm::run` when both are configured; otherwise AI hooks stay off
    /// regardless of step-level `ai:` blocks.
    pub fn with_ai(
        mut self,
        provider: Option<Arc<dyn AiHookProvider>>,
        flow_ai: Option<FlowAi>,
    ) -> Self {
        self.ai_provider = provider;
        self.flow_ai = flow_ai;
        self
    }

    /// Attach the age identity used to decrypt `${{ vault.* }}` from the
    /// encrypted store (P1-3). Seeded by the VM from `FlowVm::with_vault`.
    /// `None` keeps resolution env-only.
    pub fn with_vault(mut self, identity: Option<Arc<lumo_storage::VaultIdentity>>) -> Self {
        self.vault_identity = identity;
        self
    }

    /// 架构 P0-1:vault 解密身份,供子流程 VM（[`crate::FlowVm::child_of`]）
    /// 继承 —— 子流程里 `${{ vault.* }}` 不因裸建 VM 断链。`None` ⇒ env-only。
    pub fn vault_identity(&self) -> Option<Arc<lumo_storage::VaultIdentity>> {
        self.vault_identity.clone()
    }

    /// T3: seed the flow-level resource declarations (`spec.resources`) so steps
    /// can validate their `resource:` ref and look up its kind/profile/config.
    /// Seeded by the VM from `FlowVm::run`. Absent ⇒ no declared resources.
    pub fn with_resources(mut self, resources: BTreeMap<String, ResourceDecl>) -> Self {
        self.resources = Arc::new(resources);
        self
    }

    /// T3: whether a resource named `name` is declared in `spec.resources`.
    pub fn has_resource(&self, name: &str) -> bool {
        self.resources.contains_key(name)
    }

    /// T3: the declaration for resource `name`, or an error if it isn't declared
    /// in `spec.resources`. Mirrors the `${{ vault.* }}` ref check: a reference
    /// to an undeclared resource is rejected at resolve time. Returns a clone so
    /// an action family can read `kind`/`profile`/`config` without holding a lock
    /// on the context.
    pub fn resource_decl(&self, name: &str) -> Result<ResourceDecl, StepError> {
        self.resources.get(name).cloned().ok_or_else(|| {
            StepError::msg(format!(
                "resource `{name}` is not declared in spec.resources"
            ))
        })
    }

    /// T3: stash the resource name the step about to run binds to
    /// (`Step.resource`) so a resource-aware action can resolve its shared,
    /// run-scoped handle. Set by the VM right before `Action::execute`, mirroring
    /// [`set_current_step`](Self::set_current_step). `None` clears it.
    pub fn set_current_resource(&self, name: Option<&str>) {
        self.inner.lock().current_resource = name.map(str::to_string);
    }

    /// T3: the resource name the current step binds to, if any.
    pub fn current_resource(&self) -> Option<String> {
        self.inner.lock().current_resource.clone()
    }

    pub fn ai_provider(&self) -> Option<&Arc<dyn AiHookProvider>> {
        self.ai_provider.as_ref()
    }

    pub fn flow_ai(&self) -> Option<&FlowAi> {
        self.flow_ai.as_ref()
    }

    /// The capability sandbox in force for this context. Used by `skill.invoke`
    /// to clamp a sub-flow's grants to the caller's (P0-5).
    pub fn capabilities(&self) -> &Capabilities {
        &self.capabilities
    }

    /// Current `skill.invoke` nesting depth (0 at the top level).
    pub fn skill_depth(&self) -> u32 {
        self.skill_depth
    }

    /// Seed the `skill.invoke` nesting depth (set by the VM from `FlowVm`).
    pub fn with_skill_depth(mut self, depth: u32) -> Self {
        self.skill_depth = depth;
        self
    }

    /// Seed the run's cancellation handle (P1-1, set by the VM from `FlowVm`).
    pub fn with_cancel(mut self, cancel: Option<CancelToken>) -> Self {
        self.cancel = cancel;
        self
    }

    /// Seed the per-step timeout (P1-1, set by the VM from `FlowVm`).
    pub fn with_step_timeout(mut self, timeout: Option<Duration>) -> Self {
        self.step_timeout = timeout;
        self
    }

    /// F-13: seed the prior-run replay memo so completed steps are reused
    /// instead of re-run. `None` ⇒ a normal (non-resumed) run.
    pub fn with_resume_memo(mut self, memo: Option<Arc<ResumeMemo>>) -> Self {
        self.resume_memo = memo;
        self
    }

    /// F-13: the replayable output for this step path on resume, if any — set
    /// when the prior run completed this exact step with the same input hash.
    pub fn resume_hit(&self, path: &str, input_hash: &[u8]) -> Option<Value> {
        self.resume_memo
            .as_ref()
            .and_then(|m| m.hit(path, input_hash))
            .cloned()
    }

    /// F-20: seed the breakpoint / single-step debug controller (set by the VM
    /// from `FlowVm`). `None` ⇒ a normal (non-debugged) run.
    pub fn with_debug(mut self, debug: Option<DebugController>) -> Self {
        self.debug = debug;
        self
    }

    /// P1（人机交互）：注入宿主的 [`crate::human::HumanPrompter`]（由 VM 从
    /// `FlowVm::with_human_prompter` 播种）。`None` ⇒ 宿主不支持人机交互。
    pub fn with_human_prompter(
        mut self,
        prompter: Option<Arc<dyn crate::human::HumanPrompter>>,
    ) -> Self {
        self.human = prompter;
        self
    }

    /// P1（人机交互）：宿主注入的 human prompt 通道（克隆出 `Arc`，等待期间
    /// 不持有上下文借用）。`None` ⇒ 动作侧报"宿主不支持人机交互"。
    pub fn human_prompter(&self) -> Option<Arc<dyn crate::human::HumanPrompter>> {
        self.human.clone()
    }

    /// P2-7:注册宿主进度观察者(由 VM 从 `FlowVm::with_on_step` 播种)。
    /// `None` ⇒ 不发事件,行为与旧版逐字节一致。
    pub fn with_on_step(mut self, obs: Option<StepObserver>) -> Self {
        self.on_step = obs;
        self
    }

    /// P2-7:当前注册的进度观察者,供子流程 VM([`crate::FlowVm::child_of`])
    /// 继承 —— 子流程步骤事件汇入同一通道(事件带各自 run_id,宿主可分流)。
    pub fn on_step(&self) -> Option<StepObserver> {
        self.on_step.clone()
    }

    /// P2-7:向观察者投递一条进度事件。未注册时闭包不执行、零分配(向后
    /// 兼容:不设回调行为不变)。观察者在引擎线程同步调用,期间不持有任何
    /// ctx 内部锁,回调里可安全再调 ctx 方法。
    pub fn emit_event(&self, make: impl FnOnce() -> StepEvent) {
        if let Some(obs) = &self.on_step {
            obs(&make());
        }
    }

    /// F-20: whether a breakpoint has already fired (sticky). The VM checks this
    /// before each step and short-circuits the rest of the run once it's set, so
    /// the pause unwinds cleanly even through `control.try` / `control.parallel`.
    pub fn is_debug_paused(&self) -> bool {
        self.debug.as_ref().is_some_and(DebugController::is_paused)
    }

    /// F-20: should the VM pause *before* executing the step at `path`? Arms the
    /// controller on the first executing step of a resumed run (the step-off).
    pub fn debug_should_pause(&self, path: &str) -> bool {
        self.debug.as_ref().is_some_and(|d| d.should_pause(path))
    }

    /// F-20: latch the sticky pause at `path` (called right before returning the
    /// pause error from the step about to execute).
    pub fn debug_mark_paused(&self, path: &str) {
        if let Some(d) = &self.debug {
            d.mark_paused(path);
        }
    }

    /// F-20: the step path the run paused before, if a breakpoint fired.
    pub fn debug_paused_at(&self) -> Option<String> {
        self.debug.as_ref().and_then(DebugController::paused_at)
    }

    /// Whether the run has been cancelled. The VM checks this before each step.
    pub fn is_cancelled(&self) -> bool {
        self.cancel.as_ref().is_some_and(CancelToken::is_cancelled)
    }

    /// A clone of the run's cancel token, if one is attached. Cloned out so the
    /// VM can await `cancelled()` without holding a borrow on the context.
    pub fn cancel_token(&self) -> Option<CancelToken> {
        self.cancel.clone()
    }

    /// The per-step timeout in force, if any.
    pub fn step_timeout(&self) -> Option<Duration> {
        self.step_timeout
    }

    /// P0-2:为即将开始的 leaf attempt 装填一个全新的步级中断位,返回引擎侧
    /// 句柄。换新而非复位:上一个 attempt 超时后遗留的孤儿阻塞任务还持着旧位,
    /// 原地复位会把它误"复活";换新让旧任务保持已中断、新 attempt 干净起步。
    /// 由 VM 在每个 attempt 前调用,`select!` 判定超时/取消后对返回句柄置 true。
    pub fn arm_step_interrupt(&self) -> Arc<AtomicBool> {
        let fresh = Arc::new(AtomicBool::new(false));
        self.inner.lock().step_interrupt = fresh.clone();
        fresh
    }

    /// P0-2:动作侧取当前步骤的协作中断信号,克隆进 `spawn_blocking` 闭包,
    /// 在循环边界 / 写回(commit)前调用 [`StepInterrupt::is_interrupted`]。
    pub fn step_interrupt(&self) -> StepInterrupt {
        StepInterrupt {
            flag: self.inner.lock().step_interrupt.clone(),
            cancel: self.cancel.clone(),
        }
    }

    /// 当前状态的模板上下文快照。P1-3:命名空间以 `Arc` 共享(零深拷贝),并按
    /// "代数"缓存——自上次 steps/vars/bindings 变更以来若已构造过快照,直接克隆
    /// 复用(含其内部已 serialize 好的 minijinja scope)。`env` 快照随缓存重建
    /// 刷新,粒度为每个状态变更点(≥ 每步一次),与旧行为在步骤边界上等价。
    pub fn template_ctx(&self) -> TemplateCtx {
        let mut g = self.inner.lock();
        if let Some(tc) = &g.tpl_cache {
            return tc.clone(); // 全 Arc 字段,克隆是 O(1) 引用计数操作
        }
        let tc = TemplateCtx {
            inputs: g.inputs.clone(),
            steps: g.steps.clone(),
            vars: g.vars.clone(),
            bindings: g.bindings.clone(),
            env: Arc::new(env_snapshot()),
            vault: self.vault_names.clone(),
            ..Default::default()
        };
        g.tpl_cache = Some(tc.clone());
        tc
    }

    pub fn record_step_output(&self, step_id: &str, output: &Value) {
        let mut g = self.inner.lock();
        g.tpl_cache = None; // steps 变更 ⇒ 模板上下文代数 +1
        Arc::make_mut(&mut g.steps)
            .insert(step_id.to_string(), serde_json::json!({ "result": output }));
    }

    /// Attach an `_ai` trace next to a step's `result` (runtime-only; never
    /// written back to YAML). Lets `steps.<id>._ai` and the Studio timeline
    /// surface a purple "AI heal" badge while `steps.<id>.result` keeps its
    /// reference contract. No-op if the step has no recorded output yet.
    pub fn record_step_ai(&self, step_id: &str, ai: Value) {
        let mut g = self.inner.lock();
        g.tpl_cache = None; // steps 内容变更 ⇒ 失效
        if let Some(Value::Object(entry)) = Arc::make_mut(&mut g.steps).get_mut(step_id) {
            entry.insert("_ai".to_string(), ai);
        }
    }

    /// Stash a page screenshot so a subsequent AI hook (e.g. `extract_visual`)
    /// can pass it to the vision model. Browser actions call this right before
    /// returning an `ExtractFailed` error.
    pub fn stash_screenshot(&self, png: bytes::Bytes) {
        self.inner.lock().last_screenshot = Some(png);
    }

    /// Take (and clear) the most recently stashed screenshot, if any.
    pub fn take_screenshot(&self) -> Option<bytes::Bytes> {
        self.inner.lock().last_screenshot.take()
    }

    pub fn set_var(&self, key: &str, value: Value) {
        let mut g = self.inner.lock();
        g.tpl_cache = None; // vars 变更 ⇒ 失效,下次渲染必须看到新值
        g.vars_rev = next_vars_rev(); // P1-3:vars 状态换代
        Arc::make_mut(&mut g.vars).insert(key.to_string(), value);
    }

    /// 移除一个 vars 变量并返回旧值(无则 `None`)。P1:供 `control.try` 把
    /// `error` 做成 try 作用域 —— 进 catch/finally 前保存旧值、结束后还原,
    /// 不再永久污染 vars 命名空间(嵌套 try 各还原各的)。
    pub fn remove_var(&self, key: &str) -> Option<Value> {
        let mut g = self.inner.lock();
        g.tpl_cache = None; // vars 变更 ⇒ 失效,下次渲染必须看到新值
        g.vars_rev = next_vars_rev(); // P1-3:vars 状态换代(空移除也保守换代)
        Arc::make_mut(&mut g.vars).remove(key)
    }

    /// vars 命名空间的深拷贝快照。P1-3:锁内只克隆 `Arc`(O(1)),真正的
    /// 深拷贝在锁外做 —— 大 vars 不再让持锁方(含 parallel 分支)排队。
    pub fn vars_snapshot(&self) -> Value {
        let vars = self.inner.lock().vars.clone();
        Value::Object((*vars).clone())
    }

    pub fn outputs_snapshot(&self) -> Value {
        Value::Object((*self.inner.lock().steps).clone())
    }

    pub fn push_binding(&self, key: &str, value: Value) {
        let mut g = self.inner.lock();
        g.tpl_cache = None; // bindings 变更 ⇒ 失效
        Arc::make_mut(&mut g.bindings).insert(key.into(), value);
    }

    pub fn clear_binding(&self, key: &str) {
        let mut g = self.inner.lock();
        g.tpl_cache = None; // bindings 变更 ⇒ 失效
        Arc::make_mut(&mut g.bindings).remove(key);
    }

    pub fn log(&self, line: impl Into<String>) {
        let line = line.into();
        if let Some(obs) = &self.on_step {
            // P2-7:log 行进宿主进度通道(如 `control.log`)。锁先释放再回调,
            // 观察者内可安全再调 ctx 方法(parking_lot Mutex 不可重入)。
            let step_path = {
                let mut g = self.inner.lock();
                g.log_buffer.push(line.clone());
                g.current_step_path.clone()
            };
            obs(&StepEvent::Log {
                run_id: self.run_id.clone(),
                step_path,
                line,
            });
        } else {
            self.inner.lock().log_buffer.push(line);
        }
    }

    /// P2-1:当前日志环快照(最旧 → 最新,至多 [`LOG_BUFFER_MAX`] 条)。
    pub fn logs(&self) -> Vec<String> {
        self.inner
            .lock()
            .log_buffer
            .entries
            .iter()
            .cloned()
            .collect()
    }

    /// P2-1:因环形上限被丢弃的最旧日志条数(含并入的 parallel 分支)。
    pub fn logs_dropped(&self) -> u64 {
        self.inner.lock().log_buffer.dropped
    }

    pub fn lookup_action(&self, id: &str) -> Option<ActionRef> {
        self.registry.get(id)
    }

    pub fn next_step_seq(&self) -> i64 {
        self.seq.fetch_add(1, Ordering::Relaxed)
    }

    /// P1-3(vars_json 治理):一次临界区里领取持久化 seq 并做 vars 快照去重
    /// 判定。返回 `(seq, Some(vars))` ⇒ 本行写全量快照(锁内只克隆 `Arc`,
    /// 深拷贝/截断/序列化全部移到锁外的阻塞线程);`(seq, None)` ⇒ vars 自
    /// 上一条持久化行(全局按 seq)以来未变,落 NULL,读取侧
    /// (`Repo::list_steps`)向前回溯补齐。seq 分配与判定同锁,保证
    /// "NULL = 与前一条 seq 行相同"在 parallel 分支交错持久化时依然成立
    /// (交错处版本号不同,自动退回写全量)。
    pub fn next_seq_with_vars(&self) -> (i64, Option<Arc<Map<String, Value>>>) {
        let mut last = self.vars_persist.lock();
        let seq = self.seq.fetch_add(1, Ordering::Relaxed);
        let g = self.inner.lock();
        let rev = g.vars_rev;
        let vars = if *last == Some(rev) {
            None
        } else {
            Some(g.vars.clone())
        };
        *last = Some(rev);
        (seq, vars)
    }

    /// P0-4: produce an isolated child context for a `control.parallel` branch.
    /// The fork gets a deep copy of the current inputs/vars/bindings/steps so
    /// concurrent branches can't race on shared mutable state, while the
    /// registry/repo/capabilities/AI provider and the persisted `seq` counter
    /// are shared (read-only or append-only). Call [`StepCtx::merge_branch`]
    /// afterward to fold the branch's results back.
    pub fn fork(&self) -> StepCtx {
        let g = self.inner.lock();
        StepCtx {
            run_id: self.run_id.clone(),
            flow_id: self.flow_id.clone(),
            registry: self.registry.clone(),
            capabilities: self.capabilities.clone(),
            vault_names: self.vault_names.clone(),
            resources: self.resources.clone(),
            repo: self.repo.clone(),
            artifacts_dir: self.artifacts_dir.clone(),
            ai_provider: self.ai_provider.clone(),
            flow_ai: self.flow_ai.clone(),
            vault_identity: self.vault_identity.clone(),
            skill_depth: self.skill_depth,
            cancel: self.cancel.clone(),
            step_timeout: self.step_timeout,
            seq: self.seq.clone(),
            resume_memo: self.resume_memo.clone(),
            debug: self.debug.clone(),
            human: self.human.clone(),
            // P2-7:分支事件汇入同一观察者通道(Arc 共享)。
            on_step: self.on_step.clone(),
            // P1-3:与 `seq` 同理跨分支共享 —— 去重判定必须看全局最近一条
            // 持久化行,而非本分支的。
            vars_persist: self.vars_persist.clone(),
            inner: Arc::new(Mutex::new(CtxInner {
                inputs: g.inputs.clone(),
                steps: g.steps.clone(),
                vars: g.vars.clone(),
                bindings: g.bindings.clone(),
                // 分支不继承父缓存:fork 后分支内首个渲染重建自己的快照,
                // 之后分支内 set_var 走写时复制,不会污染父/兄弟分支。
                tpl_cache: None,
                // P1-3:fork 时 vars 经 Arc 共享、状态确同 ⇒ 版本号原样复制,
                // 分支首次持久化若 vars 仍未变可继续去重;分支内首次写时复制
                // 会换新版本号。
                vars_rev: g.vars_rev,
                log_buffer: LogBuffer::default(),
                stats: RunStats::default(),
                current_step_id: g.current_step_id.clone(),
                current_step_path: g.current_step_path.clone(),
                current_resource: g.current_resource.clone(),
                last_screenshot: None,
                // P0-2:分支自带全新中断位 —— 各分支的 leaf attempt 独立判死,
                // 一个分支超时不得连坐兄弟分支的阻塞任务。
                step_interrupt: Arc::new(AtomicBool::new(false)),
            })),
        }
    }

    /// P0-4: fold a finished parallel branch's recorded step outputs, vars, run
    /// stats, and logs back into this (parent) context after the join. Branches
    /// should write distinct step ids / var names; on collision the
    /// later-merged branch wins (callers merge in deterministic branch order).
    pub fn merge_branch(&self, branch: &StepCtx) {
        let b = branch.inner.lock();
        let mut g = self.inner.lock();
        g.tpl_cache = None; // 合并写入 steps/vars ⇒ 失效
        g.vars_rev = next_vars_rev(); // P1-3:vars 状态换代(合并即变更)
        {
            let steps = Arc::make_mut(&mut g.steps);
            for (k, v) in b.steps.iter() {
                steps.insert(k.clone(), v.clone());
            }
        }
        {
            let vars = Arc::make_mut(&mut g.vars);
            for (k, v) in b.vars.iter() {
                vars.insert(k.clone(), v.clone());
            }
        }
        g.stats.executed += b.stats.executed;
        g.stats.ok += b.stats.ok;
        g.stats.failed += b.stats.failed;
        g.stats.skipped += b.stats.skipped;
        g.stats.retried += b.stats.retried;
        g.stats.caught += b.stats.caught;
        // P2-1:分支日志经带上限的 push 汇入父环,dropped 计数一并累计。
        for line in &b.log_buffer.entries {
            g.log_buffer.push(line.clone());
        }
        g.log_buffer.dropped += b.log_buffer.dropped;
    }

    /// Stash the step id about to run so `Action::execute` can read it back
    /// for cost / OTel attribution.
    pub fn set_current_step(&self, id: &str) {
        self.inner.lock().current_step_id = Some(id.to_string());
    }

    pub fn current_step_id(&self) -> Option<String> {
        self.inner.lock().current_step_id.clone()
    }

    /// Stash the nested step path (e.g. `loop/item.3/click`) so
    /// `attach_artifact` can attribute artifacts to the right step.
    pub fn set_current_step_path(&self, path: &str) {
        self.inner.lock().current_step_path = Some(path.to_string());
    }

    pub fn current_step_path(&self) -> Option<String> {
        self.inner.lock().current_step_path.clone()
    }

    /// Builder method to set the artifacts directory.
    pub fn with_artifacts_dir(mut self, dir: PathBuf) -> Self {
        self.artifacts_dir = Some(dir);
        self
    }

    /// 架构 P0-1:artifacts 落盘根目录,供子流程 VM 继承 —— 子流程的
    /// `attach_artifact` 与父运行落同一根目录。`None` ⇒ 未开启落盘。
    pub fn artifacts_dir(&self) -> Option<&PathBuf> {
        self.artifacts_dir.as_ref()
    }

    /// Attach a blob artifact (screenshot, DOM, HAR, etc.) to the current step.
    /// Returns the artifact ID (ULID) on success, or empty string if artifacts_dir is None.
    pub fn attach_artifact(
        &self,
        kind: &str,
        mime: &str,
        data: &[u8],
    ) -> Result<String, StepError> {
        let artifacts_dir = match &self.artifacts_dir {
            Some(d) => d,
            None => return Ok(String::new()), // No-op if artifacts_dir not set
        };

        let artifact_id = Ulid::new().to_string();
        let sha256 = Sha256::digest(data).to_vec();
        let ext = mime_to_ext(mime, kind);
        let run_dir = artifacts_dir.join(&self.run_id);
        std::fs::create_dir_all(&run_dir)
            .map_err(|e| StepError::msg(format!("create artifacts dir: {e}")))?;
        let blob_path = run_dir.join(format!("{artifact_id}.{ext}"));
        std::fs::write(&blob_path, data)
            .map_err(|e| StepError::msg(format!("write artifact blob: {e}")))?;

        if let Some(repo) = &self.repo {
            let row = ArtifactRow {
                id: artifact_id.clone(),
                flow_run_id: self.run_id.clone(),
                step_id: self.current_step_id(),
                kind: kind.to_string(),
                mime: mime.to_string(),
                size: data.len() as i64,
                blob_path: blob_path.to_string_lossy().to_string(),
                sha256,
                created_at: chrono::Utc::now(),
            };
            repo.insert_artifact(&row)
                .map_err(|e| StepError::msg(format!("insert artifact row: {e}")))?;
        }

        Ok(artifact_id)
    }

    pub fn mark_step_state(&self, state: &str) {
        let mut g = self.inner.lock();
        g.stats.executed += 1;
        match state {
            "ok" | "cached" => g.stats.ok += 1,
            "failed" => g.stats.failed += 1,
            "skipped" => g.stats.skipped += 1,
            "retrying" => g.stats.retried += 1,
            "caught" | "ai_healed" => {
                g.stats.caught += 1;
                g.stats.ok += 1;
            }
            _ => {}
        }
    }

    pub fn stats(&self) -> RunStats {
        self.inner.lock().stats
    }

    pub fn run_id(&self) -> &str {
        &self.run_id
    }
    pub fn flow_id(&self) -> &str {
        &self.flow_id
    }
    pub fn repo(&self) -> Option<&Repo> {
        self.repo.as_ref()
    }

    pub async fn run_block(&mut self, steps: &[Step]) -> Result<(), crate::ExecError> {
        crate::vm::run_block_inline(self, steps).await
    }

    pub fn resolve_vault_placeholders(&self, value: &Value) -> Result<Value, StepError> {
        VaultResolver {
            names: &self.vault_names,
            repo: self.repo.as_ref(),
            identity: self.vault_identity.as_deref(),
        }
        .resolve_value(value)
    }

    pub fn ensure_fs_read(&self, path: &Path) -> Result<(), StepError> {
        ensure_path_allowed("fs.read", path, &self.capabilities.fs_read)
    }

    pub fn ensure_fs_write(&self, path: &Path) -> Result<(), StepError> {
        ensure_path_allowed("fs.write", path, &self.capabilities.fs_write)
    }

    pub fn ensure_network_url(&self, url: &str) -> Result<(), StepError> {
        // 不回显原始 URL:DSN(postgres://user:pass@…)带凭据,host 解析失败的
        // 畸形串若整条进错误信息,凭据会落进日志/运行记录。只给 scheme 提示。
        let host = extract_host(url).ok_or_else(|| {
            let scheme = url.split("://").next().unwrap_or("<unknown>");
            StepError::msg(format!("network URL has no host (scheme: {scheme})"))
        })?;
        if host_matches_grants(&host, &self.capabilities.network) {
            return Ok(());
        }
        Err(StepError::CapabilityDenied {
            kind: CapKind::Network,
            target: host,
        })
    }

    /// The network capability grants in force for this context. Used by HTTP
    /// actions to re-authorize every redirect hop against the same allow-list
    /// the initial [`ensure_network_url`] gate uses (SSRF / sandbox-bypass fix).
    pub fn network_grants(&self) -> &[String] {
        &self.capabilities.network
    }

    pub fn ensure_llm(&self, model: &str) -> Result<(), StepError> {
        let target = if model.trim().is_empty() { "*" } else { model };
        if matches_any(target, &self.capabilities.llm) {
            return Ok(());
        }
        Err(StepError::CapabilityDenied {
            kind: CapKind::Llm,
            target: target.to_string(),
        })
    }

    pub fn ensure_mcp_server(&self, name: &str) -> Result<(), StepError> {
        if matches_mcp_server(name, &self.capabilities.mcp) {
            return Ok(());
        }
        Err(StepError::CapabilityDenied {
            kind: CapKind::Mcp,
            target: name.to_string(),
        })
    }

    /// Capability gate for a specific `(server, tool)` pair.
    ///
    /// Allow list syntax:
    /// - `"*"`              → all servers, all tools
    /// - `"server"`         → all tools on the named server
    /// - `"server:tool"`    → exact tool
    /// - `"server:tool_*"`  → wildcard tool on the named server
    pub fn ensure_mcp_tool(&self, server: &str, tool: &str) -> Result<(), StepError> {
        if matches_mcp_tool(server, tool, &self.capabilities.mcp) {
            return Ok(());
        }
        Err(StepError::CapabilityDenied {
            kind: CapKind::Mcp,
            target: format!("{server}:{tool}"),
        })
    }

    /// F-1 capability gate for desktop input actuation. `category` is a coarse
    /// bucket — `"mouse"` (move/click/scroll) or `"keyboard"` (key/type) —
    /// checked against the `desktop:` allow-list (`"*"` grants all). Desktop
    /// input can drive the whole machine, so it is denied unless explicitly
    /// granted (empty allow-list ⇒ every category denied).
    pub fn ensure_desktop(&self, category: &str) -> Result<(), StepError> {
        if matches_any(category, &self.capabilities.desktop) {
            return Ok(());
        }
        Err(StepError::CapabilityDenied {
            kind: CapKind::Desktop,
            target: category.to_string(),
        })
    }
}

fn env_snapshot() -> Value {
    let mut m = Map::new();
    for (k, v) in std::env::vars() {
        if k.starts_with("LUMO_") || matches!(k.as_str(), "HOME" | "USER" | "USERNAME" | "PATH") {
            m.insert(k, Value::String(v));
        }
    }
    Value::Object(m)
}

fn ensure_path_allowed(kind: &str, path: &Path, grants: &[String]) -> Result<(), StepError> {
    let candidate = normalize_path(path);
    if grants
        .iter()
        .map(|g| expand_env_vars(g))
        .any(|grant| path_matches(&candidate, &grant))
    {
        return Ok(());
    }
    let cap_kind = match kind {
        "fs.read" => CapKind::FsRead,
        "fs.write" => CapKind::FsWrite,
        _ => unreachable!("ensure_path_allowed called with unsupported kind `{kind}`"),
    };
    Err(StepError::CapabilityDenied {
        kind: cap_kind,
        target: candidate.display().to_string(),
    })
}

fn normalize_path(path: &Path) -> PathBuf {
    let expanded = expand_env_vars(&path.to_string_lossy());
    let p = PathBuf::from(expanded);
    let abs = if p.is_absolute() {
        p
    } else {
        std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .join(p)
    };
    // P0-2: lexically resolve `.` / `..` BEFORE prefix matching so a grant of
    // `/data/**` can't be escaped by `/data/../etc/passwd` (whose raw string
    // still starts with `/data/`). We do NOT canonicalize symlinks here on
    // purpose: canonicalizing the candidate but not the glob grant would break
    // legitimate cases like macOS `/tmp` → `/private/tmp`. Lexical cleaning is
    // the standard, footgun-free way to close the traversal hole.
    lexical_clean(&abs)
}

/// Resolve `.` and `..` components without touching the filesystem. `..` pops
/// the preceding normal component; a `..` that would climb past the root (or a
/// leading `..` in a relative path) is preserved so it can never match a grant.
fn lexical_clean(path: &Path) -> PathBuf {
    use std::path::Component;
    let mut out: Vec<Component> = Vec::new();
    for comp in path.components() {
        match comp {
            Component::CurDir => {}
            Component::ParentDir => match out.last() {
                Some(Component::Normal(_)) => {
                    out.pop();
                }
                Some(Component::RootDir) => { /* `..` at root is a no-op */ }
                _ => out.push(comp),
            },
            other => out.push(other),
        }
    }
    let mut buf = PathBuf::new();
    for comp in out {
        buf.push(comp.as_os_str());
    }
    if buf.as_os_str().is_empty() {
        PathBuf::from(".")
    } else {
        buf
    }
}

fn path_matches(candidate: &Path, grant: &str) -> bool {
    if grant == "*" || grant == "**" {
        return true;
    }
    let candidate = candidate.to_string_lossy();
    let grant = normalize_path(Path::new(grant));
    let grant = grant.to_string_lossy();
    if let Some(prefix) = grant.strip_suffix("/**") {
        candidate == prefix || candidate.starts_with(&format!("{prefix}/"))
    } else if grant.contains('*') {
        wildcard_match(&candidate, &grant)
    } else {
        candidate == grant
    }
}

fn matches_any(candidate: &str, grants: &[String]) -> bool {
    grants.iter().any(|grant| {
        let grant = expand_env_vars(grant);
        grant == "*"
            || grant == candidate
            || grant.strip_prefix("*.").is_some_and(|suffix| {
                candidate == suffix || candidate.ends_with(&format!(".{suffix}"))
            })
            || wildcard_match(candidate, &grant)
    })
}

/// Public wrapper over the network host matcher so HTTP actions can re-gate
/// redirect hops against the network capability grants using the EXACT same
/// logic as [`StepCtx::ensure_network_url`] (one code path, no reimplementation).
/// `host` should be a bare host (no scheme/port), as produced by `extract_host`.
pub fn host_matches_grants(host: &str, grants: &[String]) -> bool {
    matches_any(host, grants)
}

/// `capabilities.mcp` entry → server match (ignores any `:tool` suffix).
fn matches_mcp_server(server: &str, grants: &[String]) -> bool {
    grants.iter().any(|raw| {
        let grant = expand_env_vars(raw);
        let head = grant.split(':').next().unwrap_or(&grant);
        head == "*" || head == server || wildcard_match(server, head)
    })
}

/// `capabilities.mcp` entry → `(server, tool)` match. A grant without `:` allows
/// every tool on the server; a `server:tool` grant gates per tool with wildcards.
fn matches_mcp_tool(server: &str, tool: &str, grants: &[String]) -> bool {
    grants.iter().any(|raw| {
        let grant = expand_env_vars(raw);
        let (head, tool_pat) = match grant.split_once(':') {
            Some((h, t)) => (h, Some(t)),
            None => (grant.as_str(), None),
        };
        let server_ok = head == "*" || head == server || wildcard_match(server, head);
        if !server_ok {
            return false;
        }
        match tool_pat {
            None => true,
            Some(pat) => pat == "*" || pat == tool || wildcard_match(tool, pat),
        }
    })
}

/// P0-5: clamp a child (skill sub-flow) capability set to the caller's, so an
/// invoked skill can never exceed the capabilities of the flow that called it.
/// A child grant is kept only when the parent would also allow it; uncovered
/// grants are dropped (the skill then hits `CapabilityDenied` if it tries to
/// use them, exactly as if the caller lacked the grant). This is the
/// privilege-escalation fix for `skill.invoke`.
pub fn clamp_capabilities(child: &Capabilities, parent: &Capabilities) -> Capabilities {
    Capabilities {
        network: filter_covered(&child.network, |g| host_grant_covered(g, &parent.network)),
        llm: filter_covered(&child.llm, |g| host_grant_covered(g, &parent.llm)),
        mcp: filter_covered(&child.mcp, |g| mcp_grant_covered(g, &parent.mcp)),
        fs_read: filter_covered(&child.fs_read, |g| path_grant_covered(g, &parent.fs_read)),
        fs_write: filter_covered(&child.fs_write, |g| path_grant_covered(g, &parent.fs_write)),
        desktop: filter_covered(&child.desktop, |g| matches_any(g, &parent.desktop)),
    }
}

fn filter_covered(grants: &[String], covered: impl Fn(&str) -> bool) -> Vec<String> {
    grants.iter().filter(|g| covered(g)).cloned().collect()
}

/// A child host/llm grant is covered when the parent allow-list would match it
/// (or its `*.`-stripped representative host).
fn host_grant_covered(child: &str, parent: &[String]) -> bool {
    let c = expand_env_vars(child);
    let repr = c.trim_start_matches("*.");
    matches_any(&c, parent) || matches_any(repr, parent)
}

/// A child MCP grant `server[:tool]` is covered when the parent allows that
/// `(server, tool)` pair (a bare server means "all tools").
fn mcp_grant_covered(child: &str, parent: &[String]) -> bool {
    let c = expand_env_vars(child);
    let (server, tool) = match c.split_once(':') {
        Some((s, t)) => (s, t),
        None => (c.as_str(), "*"),
    };
    matches_mcp_tool(server, tool, parent)
}

/// A child path grant is covered when its glob-stripped representative path
/// matches some parent path grant.
fn path_grant_covered(child: &str, parent: &[String]) -> bool {
    let stripped = child.trim_end_matches("**").trim_end_matches('/');
    let repr = if stripped.is_empty() { child } else { stripped };
    let cand = normalize_path(Path::new(repr));
    parent.iter().any(|p| {
        let p = expand_env_vars(p);
        p == "*" || p == "**" || path_matches(&cand, &p)
    })
}

fn wildcard_match(candidate: &str, pattern: &str) -> bool {
    if pattern == "*" {
        return true;
    }
    let parts: Vec<&str> = pattern.split('*').collect();
    if parts.len() == 1 {
        return candidate == pattern;
    }
    let mut rest = candidate;
    if let Some(first) = parts.first() {
        if !first.is_empty() {
            let Some(stripped) = rest.strip_prefix(first) else {
                return false;
            };
            rest = stripped;
        }
    }
    for part in parts.iter().skip(1).take(parts.len().saturating_sub(2)) {
        if part.is_empty() {
            continue;
        }
        let Some(pos) = rest.find(part) else {
            return false;
        };
        rest = &rest[pos + part.len()..];
    }
    if let Some(last) = parts.last() {
        last.is_empty() || rest.ends_with(last)
    } else {
        true
    }
}

fn expand_env_vars(input: &str) -> String {
    let mut out = input.to_string();
    if let Ok(home) = std::env::var("HOME") {
        out = out.replace("${HOME}", &home).replace("$HOME", &home);
    }
    out
}

fn extract_host(url: &str) -> Option<String> {
    let after_scheme = url.split_once("://").map(|(_, rest)| rest).unwrap_or(url);
    let host_port = after_scheme.split('/').next()?.split('@').next_back()?;
    let host = host_port.split(':').next()?.trim();
    if host.is_empty() {
        None
    } else {
        Some(host.to_ascii_lowercase())
    }
}

/// Resolves `${{ vault.NAME.KEY }}` placeholders left intact through template
/// rendering. Env vars win (back-compat / CI override); the encrypted store is
/// the fallback when both a repo and an identity are present (P1-3).
struct VaultResolver<'a> {
    names: &'a [String],
    repo: Option<&'a Repo>,
    identity: Option<&'a lumo_storage::VaultIdentity>,
}

impl VaultResolver<'_> {
    fn resolve_value(&self, value: &Value) -> Result<Value, StepError> {
        match value {
            Value::String(s) => Ok(Value::String(self.resolve_string(s)?)),
            Value::Array(items) => Ok(Value::Array(
                items
                    .iter()
                    .map(|v| self.resolve_value(v))
                    .collect::<Result<Vec<_>, _>>()?,
            )),
            Value::Object(map) => {
                let mut out = Map::with_capacity(map.len());
                for (k, v) in map {
                    out.insert(k.clone(), self.resolve_value(v)?);
                }
                Ok(Value::Object(out))
            }
            other => Ok(other.clone()),
        }
    }

    fn resolve_string(&self, src: &str) -> Result<String, StepError> {
        let mut out = String::new();
        let mut rest = src;
        while let Some(start) = rest.find("${{ vault.") {
            out.push_str(&rest[..start]);
            let token_rest = &rest[start + 4..];
            let Some(end) = token_rest.find("}}") else {
                out.push_str(&rest[start..]);
                return Ok(out);
            };
            let expr = token_rest[..end].trim();
            out.push_str(&self.resolve_expr(expr)?);
            rest = &token_rest[end + 2..];
        }
        out.push_str(rest);
        Ok(out)
    }

    fn resolve_expr(&self, expr: &str) -> Result<String, StepError> {
        let path = expr
            .strip_prefix("vault.")
            .ok_or_else(|| StepError::msg(format!("invalid vault placeholder `{expr}`")))?;
        let mut parts = path.split('.');
        let name = parts
            .next()
            .ok_or_else(|| StepError::msg(format!("invalid vault placeholder `{expr}`")))?;
        if !self.names.iter().any(|n| n == name) {
            return Err(StepError::msg(format!(
                "vault `{name}` is not declared in spec.vault"
            )));
        }
        let key = parts.collect::<Vec<_>>().join("_");

        // 1) Env wins: LUMO_VAULT_<NAME>[_<KEY>] (back-compat + CI override).
        let env_key = if key.is_empty() {
            format!("LUMO_VAULT_{}", sanitize_env(name))
        } else {
            format!("LUMO_VAULT_{}_{}", sanitize_env(name), sanitize_env(&key))
        };
        if let Ok(v) = std::env::var(&env_key) {
            return Ok(v);
        }

        // 2) Encrypted store fallback (only when both repo + identity present).
        if let (Some(repo), Some(identity)) = (self.repo, self.identity) {
            match lumo_storage::vault::get_field(repo, identity, name, &key) {
                Ok(Some(v)) => return Ok(v),
                Ok(None) => {}
                Err(e) => {
                    return Err(StepError::msg(format!(
                        "vault `{name}` could not be decrypted: {e}"
                    )))
                }
            }
        }

        // 3) Neither env nor store had it.
        Err(StepError::msg(format!(
            "vault value `{expr}` is missing; set {env_key} or run `lumo vault add {name}`"
        )))
    }
}

fn sanitize_env(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_uppercase()
            } else {
                '_'
            }
        })
        .collect()
}

fn mime_to_ext(mime: &str, kind: &str) -> String {
    match mime {
        "image/png" => "png",
        "image/jpeg" => "jpg",
        "text/html" => "html",
        "application/json" => "json",
        "video/webm" => "webm",
        _ => match kind {
            "screenshot" => "png",
            "dom" => "html",
            "har" => "json",
            "video" => "webm",
            _ => "bin",
        },
    }
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_ctx() -> StepCtx {
        StepCtx::new(
            "run-test".into(),
            "flow-test".into(),
            ActionRegistry::new(),
            None,
            Value::Null,
            Capabilities::default(),
            Vec::new(),
        )
    }

    #[test]
    fn record_step_ai_sits_beside_result_without_breaking_reference() {
        let ctx = test_ctx();
        ctx.record_step_output("title", &Value::String("Hello".into()));
        ctx.record_step_ai(
            "title",
            serde_json::json!({ "used": true, "helper": "extract_visual" }),
        );

        let snap = ctx.outputs_snapshot();
        let entry = snap.get("title").expect("step entry present");

        // `steps.title.result` reference contract is preserved.
        assert_eq!(entry.get("result").and_then(Value::as_str), Some("Hello"));
        // `_ai` trace is recorded alongside `result`, not nested inside it.
        assert_eq!(
            entry.pointer("/_ai/helper").and_then(Value::as_str),
            Some("extract_visual")
        );
        assert_eq!(
            entry.pointer("/_ai/used").and_then(Value::as_bool),
            Some(true)
        );
    }

    #[test]
    fn record_step_ai_is_noop_without_prior_output() {
        let ctx = test_ctx();
        // No record_step_output first → nothing to attach to.
        ctx.record_step_ai("ghost", serde_json::json!({ "used": true }));
        assert!(ctx.outputs_snapshot().get("ghost").is_none());
    }

    fn ctx_with_fs_read(grants: Vec<String>) -> StepCtx {
        StepCtx::new(
            "run".into(),
            "flow".into(),
            ActionRegistry::new(),
            None,
            Value::Null,
            Capabilities {
                fs_read: grants,
                ..Default::default()
            },
            Vec::new(),
        )
    }

    #[test]
    fn fs_read_denies_dotdot_escape_from_grant() {
        // P0-2: a `..` segment must not let a path escape its grant root even
        // though the raw string still starts with the granted prefix.
        let ctx = ctx_with_fs_read(vec!["/data/**".into()]);
        assert!(
            ctx.ensure_fs_read(Path::new("/data/sub/f.txt")).is_ok(),
            "paths inside the grant stay allowed"
        );
        assert!(
            ctx.ensure_fs_read(Path::new("/data/../etc/passwd"))
                .is_err(),
            "`/data/../etc/passwd` resolves outside `/data/**` and must be denied"
        );
        assert!(
            ctx.ensure_fs_read(Path::new("/data/sub/../../etc/passwd"))
                .is_err(),
            "deeper `..` escapes must also be denied"
        );
    }

    #[test]
    fn fs_read_allows_internal_dot_segments() {
        // `.` and harmless `..` that stay within the grant must still pass.
        let ctx = ctx_with_fs_read(vec!["/data/**".into()]);
        assert!(ctx.ensure_fs_read(Path::new("/data/./a/b.txt")).is_ok());
        assert!(ctx.ensure_fs_read(Path::new("/data/a/../b.txt")).is_ok());
    }

    #[test]
    fn clamp_capabilities_drops_uncovered_child_grants() {
        // P0-5: an invoked skill (child) can never exceed the caller (parent).
        let parent = Capabilities {
            fs_read: vec!["/data/**".into()],
            network: vec!["api.example.com".into()],
            ..Default::default()
        };
        let child = Capabilities {
            fs_read: vec!["/data/sub/**".into(), "/etc/**".into()],
            network: vec!["api.example.com".into(), "evil.com".into()],
            fs_write: vec!["/tmp/**".into()],
            ..Default::default()
        };
        let clamped = clamp_capabilities(&child, &parent);
        // `/data/sub/**` ⊆ `/data/**` kept; `/etc/**` dropped.
        assert_eq!(clamped.fs_read, vec!["/data/sub/**".to_string()]);
        // `evil.com` dropped, declared `api.example.com` kept.
        assert_eq!(clamped.network, vec!["api.example.com".to_string()]);
        // Parent grants no fs_write, so the child gets none.
        assert!(clamped.fs_write.is_empty());
    }

    #[test]
    fn clamp_capabilities_wildcard_parent_keeps_child() {
        let parent = Capabilities {
            fs_read: vec!["**".into()],
            network: vec!["*".into()],
            ..Default::default()
        };
        let child = Capabilities {
            fs_read: vec!["/anything/**".into()],
            network: vec!["whatever.com".into()],
            ..Default::default()
        };
        let clamped = clamp_capabilities(&child, &parent);
        assert_eq!(clamped.fs_read, child.fs_read);
        assert_eq!(clamped.network, child.network);
    }

    #[test]
    fn fork_shares_state_until_write_then_copies() {
        // A2: `fork()` must share the inputs/steps/vars/bindings allocations
        // (cheap Arc clone) and only copy a map when it is mutated.
        let parent = test_ctx();
        parent.set_var("a", Value::from(1));
        let child = parent.fork();
        {
            let p = parent.inner.lock();
            let c = child.inner.lock();
            assert!(
                Arc::ptr_eq(&p.vars, &c.vars),
                "fork must share the vars allocation until a write (copy-on-write)"
            );
        }
        // A write after the fork copies (diverges) and must not touch the parent.
        child.set_var("b", Value::from(2));
        {
            let p = parent.inner.lock();
            let c = child.inner.lock();
            assert!(
                !Arc::ptr_eq(&p.vars, &c.vars),
                "a write after fork must copy, diverging the allocations"
            );
            assert!(c.vars.contains_key("b"));
            assert!(
                !p.vars.contains_key("b"),
                "the child's write must not leak back into the parent"
            );
            assert!(
                p.vars.contains_key("a") && c.vars.contains_key("a"),
                "state written before the fork stays visible to both"
            );
        }
    }

    fn decl(kind: &str) -> ResourceDecl {
        ResourceDecl {
            kind: kind.into(),
            profile: None,
            config: serde_yaml::Value::Null,
        }
    }

    fn ctx_with_resources(pairs: &[(&str, &str)]) -> StepCtx {
        let resources = pairs
            .iter()
            .map(|(name, kind)| (name.to_string(), decl(kind)))
            .collect::<BTreeMap<_, _>>();
        test_ctx().with_resources(resources)
    }

    #[test]
    fn resource_decl_returns_declared_and_rejects_undeclared() {
        // T3: mirrors the vault-ref check — a declared resource resolves to its
        // decl; an undeclared ref is a hard error at resolve time.
        let ctx = ctx_with_resources(&[("browser", "chromium.cdp"), ("db", "sqlite")]);
        assert!(ctx.has_resource("browser"));
        assert_eq!(ctx.resource_decl("db").unwrap().kind, "sqlite");

        assert!(!ctx.has_resource("ghost"));
        let err = ctx.resource_decl("ghost").unwrap_err().to_string();
        assert!(
            err.contains("ghost") && err.contains("spec.resources"),
            "undeclared-resource error should name the ref and spec.resources, got: {err}"
        );
    }

    #[test]
    fn current_resource_round_trips_and_clears() {
        // T3: the per-step `resource:` ref is stashed like `current_step_id`.
        let ctx = ctx_with_resources(&[("browser", "chromium.cdp")]);
        assert_eq!(ctx.current_resource(), None);
        ctx.set_current_resource(Some("browser"));
        assert_eq!(ctx.current_resource().as_deref(), Some("browser"));
        ctx.set_current_resource(None);
        assert_eq!(ctx.current_resource(), None);
    }

    #[test]
    fn fork_preserves_resource_decls_and_current_ref() {
        // T3: a parallel-branch fork must keep the declared resources (so refs
        // still validate in the branch) and the current resource ref captured at
        // fork time.
        let ctx = ctx_with_resources(&[("browser", "chromium.cdp")]);
        ctx.set_current_resource(Some("browser"));
        let child = ctx.fork();
        assert!(child.has_resource("browser"));
        assert_eq!(child.resource_decl("browser").unwrap().kind, "chromium.cdp");
        assert_eq!(child.current_resource().as_deref(), Some("browser"));
    }

    #[test]
    fn log_buffer_is_a_capped_ring_that_counts_drops() {
        // P2-1: the run log must not grow unbounded — past LOG_BUFFER_MAX the
        // oldest line is dropped and accounted for.
        let ctx = test_ctx();
        for i in 0..(LOG_BUFFER_MAX + 5) {
            ctx.log(format!("line-{i}"));
        }
        let logs = ctx.logs();
        assert_eq!(logs.len(), LOG_BUFFER_MAX, "ring is capped");
        assert_eq!(ctx.logs_dropped(), 5, "overflow drops are counted");
        assert_eq!(
            logs.first().map(String::as_str),
            Some("line-5"),
            "oldest dropped first"
        );
        assert_eq!(
            logs.last().map(String::as_str),
            Some(format!("line-{}", LOG_BUFFER_MAX + 4).as_str()),
            "newest kept"
        );
    }

    #[test]
    fn merge_branch_folds_logs_through_the_cap() {
        // P2-1: branch logs merge through the capped push, dropped counts add up.
        let parent = test_ctx();
        parent.log("p0");
        let child = parent.fork();
        child.log("c0");
        child.log("c1");
        parent.merge_branch(&child);
        assert_eq!(parent.logs(), vec!["p0", "c0", "c1"]);
        assert_eq!(parent.logs_dropped(), 0);
    }

    #[test]
    fn log_event_reaches_on_step_observer() {
        // P2-7: ctx.log (the `control.log` channel) must surface as a Log event.
        let events: Arc<Mutex<Vec<StepEvent>>> = Arc::new(Mutex::new(Vec::new()));
        let sink = events.clone();
        let ctx = test_ctx().with_on_step(Some(Arc::new(move |e: &StepEvent| {
            sink.lock().push(e.clone());
        })));
        ctx.set_current_step_path("a/b");
        ctx.log("hello");
        let got = events.lock();
        assert_eq!(
            *got,
            vec![StepEvent::Log {
                run_id: "run-test".into(),
                step_path: Some("a/b".into()),
                line: "hello".into(),
            }]
        );
        // The line also still lands in the ring buffer.
        drop(got);
        assert_eq!(ctx.logs(), vec!["hello"]);
    }

    #[test]
    fn next_seq_with_vars_dedups_until_vars_change() {
        // P1-3: unchanged vars ⇒ NULL reference row; any mutation ⇒ full snapshot.
        let ctx = test_ctx();
        ctx.set_var("a", Value::from(1));
        let (s0, v0) = ctx.next_seq_with_vars();
        assert_eq!(s0, 0);
        assert!(
            v0.is_some(),
            "first persisted row always writes the full snapshot"
        );
        let (s1, v1) = ctx.next_seq_with_vars();
        assert_eq!(s1, 1);
        assert!(v1.is_none(), "unchanged vars dedup to a NULL reference");
        ctx.set_var("a", Value::from(2));
        let (_, v2) = ctx.next_seq_with_vars();
        assert!(v2.is_some(), "a set_var re-writes the full snapshot");
        ctx.remove_var("a");
        let (_, v3) = ctx.next_seq_with_vars();
        assert!(v3.is_some(), "remove_var also counts as a vars change");
    }

    #[test]
    fn next_seq_with_vars_dedup_spans_undiverged_forks_and_resets_on_divergence() {
        // P1-3: at fork time vars are Arc-shared and rev is copied, so an
        // undiverged branch may keep referencing the parent's snapshot; once a
        // branch writes, its rev diverges and interleaved persists fall back to
        // full snapshots (the "NULL = previous seq row" invariant).
        let parent = test_ctx();
        parent.set_var("a", Value::from(1));
        let (_, p0) = parent.next_seq_with_vars();
        assert!(p0.is_some());
        let child = parent.fork();
        let (_, c0) = child.next_seq_with_vars();
        assert!(c0.is_none(), "undiverged fork shares the same vars state");
        child.set_var("b", Value::from(2));
        let (_, c1) = child.next_seq_with_vars();
        assert!(c1.is_some(), "diverged branch writes full");
        // Parent persists after the child: tracker holds the child's rev, the
        // parent's differs ⇒ full snapshot again (never a wrong NULL).
        let (_, p1) = parent.next_seq_with_vars();
        assert!(
            p1.is_some(),
            "interleaved branch persist must not dedup across states"
        );
    }
}
