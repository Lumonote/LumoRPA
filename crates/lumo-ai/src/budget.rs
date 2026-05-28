//! AI call budget. Counts helper invocations per run; exceeds → AI hooks disable
//! for the remainder of the run (deterministic paths continue).

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

#[derive(Debug, Clone)]
pub struct RunBudget {
    max: u32,
    used: Arc<AtomicU32>,
}

impl RunBudget {
    pub fn new(max: u32) -> Self {
        Self {
            max,
            used: Arc::new(AtomicU32::new(0)),
        }
    }

    /// Returns Ok(new_used) on success, Err(()) on overflow.
    /// The shared atomic is rolled back on overflow so `used()` reflects truth.
    // Only one failure mode (overflow); a richer error type would carry no extra info.
    #[allow(clippy::result_unit_err)]
    pub fn consume(&self) -> Result<u32, ()> {
        let prev = self.used.fetch_add(1, Ordering::Relaxed);
        if prev + 1 > self.max {
            self.used.fetch_sub(1, Ordering::Relaxed);
            Err(())
        } else {
            Ok(prev + 1)
        }
    }

    pub fn used(&self) -> u32 {
        self.used.load(Ordering::Relaxed)
    }
    pub fn max(&self) -> u32 {
        self.max
    }
    pub fn remaining(&self) -> u32 {
        self.max.saturating_sub(self.used())
    }
}

impl Default for RunBudget {
    fn default() -> Self {
        Self::new(100)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn consumes_and_overflows() {
        let b = RunBudget::new(2);
        assert_eq!(b.consume(), Ok(1));
        assert_eq!(b.consume(), Ok(2));
        assert_eq!(b.consume(), Err(()));
        assert_eq!(b.used(), 2);
        assert_eq!(b.remaining(), 0);
    }

    #[test]
    fn defaults_to_100() {
        let b = RunBudget::default();
        assert_eq!(b.max(), 100);
        assert_eq!(b.used(), 0);
    }

    #[test]
    fn shares_across_clones() {
        let b = RunBudget::new(3);
        let b2 = b.clone();
        assert_eq!(b.consume(), Ok(1));
        assert_eq!(b2.consume(), Ok(2));
        assert_eq!(b.used(), 2);
        assert_eq!(b2.used(), 2);
    }
}
