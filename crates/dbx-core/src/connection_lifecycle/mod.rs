//! Connection lifecycle deep module (architecture review #1 / PIP-0001).
//!
//! Owns execution budgets and stage logging for connect, health, checkout,
//! cancel, and cleanup. Later PRs move connect orchestration here; callers
//! (Tauri, web, query) stay as thin adapters.
//!
//! See `docs/pips/plans/2026-07-14-phase-a-connection-lifecycle.md`.

mod budget;
mod cleanup;
pub mod connect;
mod health;
mod stage;

pub use budget::{
    resolve_query_timeout, DbOperationBudget, DEFAULT_CANCEL_TIMEOUT, DEFAULT_CLEANUP_TIMEOUT, DEFAULT_QUERY_TIMEOUT,
};
pub use cleanup::{
    cleanup_timeout_from_budget, close_with_default_timeout, close_with_timeout, DEFAULT_POOL_CLOSE_TIMEOUT,
};
pub use connect::{connect, test_connection};
pub use health::{
    health_budget_defaults, health_budget_from_connect_timeout_secs, probe_mysql_pool_health,
    probe_postgres_pool_health, PoolHealthProbeResult, HEALTH_CHECK_POOL_ACQUIRE_TIMEOUT,
};
pub use stage::{
    connection_id_from_pool_key, database_type_log_label, format_stage_log, log_stage,
    optional_database_type_log_label, LifecycleStage, StageLog, StageLogContext, StageOutcome,
};
