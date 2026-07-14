//! Unified database operation execution budget (PIP-0001).
//!
//! `query_timeout = None` only means SQL execution has no upper limit.
//! Checkout, connect, recycle, cancel, and cleanup always have hard upper limits.

use std::time::Duration;

use crate::db;
use crate::models::connection::ConnectionConfig;

/// Default SQL query timeout used when the caller does not supply one.
/// Kept in sync with [`crate::query::QUERY_TIMEOUT`].
pub const DEFAULT_QUERY_TIMEOUT: Duration = Duration::from_secs(30);

/// Fixed cancel budget; cannot be disabled by `query_timeout_secs = 0`.
pub const DEFAULT_CANCEL_TIMEOUT: Duration = Duration::from_secs(5);

/// Fixed cleanup budget; cannot be disabled by `query_timeout_secs = 0`.
pub const DEFAULT_CLEANUP_TIMEOUT: Duration = Duration::from_secs(3);

/// Unified database operation execution budget.
#[derive(Debug, Clone)]
pub struct DbOperationBudget {
    pub checkout_timeout: Duration,
    pub connect_timeout: Duration,
    pub recycle_timeout: Duration,
    pub query_timeout: Option<Duration>,
    pub cancel_timeout: Duration,
    pub cleanup_timeout: Duration,
}

impl DbOperationBudget {
    /// Build an execution budget from connection config values.
    ///
    /// - checkout/connect/recycle use `connect_timeout_secs` (clamped to 1s..=300s)
    /// - query_timeout follows [`resolve_query_timeout`] (`Some(0)` → `None`)
    /// - cancel/cleanup are fixed and cannot be disabled
    pub fn from_config(connect_timeout_secs: u64, query_timeout_secs: Option<u64>) -> Self {
        let infra_timeout = Duration::from_secs(connect_timeout_secs.clamp(1, 300));
        Self {
            checkout_timeout: infra_timeout,
            connect_timeout: infra_timeout,
            recycle_timeout: infra_timeout,
            query_timeout: resolve_query_timeout(query_timeout_secs),
            cancel_timeout: DEFAULT_CANCEL_TIMEOUT,
            cleanup_timeout: DEFAULT_CLEANUP_TIMEOUT,
        }
    }

    pub fn from_connection_config(config: &ConnectionConfig) -> Self {
        Self::from_config(config.effective_connect_timeout_secs(), Some(config.query_timeout_secs))
    }

    /// Global defaults when no connection config is available.
    pub fn with_defaults() -> Self {
        let default_infra = db::connection_timeout();
        Self {
            checkout_timeout: default_infra,
            connect_timeout: default_infra,
            recycle_timeout: default_infra,
            query_timeout: Some(DEFAULT_QUERY_TIMEOUT),
            cancel_timeout: DEFAULT_CANCEL_TIMEOUT,
            cleanup_timeout: DEFAULT_CLEANUP_TIMEOUT,
        }
    }
}

/// Map optional query timeout seconds to a duration budget.
///
/// - `Some(0)` → `None` (SQL execution unbounded)
/// - `Some(n)` → `Some(n seconds)`
/// - `None` → default query timeout
pub fn resolve_query_timeout(timeout_secs: Option<u64>) -> Option<Duration> {
    match timeout_secs {
        Some(0) => None,
        Some(n) => Some(Duration::from_secs(n)),
        None => Some(DEFAULT_QUERY_TIMEOUT),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::connection::DatabaseType;

    fn minimal_config(db_type: DatabaseType, connect_timeout_secs: u64, query_timeout_secs: u64) -> ConnectionConfig {
        ConnectionConfig {
            id: "test".into(),
            name: "test".into(),
            db_type,
            driver_profile: None,
            driver_label: None,
            url_params: None,
            agent_java_options: Vec::new(),
            host: "localhost".into(),
            port: 0,
            username: String::new(),
            password: String::new(),
            database: None,
            visible_databases: None,
            visible_schemas: None,
            attached_databases: Vec::new(),
            init_script: None,
            color: None,
            transport_layers: Vec::new(),
            connect_timeout_secs,
            query_timeout_secs,
            idle_timeout_secs: 60,
            keepalive_interval_secs: 30,
            ssl: false,
            ca_cert_path: String::new(),
            client_cert_path: String::new(),
            client_key_path: String::new(),
            sysdba: false,
            oracle_connection_type: None,
            connection_string: None,
            redis_connection_mode: None,
            redis_sentinel_master: String::new(),
            redis_sentinel_nodes: String::new(),
            redis_sentinel_username: String::new(),
            redis_sentinel_password: String::new(),
            redis_sentinel_tls: false,
            redis_cluster_nodes: String::new(),
            redis_key_separator: crate::models::connection::default_redis_key_separator(),
            redis_scan_page_size: None,
            etcd_endpoints: String::new(),
            gbase_server: String::new(),
            informix_server: String::new(),
            external_config: None,
            jdbc_driver_class: None,
            jdbc_driver_paths: Vec::new(),
            one_time: false,
            read_only: false,
            is_production: false,
            production_databases: vec![],
        }
    }

    #[test]
    fn db_operation_budget_from_config() {
        let budget = DbOperationBudget::from_config(10, Some(30));
        assert_eq!(budget.checkout_timeout, Duration::from_secs(10));
        assert_eq!(budget.connect_timeout, Duration::from_secs(10));
        assert_eq!(budget.recycle_timeout, Duration::from_secs(10));
        assert_eq!(budget.query_timeout, Some(Duration::from_secs(30)));
        assert_eq!(budget.cancel_timeout, Duration::from_secs(5));
        assert_eq!(budget.cleanup_timeout, Duration::from_secs(3));
    }

    #[test]
    fn db_operation_budget_from_connection_config_uses_connection_settings() {
        let config = minimal_config(DatabaseType::Postgres, 12, 0);
        let budget = DbOperationBudget::from_connection_config(&config);

        assert_eq!(budget.checkout_timeout, Duration::from_secs(12));
        assert_eq!(budget.connect_timeout, Duration::from_secs(12));
        assert_eq!(budget.recycle_timeout, Duration::from_secs(12));
        assert_eq!(budget.query_timeout, None);
        assert_eq!(budget.cancel_timeout, Duration::from_secs(5));
        assert_eq!(budget.cleanup_timeout, Duration::from_secs(3));
    }

    #[test]
    fn db_operation_budget_query_timeout_zero_means_no_limit() {
        let budget = DbOperationBudget::from_config(10, Some(0));
        assert_eq!(budget.query_timeout, None);
        assert_eq!(budget.checkout_timeout, Duration::from_secs(10));
        assert_eq!(budget.cancel_timeout, Duration::from_secs(5));
    }

    #[test]
    fn db_operation_budget_query_timeout_zero_keeps_transaction_infra_limits() {
        let config = minimal_config(DatabaseType::Mysql, 7, 0);
        let budget = DbOperationBudget::from_connection_config(&config);

        assert_eq!(budget.query_timeout, None);
        assert_eq!(budget.checkout_timeout, Duration::from_secs(7));
        assert_eq!(budget.recycle_timeout, Duration::from_secs(7));
        assert_eq!(budget.cleanup_timeout, Duration::from_secs(3));
    }

    #[test]
    fn db_operation_budget_clamps_infra_timeout() {
        let budget = DbOperationBudget::from_config(0, Some(30));
        assert_eq!(budget.checkout_timeout, Duration::from_secs(1));
        let budget = DbOperationBudget::from_config(600, Some(30));
        assert_eq!(budget.checkout_timeout, Duration::from_secs(300));
    }

    #[test]
    fn db_operation_budget_with_defaults() {
        let budget = DbOperationBudget::with_defaults();
        assert_eq!(budget.checkout_timeout, db::connection_timeout());
        assert_eq!(budget.query_timeout, Some(DEFAULT_QUERY_TIMEOUT));
    }

    #[test]
    fn default_query_timeout_matches_query_module_constant() {
        assert_eq!(DEFAULT_QUERY_TIMEOUT, crate::query::QUERY_TIMEOUT);
    }
}
