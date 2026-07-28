//! Cross-cutting action contracts shared by unrelated instruction families.

use lumo_core::error::StepError;
use std::future::Future;

/// Default maximum number of records returned by collection-producing actions.
pub(crate) const DEFAULT_COLLECTION_LIMIT: usize = 1_000;
pub(crate) const DEFAULT_ACTION_TIMEOUT_MS: u64 = 60_000;

pub(crate) const fn default_collection_limit() -> usize {
    DEFAULT_COLLECTION_LIMIT
}

pub(crate) const fn default_action_timeout_ms() -> u64 {
    DEFAULT_ACTION_TIMEOUT_MS
}

pub(crate) async fn with_timeout<T, F>(timeout_ms: u64, future: F) -> Result<T, StepError>
where
    F: Future<Output = Result<T, StepError>>,
{
    match tokio::time::timeout(std::time::Duration::from_millis(timeout_ms), future).await {
        Ok(result) => result,
        Err(_) => Err(StepError::Timeout { ms: timeout_ms }),
    }
}

pub(crate) async fn with_interruptible_timeout<T, F>(
    timeout_ms: u64,
    interrupt: lumo_core::StepInterrupt,
    future: F,
) -> Result<T, StepError>
where
    F: Future<Output = Result<T, StepError>>,
{
    match tokio::time::timeout(std::time::Duration::from_millis(timeout_ms), future).await {
        Ok(result) => result,
        Err(_) => {
            interrupt.interrupt();
            Err(StepError::Timeout { ms: timeout_ms })
        }
    }
}

pub(crate) fn checked_limit(action: &str, limit: usize) -> Result<usize, StepError> {
    if limit == 0 {
        Err(StepError::msg(format!("{action}: `limit` must be >= 1")))
    } else {
        Ok(limit)
    }
}

/// Truncate a `limit + 1` (or larger) probe and report whether more data exists.
pub(crate) fn truncate_with_flag<T>(items: &mut Vec<T>, limit: usize) -> bool {
    let truncated = items.len() > limit;
    if truncated {
        items.truncate(limit);
    }
    truncated
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncation_uses_the_extra_item_as_the_signal() {
        let mut items = vec![1, 2, 3];
        assert!(truncate_with_flag(&mut items, 2));
        assert_eq!(items, vec![1, 2]);
        assert!(!truncate_with_flag(&mut items, 2));
    }

    #[tokio::test]
    async fn interruptible_timeout_marks_blocking_checkpoint_dead() {
        let interrupt = lumo_core::StepInterrupt::default();
        let err = with_interruptible_timeout(1, interrupt.clone(), async {
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            Ok::<_, StepError>(())
        })
        .await
        .unwrap_err();
        assert!(matches!(err, StepError::Timeout { ms: 1 }));
        assert!(interrupt.is_interrupted());
    }
}
