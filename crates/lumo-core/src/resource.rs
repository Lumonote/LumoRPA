//! T3 — per-node resource lifecycle: the [`ResourceFactory`] abstraction.
//!
//! A *resource* is an external dependency declared once under `spec.resources`
//! (browser, db, http session, ftp/smtp connection) that the VM instantiates a
//! single time per run and reuses across every step that references it via
//! `Step.resource`, then tears down at run end ("启动一次/start once").
//!
//! Each *kind* of resource is opened by a [`ResourceFactory`] registered on the
//! [`ActionRegistry`](crate::ActionRegistry), alongside actions and
//! [`RunTeardown`](crate::RunTeardown) hooks. A factory owns its *own*
//! per-`(run_id, name)` storage of live handles — exactly the model the browser
//! action already uses (`SESSIONS: OnceCell<HashMap<run_id, Arc<Session>>>`).
//! Keeping handles inside the owning family, rather than in a central store,
//! lets `!Send` handles like a CDP `Page` stay out of the shared, fork-able
//! [`StepCtx`](crate::StepCtx); the context carries only the resource
//! *declarations*, for ref validation and config lookup.

use crate::error::StepError;
use async_trait::async_trait;
use lumo_dsl::ResourceDecl;

/// Opens and reuses one *kind* of declared resource for a run.
///
/// Implemented by each stateful action family (browser, db, http, ftp, smtp)
/// and registered via
/// [`ActionRegistry::register_resource_factory`](crate::ActionRegistry::register_resource_factory).
/// The VM/actions resolve a factory by [`kind`](Self::kind) and call
/// [`open`](Self::open) on the first step that references a resource of that
/// kind, reusing the existing handle on subsequent references.
///
/// # Storage & lifetime
/// The live handle produced by `open` lives in the factory's *own*
/// per-`(run_id, name)` map — never in [`StepCtx`](crate::StepCtx) — so handles
/// that are not `Send`/`Sync` (e.g. a CDP `Page`) don't have to cross the
/// fork-able context boundary. Reclamation stays with the existing
/// [`RunTeardown`](crate::RunTeardown) hook (one per family), keyed by the same
/// `run_id`. Because the handle is owned family-side, `open` returns only
/// success/failure rather than the handle itself; the family exposes its own
/// typed accessor (e.g. browser's `session_for(run_id, name)`) to *use* it.
#[async_trait]
pub trait ResourceFactory: Send + Sync + 'static {
    /// The `spec.resources.<name>.kind` selector this factory handles
    /// (e.g. `"chromium.cdp"`, `"sqlite"`, `"http"`, `"ftp"`, `"smtp"`).
    fn kind(&self) -> &str;

    /// Ensure the resource named `name` — declared by `decl` — is open and ready
    /// for `run_id`, creating it lazily on the first call and reusing the live
    /// handle on later calls. The handle is stashed in the factory's own
    /// per-`(run_id, name)` storage; this returns only `Ok(())`/error so that
    /// `!Send` handles never enter the shared context.
    ///
    /// Must be **idempotent per `(run_id, name)`**: a second call for an
    /// already-open resource confirms/reuses the existing handle instead of
    /// opening a duplicate. Any capability gating (network host, etc.) applies
    /// here, at open time.
    ///
    /// If a future kind needs `open` to surface `Send` metadata (e.g. a
    /// negotiated session id distinct from `name`), widen the return to
    /// `Result<SomeMeta, StepError>` — an additive change that does *not*
    /// reintroduce the `!Send`-handle problem this `()` return avoids.
    async fn open(&self, decl: &ResourceDecl, run_id: &str, name: &str) -> Result<(), StepError>;
}
