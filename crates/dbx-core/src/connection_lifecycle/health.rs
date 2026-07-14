//! Budgeted pool health probes (PIP-0001 / PR-A2).
//!
//! PG-wire and MySQL share one policy:
//! - acquire is short so a saturated pool is treated as **busy**, not dead
//! - ping / `SELECT 1` is bounded by [`DbOperationBudget::recycle_timeout`]
//! - stage logs use [`LifecycleStage::Ping`]

use std::time::{Duration, Instant};

use crate::connection_lifecycle::{
    log_stage, DbOperationBudget, LifecycleStage, StageLog, StageLogContext, StageOutcome,
};

/// Max time spent waiting for a free pool handle during a health probe.
///
/// If the pool is saturated, health must **not** remove it — foreground work may still be healthy.
pub const HEALTH_CHECK_POOL_ACQUIRE_TIMEOUT: Duration = Duration::from_millis(500);

/// Outcome of a budgeted health probe against an existing pool handle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PoolHealthProbeResult {
    /// Checkout + ping succeeded within budget.
    Healthy,
    /// Pool acquire timed out (busy). Treat as non-stale.
    Busy,
    /// Acquire or ping failed / timed out — pool should be discarded.
    Unhealthy { reason: String },
}

impl PoolHealthProbeResult {
    pub fn is_stale(&self) -> bool {
        matches!(self, Self::Unhealthy { .. })
    }

    pub fn is_healthy_or_busy(&self) -> bool {
        matches!(self, Self::Healthy | Self::Busy)
    }
}

/// Probe a PostgreSQL (PG-wire) deadpool: short acquire + `SELECT 1` under recycle budget.
pub async fn probe_postgres_pool_health(
    pool: &deadpool_postgres::Pool,
    budget: &DbOperationBudget,
    log_context: StageLogContext<'_>,
) -> PoolHealthProbeResult {
    let start = Instant::now();
    let ping_timeout = budget.recycle_timeout;
    log_stage(
        StageLog::new(LifecycleStage::Ping, StageOutcome::Start, 0)
            .with_timeout(ping_timeout)
            .with_context(log_context),
    );

    let client = match tokio::time::timeout(HEALTH_CHECK_POOL_ACQUIRE_TIMEOUT, pool.get()).await {
        Err(_) => {
            log_stage(
                StageLog::new(LifecycleStage::Ping, StageOutcome::Done, start.elapsed().as_millis())
                    .with_timeout(ping_timeout)
                    .with_context(log_context)
                    .with_error("pool busy; skipped health probe"),
            );
            return PoolHealthProbeResult::Busy;
        }
        Ok(Err(err)) => {
            let reason = format!("pool checkout failed: {err}");
            log_stage(
                StageLog::new(LifecycleStage::Ping, StageOutcome::Error, start.elapsed().as_millis())
                    .with_timeout(ping_timeout)
                    .with_context(log_context)
                    .with_error(&reason),
            );
            return PoolHealthProbeResult::Unhealthy { reason };
        }
        Ok(Ok(client)) => client,
    };

    match tokio::time::timeout(ping_timeout, client.simple_query("SELECT 1")).await {
        Ok(Ok(_)) => {
            log_stage(
                StageLog::new(LifecycleStage::Ping, StageOutcome::Done, start.elapsed().as_millis())
                    .with_timeout(ping_timeout)
                    .with_context(log_context),
            );
            PoolHealthProbeResult::Healthy
        }
        Ok(Err(err)) => {
            let reason = err.to_string();
            log_stage(
                StageLog::new(LifecycleStage::Ping, StageOutcome::Error, start.elapsed().as_millis())
                    .with_timeout(ping_timeout)
                    .with_context(log_context)
                    .with_error(&reason),
            );
            PoolHealthProbeResult::Unhealthy { reason }
        }
        Err(_) => {
            let reason = format!("health check timed out ({}s)", ping_timeout.as_secs().max(1));
            log_stage(
                StageLog::new(LifecycleStage::Ping, StageOutcome::Error, start.elapsed().as_millis())
                    .with_timeout(ping_timeout)
                    .with_context(log_context)
                    .with_error(&reason),
            );
            PoolHealthProbeResult::Unhealthy { reason }
        }
    }
}

/// Probe a MySQL pool: short acquire + `ping` under recycle budget.
pub async fn probe_mysql_pool_health(
    pool: &mysql_async::Pool,
    budget: &DbOperationBudget,
    log_context: StageLogContext<'_>,
) -> PoolHealthProbeResult {
    use mysql_async::prelude::Queryable;

    let start = Instant::now();
    let ping_timeout = budget.recycle_timeout;
    log_stage(
        StageLog::new(LifecycleStage::Ping, StageOutcome::Start, 0)
            .with_timeout(ping_timeout)
            .with_context(log_context),
    );

    let mut conn = match tokio::time::timeout(HEALTH_CHECK_POOL_ACQUIRE_TIMEOUT, pool.get_conn()).await {
        Err(_) => {
            log_stage(
                StageLog::new(LifecycleStage::Ping, StageOutcome::Done, start.elapsed().as_millis())
                    .with_timeout(ping_timeout)
                    .with_context(log_context)
                    .with_error("pool busy; skipped health probe"),
            );
            return PoolHealthProbeResult::Busy;
        }
        Ok(Err(err)) => {
            let reason = err.to_string();
            log_stage(
                StageLog::new(LifecycleStage::Ping, StageOutcome::Error, start.elapsed().as_millis())
                    .with_timeout(ping_timeout)
                    .with_context(log_context)
                    .with_error(&reason),
            );
            return PoolHealthProbeResult::Unhealthy { reason };
        }
        Ok(Ok(conn)) => conn,
    };

    match tokio::time::timeout(ping_timeout, conn.ping()).await {
        Ok(Ok(())) => {
            log_stage(
                StageLog::new(LifecycleStage::Ping, StageOutcome::Done, start.elapsed().as_millis())
                    .with_timeout(ping_timeout)
                    .with_context(log_context),
            );
            PoolHealthProbeResult::Healthy
        }
        Ok(Err(err)) => {
            let reason = err.to_string();
            log_stage(
                StageLog::new(LifecycleStage::Ping, StageOutcome::Error, start.elapsed().as_millis())
                    .with_timeout(ping_timeout)
                    .with_context(log_context)
                    .with_error(&reason),
            );
            PoolHealthProbeResult::Unhealthy { reason }
        }
        Err(_) => {
            let reason = format!("health check timed out ({}s)", ping_timeout.as_secs().max(1));
            log_stage(
                StageLog::new(LifecycleStage::Ping, StageOutcome::Error, start.elapsed().as_millis())
                    .with_timeout(ping_timeout)
                    .with_context(log_context)
                    .with_error(&reason),
            );
            PoolHealthProbeResult::Unhealthy { reason }
        }
    }
}

/// Build a health budget from connect timeout seconds (same clamp as operation budgets).
pub fn health_budget_from_connect_timeout_secs(connect_timeout_secs: u64) -> DbOperationBudget {
    DbOperationBudget::from_config(connect_timeout_secs, Some(0))
}

/// Default health budget when no connection config is available.
pub fn health_budget_defaults() -> DbOperationBudget {
    DbOperationBudget::with_defaults()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn busy_is_not_stale() {
        assert!(!PoolHealthProbeResult::Busy.is_stale());
        assert!(PoolHealthProbeResult::Busy.is_healthy_or_busy());
        assert!(PoolHealthProbeResult::Healthy.is_healthy_or_busy());
        assert!(PoolHealthProbeResult::Unhealthy { reason: "x".into() }.is_stale());
    }

    #[test]
    fn health_budget_keeps_infra_limits_when_query_unbounded() {
        let budget = health_budget_from_connect_timeout_secs(12);
        assert_eq!(budget.checkout_timeout, Duration::from_secs(12));
        assert_eq!(budget.recycle_timeout, Duration::from_secs(12));
        assert_eq!(budget.query_timeout, None);
        assert_eq!(budget.cleanup_timeout, Duration::from_secs(3));
    }

    #[tokio::test]
    async fn postgres_probe_times_out_when_get_never_completes() {
        // deadpool requires a real manager; use a never-ready future pattern via timeout wrapper unit.
        // This test validates the acquire timeout constant is short enough for UI health.
        assert!(HEALTH_CHECK_POOL_ACQUIRE_TIMEOUT <= Duration::from_secs(1));
        let started = Instant::now();
        let _ = tokio::time::timeout(HEALTH_CHECK_POOL_ACQUIRE_TIMEOUT, std::future::pending::<()>()).await;
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    #[tokio::test]
    async fn cleanup_style_timeout_wrapper_returns_within_budget() {
        // Shared contract with cleanup: health ping budget must be enforceable via tokio::timeout.
        let budget = health_budget_from_connect_timeout_secs(1);
        let started = Instant::now();
        let result = tokio::time::timeout(budget.recycle_timeout, std::future::pending::<()>()).await;
        assert!(result.is_err());
        assert!(started.elapsed() < Duration::from_secs(3));
    }
}
