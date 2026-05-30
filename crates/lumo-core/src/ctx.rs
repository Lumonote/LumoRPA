//! Per-step execution context.

use crate::action::ActionRef;
use crate::ai_hook::AiHookProvider;
use crate::error::{CapKind, StepError};
use crate::registry::ActionRegistry;
use lumo_dsl::{Capabilities, FlowAi, Step, TemplateCtx};
use lumo_storage::{ArtifactRow, Repo};
use parking_lot::Mutex;
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use ulid::Ulid;

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

#[derive(Clone)]
pub struct StepCtx {
    pub run_id: String,
    pub flow_id: String,
    pub registry: ActionRegistry,
    capabilities: Capabilities,
    vault_names: Vec<String>,
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
    inner: Arc<Mutex<CtxInner>>,
}

struct CtxInner {
    inputs: Value,
    steps: Map<String, Value>,
    vars: Map<String, Value>,
    bindings: Map<String, Value>,
    log_buffer: Vec<String>,
    stats: RunStats,
    /// Step id currently being executed. Set by the VM right before
    /// `Action::execute`; lets actions (e.g. `ai.chat`) attribute cost rows
    /// to the right step without changing the trait signature.
    current_step_id: Option<String>,
    /// Full nested path (e.g. `loop/item.3/click`) of the current step.
    /// Used by `attach_artifact` so X-07 time-travel rows line up against
    /// the step_runs path column exactly.
    current_step_path: Option<String>,
    /// Last page screenshot stashed by a browser action right before it
    /// surfaced an `ExtractFailed` error. The VM's `extract_visual` AI hook
    /// picks it up so the LLM can *see* the page (true multimodal extraction)
    /// instead of falling back to text-only. Cleared after each consume.
    last_screenshot: Option<bytes::Bytes>,
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
            repo,
            artifacts_dir: None,
            ai_provider: None,
            flow_ai: None,
            vault_identity: None,
            skill_depth: 0,
            cancel: None,
            step_timeout: None,
            seq: Arc::new(AtomicI64::new(0)),
            inner: Arc::new(Mutex::new(CtxInner {
                inputs,
                steps: Map::new(),
                vars: Map::new(),
                bindings: Map::new(),
                log_buffer: Vec::new(),
                stats: RunStats::default(),
                current_step_id: None,
                current_step_path: None,
                last_screenshot: None,
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

    pub fn template_ctx(&self) -> TemplateCtx {
        let g = self.inner.lock();
        TemplateCtx {
            inputs: g.inputs.clone(),
            steps: Value::Object(g.steps.clone()),
            vars: Value::Object(g.vars.clone()),
            bindings: Value::Object(g.bindings.clone()),
            env: env_snapshot(),
            vault: self.vault_names.clone(),
        }
    }

    pub fn record_step_output(&self, step_id: &str, output: &Value) {
        let mut g = self.inner.lock();
        g.steps
            .insert(step_id.to_string(), serde_json::json!({ "result": output }));
    }

    /// Attach an `_ai` trace next to a step's `result` (runtime-only; never
    /// written back to YAML). Lets `steps.<id>._ai` and the Studio timeline
    /// surface a purple "AI heal" badge while `steps.<id>.result` keeps its
    /// reference contract. No-op if the step has no recorded output yet.
    pub fn record_step_ai(&self, step_id: &str, ai: Value) {
        let mut g = self.inner.lock();
        if let Some(Value::Object(entry)) = g.steps.get_mut(step_id) {
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
        self.inner.lock().vars.insert(key.to_string(), value);
    }

    pub fn vars_snapshot(&self) -> Value {
        Value::Object(self.inner.lock().vars.clone())
    }

    pub fn outputs_snapshot(&self) -> Value {
        Value::Object(self.inner.lock().steps.clone())
    }

    pub fn push_binding(&self, key: &str, value: Value) {
        self.inner.lock().bindings.insert(key.into(), value);
    }

    pub fn clear_binding(&self, key: &str) {
        self.inner.lock().bindings.remove(key);
    }

    pub fn log(&self, line: impl Into<String>) {
        let line = line.into();
        self.inner.lock().log_buffer.push(line);
    }

    pub fn lookup_action(&self, id: &str) -> Option<ActionRef> {
        self.registry.get(id)
    }

    pub fn next_step_seq(&self) -> i64 {
        self.seq.fetch_add(1, Ordering::Relaxed)
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
            repo: self.repo.clone(),
            artifacts_dir: self.artifacts_dir.clone(),
            ai_provider: self.ai_provider.clone(),
            flow_ai: self.flow_ai.clone(),
            vault_identity: self.vault_identity.clone(),
            skill_depth: self.skill_depth,
            cancel: self.cancel.clone(),
            step_timeout: self.step_timeout,
            seq: self.seq.clone(),
            inner: Arc::new(Mutex::new(CtxInner {
                inputs: g.inputs.clone(),
                steps: g.steps.clone(),
                vars: g.vars.clone(),
                bindings: g.bindings.clone(),
                log_buffer: Vec::new(),
                stats: RunStats::default(),
                current_step_id: g.current_step_id.clone(),
                current_step_path: g.current_step_path.clone(),
                last_screenshot: None,
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
        for (k, v) in b.steps.iter() {
            g.steps.insert(k.clone(), v.clone());
        }
        for (k, v) in b.vars.iter() {
            g.vars.insert(k.clone(), v.clone());
        }
        g.stats.executed += b.stats.executed;
        g.stats.ok += b.stats.ok;
        g.stats.failed += b.stats.failed;
        g.stats.skipped += b.stats.skipped;
        g.stats.retried += b.stats.retried;
        g.stats.caught += b.stats.caught;
        g.log_buffer.extend(b.log_buffer.iter().cloned());
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
            "ok" => g.stats.ok += 1,
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
        let host = extract_host(url)
            .ok_or_else(|| StepError::msg(format!("network URL has no host: {url}")))?;
        if matches_any(&host, &self.capabilities.network) {
            return Ok(());
        }
        Err(StepError::CapabilityDenied {
            kind: CapKind::Network,
            target: host,
        })
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
}
