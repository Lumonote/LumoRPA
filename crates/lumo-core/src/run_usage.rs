use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

#[derive(Debug, Clone)]
pub enum RunUsageEvent {
    Started {
        run_id: String,
        parent_run_id: Option<String>,
    },
    Finished {
        run_id: String,
        parent_run_id: Option<String>,
        tokens_used: u64,
        cost_usd_micro: u64,
        usage_unknown: bool,
        state: String,
        duration_ms: u64,
    },
}

pub type RunUsageObserver = Arc<dyn Fn(&RunUsageEvent) + Send + Sync>;

/// A dropped request may have reached the provider even when no response was
/// observed. Keep that uncertainty separate from the known billed lower bound.
pub struct UsageCallGuard {
    unknown: Arc<AtomicBool>,
    resolved: bool,
}

impl UsageCallGuard {
    pub(crate) fn new(unknown: Arc<AtomicBool>) -> Self {
        Self {
            unknown,
            resolved: false,
        }
    }

    pub fn finish(mut self, reported: bool) {
        if !reported {
            self.unknown.store(true, Ordering::Relaxed);
        }
        self.resolved = true;
    }
}

impl Drop for UsageCallGuard {
    fn drop(&mut self) {
        if !self.resolved {
            self.unknown.store(true, Ordering::Relaxed);
        }
    }
}
