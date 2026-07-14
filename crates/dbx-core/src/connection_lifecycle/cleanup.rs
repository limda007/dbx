//! Budgeted pool cleanup (PIP-0001 / PR-A2).
//!
//! Close I/O must never hold the connections write lock. Callers remove the pool
//! handle first, then invoke [`close_with_timeout`] so driver disconnect is bounded
//! by [`DbOperationBudget::cleanup_timeout`] (default 3s).

use std::future::Future;
use std::time::{Duration, Instant};

use crate::connection_lifecycle::{
    log_stage, DbOperationBudget, LifecycleStage, StageLog, StageLogContext, StageOutcome, DEFAULT_CLEANUP_TIMEOUT,
};

/// Default pool-close budget; matches historical `POOL_CLOSE_TIMEOUT_SECS` (3s).
pub const DEFAULT_POOL_CLOSE_TIMEOUT: Duration = DEFAULT_CLEANUP_TIMEOUT;

/// Resolve cleanup timeout from an operation budget (always hard-capped).
pub fn cleanup_timeout_from_budget(budget: &DbOperationBudget) -> Duration {
    if budget.cleanup_timeout.is_zero() {
        DEFAULT_POOL_CLOSE_TIMEOUT
    } else {
        budget.cleanup_timeout
    }
}

/// Run a pool-close future under `cleanup_timeout`, emitting cleanup stage logs.
///
/// On timeout the future is dropped (pool handle drop continues cleanup best-effort).
/// Calling this twice for the same key is safe when the second call is a no-op future
/// (idempotent remove-then-close pattern).
pub async fn close_with_timeout(
    pool_key: &str,
    cleanup_timeout: Duration,
    log_context: StageLogContext<'_>,
    close_future: impl Future<Output = ()>,
) {
    let timeout = if cleanup_timeout.is_zero() { DEFAULT_POOL_CLOSE_TIMEOUT } else { cleanup_timeout };
    let start = Instant::now();
    log_stage(
        StageLog::new(LifecycleStage::Cleanup, StageOutcome::Start, 0)
            .with_timeout(timeout)
            .with_context(log_context)
            .with_pool_key(pool_key),
    );

    match tokio::time::timeout(timeout, close_future).await {
        Ok(()) => {
            log_stage(
                StageLog::new(LifecycleStage::Cleanup, StageOutcome::Done, start.elapsed().as_millis())
                    .with_timeout(timeout)
                    .with_context(log_context)
                    .with_pool_key(pool_key),
            );
        }
        Err(_) => {
            let error = format!(
                "Timed out closing connection pool '{pool_key}' after {}s; cleanup will continue by dropping the pool handle.",
                timeout.as_secs().max(1)
            );
            log::warn!("{error}");
            log_stage(
                StageLog::new(LifecycleStage::Cleanup, StageOutcome::Error, start.elapsed().as_millis())
                    .with_timeout(timeout)
                    .with_context(log_context)
                    .with_pool_key(pool_key)
                    .with_error("cleanup timed out"),
            );
        }
    }
}

/// Convenience: close with the default cleanup budget.
pub async fn close_with_default_timeout(
    pool_key: &str,
    log_context: StageLogContext<'_>,
    close_future: impl Future<Output = ()>,
) {
    close_with_timeout(pool_key, DEFAULT_POOL_CLOSE_TIMEOUT, log_context, close_future).await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    #[test]
    fn default_pool_close_matches_budget_cleanup() {
        assert_eq!(DEFAULT_POOL_CLOSE_TIMEOUT, Duration::from_secs(3));
        assert_eq!(DEFAULT_POOL_CLOSE_TIMEOUT, DEFAULT_CLEANUP_TIMEOUT);
        let budget = DbOperationBudget::with_defaults();
        assert_eq!(cleanup_timeout_from_budget(&budget), DEFAULT_POOL_CLOSE_TIMEOUT);
    }

    #[tokio::test]
    async fn close_with_timeout_completes_when_future_finishes() {
        let started = Instant::now();
        close_with_timeout("pool-1", Duration::from_secs(2), StageLogContext::empty(), async {}).await;
        assert!(started.elapsed() < Duration::from_secs(1));
    }

    #[tokio::test]
    async fn close_with_timeout_returns_within_budget_when_future_hangs() {
        let started = Instant::now();
        close_with_timeout(
            "pool-hang",
            Duration::from_millis(80),
            StageLogContext::empty(),
            std::future::pending::<()>(),
        )
        .await;
        let elapsed = started.elapsed();
        assert!(elapsed >= Duration::from_millis(60));
        assert!(elapsed < Duration::from_secs(2));
    }

    #[tokio::test]
    async fn double_close_is_idempotent() {
        let calls = Arc::new(AtomicUsize::new(0));
        let c1 = calls.clone();
        close_with_timeout("pool-idemp", Duration::from_secs(1), StageLogContext::empty(), async move {
            c1.fetch_add(1, Ordering::SeqCst);
        })
        .await;
        // Second cleanup is a no-op future (pool already removed from map).
        close_with_timeout("pool-idemp", Duration::from_secs(1), StageLogContext::empty(), async {}).await;
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }
}
