//! Connection lifecycle deep module (architecture review #1 / PIP-0001).
//!
//! Owns execution budgets and stage logging for connect, health, checkout,
//! cancel, and cleanup. Later PRs move connect/health/cleanup orchestration
//! here; callers (Tauri, web, query) stay as thin adapters.
//!
//! See `docs/pips/plans/2026-07-14-phase-a-connection-lifecycle.md`.

mod budget;
mod stage;

pub use budget::{
    resolve_query_timeout, DbOperationBudget, DEFAULT_CANCEL_TIMEOUT, DEFAULT_CLEANUP_TIMEOUT, DEFAULT_QUERY_TIMEOUT,
};
pub use stage::{
    connection_id_from_pool_key, database_type_log_label, format_stage_log, log_stage,
    optional_database_type_log_label, LifecycleStage, StageLog, StageLogContext, StageOutcome,
};
