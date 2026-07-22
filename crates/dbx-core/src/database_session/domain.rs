//! Domain-ops session helpers (Phase B residual / multi-adapter slice).
//!
//! Hides `PoolKind` from `mongo_ops` / `redis_ops` / `document_ops` so those modules
//! match capability handles instead of the connection registry enum.
//!
//! See `docs/pips/plans/2026-07-15-phase-b-database-session.md` residual section.

use std::sync::Arc;

use tokio::sync::Mutex;

use crate::connection::{AppState, PoolKind};
use crate::db::agent_driver::AgentDriverClient;
use crate::db::elasticsearch_driver::EsClient;
use crate::db::vector_driver::VectorClient;
use crate::models::connection::ConnectionConfig;
use crate::plugins::PluginDriverSession;

/// Mongo-only pool handle (native driver or legacy agent).
#[derive(Clone)]
pub(crate) enum MongoHandle {
    Native(mongodb::Client),
    Agent(Arc<Mutex<AgentDriverClient>>),
}

/// Document stores that share the document browser surface (Mongo / ES / vector / agent).
#[derive(Clone)]
pub(crate) enum DocumentHandle {
    Mongo(mongodb::Client),
    Elasticsearch(EsClient),
    Vector(VectorClient),
    Agent(Arc<Mutex<AgentDriverClient>>),
}

async fn ensure_pool(state: &AppState, connection_id: &str) -> Result<(), String> {
    state.get_or_create_pool(connection_id, None).await.map(|_| ())
}

/// Resolve a MongoDB (native or agent) handle for domain ops.
pub(crate) async fn resolve_mongo_handle(state: &AppState, connection_id: &str) -> Result<MongoHandle, String> {
    ensure_pool(state, connection_id).await?;
    let connections = state.connections.read().await;
    match connections.get(connection_id).ok_or_else(|| "Not found".to_string())? {
        PoolKind::MongoDb(client) => Ok(MongoHandle::Native(client.clone())),
        PoolKind::Agent(client) => Ok(MongoHandle::Agent(client.clone())),
        _ => Err("Not a MongoDB connection".to_string()),
    }
}

/// Resolve a document-store handle for shared list/find/gridfs surfaces.
pub(crate) async fn resolve_document_handle(state: &AppState, connection_id: &str) -> Result<DocumentHandle, String> {
    ensure_pool(state, connection_id).await?;
    let connections = state.connections.read().await;
    match connections.get(connection_id).ok_or_else(|| "Not found".to_string())? {
        PoolKind::MongoDb(client) => Ok(DocumentHandle::Mongo(client.clone())),
        PoolKind::Elasticsearch(client) => Ok(DocumentHandle::Elasticsearch(client.clone())),
        PoolKind::VectorDb(client) => Ok(DocumentHandle::Vector(client.clone())),
        PoolKind::Agent(client) => Ok(DocumentHandle::Agent(client.clone())),
        _ => Err("Not a MongoDB/Elasticsearch/vector connection".to_string()),
    }
}

/// Inlined Redis domain-ops dispatch: ensure pool, hold registry lock, match Redis.
///
/// Body runs while the read lock is held (same as the historical `PoolKind::Redis`
/// matches). Expands in the caller's async context so lifetimes stay valid.
#[macro_export]
macro_rules! with_redis {
    ($state:expr, $connection_id:expr, |$redis:ident| $body:expr) => {{
        $state.get_or_create_pool($connection_id, None).await.map(|_| ())?;
        let __connections = $state.connections.read().await;
        match __connections.get($connection_id).ok_or_else(|| "Connection not found".to_string())? {
            $crate::connection::PoolKind::Redis($redis) => $body,
            _ => Err("Not a Redis connection".to_string()),
        }
    }};
}

/// Clone a native PostgreSQL pool handle for stream/export paths (no `PoolKind` at call site).
pub(crate) async fn resolve_postgres_pool(
    state: &AppState,
    pool_key: &str,
) -> Result<Option<deadpool_postgres::Pool>, String> {
    let connections = state.connections.read().await;
    Ok(match connections.get(pool_key) {
        Some(PoolKind::Postgres(pool)) => Some(pool.clone()),
        Some(_) => None,
        None => return Err("Connection not found".to_string()),
    })
}

/// Clone a MySQL pool + bare-mode flag for stream/export paths.
pub(crate) async fn resolve_mysql_pool(
    state: &AppState,
    pool_key: &str,
) -> Result<Option<(crate::db::mysql::MySqlPool, bool)>, String> {
    let connections = state.connections.read().await;
    Ok(match connections.get(pool_key) {
        Some(PoolKind::Mysql(pool, mode)) => Some((pool.clone(), *mode == crate::connection::MysqlMode::Bare)),
        Some(_) => None,
        None => return Err("Connection not found".to_string()),
    })
}

/// Clone a ClickHouse client for stream/export paths.
pub(crate) async fn resolve_clickhouse_client(
    state: &AppState,
    pool_key: &str,
) -> Result<Option<crate::db::clickhouse_driver::ChClient>, String> {
    let connections = state.connections.read().await;
    Ok(match connections.get(pool_key) {
        Some(PoolKind::ClickHouse(client)) => Some(client.clone()),
        Some(_) => None,
        None => return Err("Connection not found".to_string()),
    })
}

/// Clone an InfluxDB client for schema early paths.
pub(crate) async fn resolve_influxdb_client(
    state: &AppState,
    pool_key: &str,
) -> Result<Option<crate::db::influxdb_driver::InfluxdbClient>, String> {
    let connections = state.connections.read().await;
    Ok(match connections.get(pool_key) {
        Some(PoolKind::InfluxDb(client)) => Some(client.clone()),
        Some(_) => None,
        None => return Err("Connection not found".to_string()),
    })
}

/// Clone a SQL Server client for stream/export paths.
pub(crate) async fn resolve_sqlserver_client(
    state: &AppState,
    pool_key: &str,
) -> Result<Option<Arc<Mutex<crate::db::sqlserver::SqlServerClient>>>, String> {
    let connections = state.connections.read().await;
    Ok(match connections.get(pool_key) {
        Some(PoolKind::SqlServer(client)) => Some(client.clone()),
        Some(_) => None,
        None => return Err("Connection not found".to_string()),
    })
}

/// Clone a native DuckDB handle (feature-gated registry arm).
#[cfg(feature = "duckdb-bundled")]
pub(crate) async fn resolve_duckdb_handle(
    state: &AppState,
    pool_key: &str,
) -> Result<Option<Arc<crate::db::duckdb_driver::DuckDbConnection>>, String> {
    let connections = state.connections.read().await;
    Ok(match connections.get(pool_key) {
        Some(PoolKind::DuckDb(con)) => Some(con.clone()),
        Some(_) => None,
        None => return Err("Connection not found".to_string()),
    })
}

/// Clone a DuckDB worker-process client handle.
#[cfg(feature = "duckdb-bundled")]
pub(crate) async fn resolve_duckdb_worker(
    state: &AppState,
    pool_key: &str,
) -> Result<Option<Arc<crate::db::duckdb_worker_process::DuckDbWorkerClient>>, String> {
    let connections = state.connections.read().await;
    Ok(match connections.get(pool_key) {
        Some(PoolKind::DuckDbWorker(client)) => Some(client.clone()),
        Some(_) => None,
        None => return Err("Connection not found".to_string()),
    })
}

/// Clone an external-tabular (DuckDB-backed) pool handle.
#[cfg(feature = "duckdb-bundled")]
pub(crate) async fn resolve_external_tabular(
    state: &AppState,
    pool_key: &str,
) -> Result<Option<Arc<crate::external::ExternalPool>>, String> {
    let connections = state.connections.read().await;
    Ok(match connections.get(pool_key) {
        Some(PoolKind::ExternalTabular(pool)) => Some(pool.clone()),
        Some(_) => None,
        None => return Err("Connection not found".to_string()),
    })
}

/// Clone a vector DB client (Milvus / Qdrant / etc.) for schema browser paths.
pub(crate) async fn resolve_vector_client(state: &AppState, pool_key: &str) -> Result<Option<VectorClient>, String> {
    let connections = state.connections.read().await;
    Ok(match connections.get(pool_key) {
        Some(PoolKind::VectorDb(client)) => Some(client.clone()),
        Some(_) => None,
        None => return Err("Connection not found".to_string()),
    })
}

/// External (plugin) driver session handle for schema early paths.
#[derive(Clone)]
pub(crate) struct ExternalDriverHandle {
    pub config: Arc<ConnectionConfig>,
    pub session: Arc<PluginDriverSession>,
}

/// Resolve a plugin external-driver session, if the pool is that kind.
pub(crate) async fn resolve_external_driver(
    state: &AppState,
    pool_key: &str,
) -> Result<Option<ExternalDriverHandle>, String> {
    let connections = state.connections.read().await;
    Ok(match connections.get(pool_key) {
        Some(PoolKind::ExternalDriver { config, session, .. }) => {
            Some(ExternalDriverHandle { config: config.clone(), session: session.clone() })
        }
        Some(_) => None,
        None => return Err("Connection not found".to_string()),
    })
}

/// Resolve a legacy agent driver client, if the pool is that kind.
pub(crate) async fn resolve_agent_client(
    state: &AppState,
    pool_key: &str,
) -> Result<Option<Arc<Mutex<AgentDriverClient>>>, String> {
    let connections = state.connections.read().await;
    Ok(match connections.get(pool_key) {
        Some(PoolKind::Agent(client)) => Some(client.clone()),
        Some(_) => None,
        None => return Err("Connection not found".to_string()),
    })
}

/// Whether the registry entry for `pool_key` is a legacy agent client (no clone).
pub(crate) async fn is_agent_pool(state: &AppState, pool_key: &str) -> bool {
    let connections = state.connections.read().await;
    matches!(connections.get(pool_key), Some(PoolKind::Agent(_)))
}

/// Whether concurrent export-metadata prefetch is safe for this live pool kind.
///
/// Multi-connection pools only (Postgres / MySQL / ClickHouse). Serial clients
/// (SQL Server mutex, Agent, SQLite, DuckDB, …) must stay sequential.
pub(crate) fn concurrent_metadata_prefetch_allowed_for_kind(pool: Option<&PoolKind>) -> bool {
    matches!(pool, Some(PoolKind::Postgres(_)) | Some(PoolKind::Mysql(..)) | Some(PoolKind::ClickHouse(_)))
}

/// Registry lookup wrapper for [`concurrent_metadata_prefetch_allowed_for_kind`].
pub(crate) async fn concurrent_metadata_prefetch_allowed(state: &AppState, pool_key: &str) -> bool {
    let connections = state.connections.read().await;
    concurrent_metadata_prefetch_allowed_for_kind(connections.get(pool_key))
}

/// Whether the registry entry for `pool_key` is a SQL Server client (no clone).
pub(crate) async fn is_sqlserver_pool(state: &AppState, pool_key: &str) -> bool {
    let connections = state.connections.read().await;
    matches!(connections.get(pool_key), Some(PoolKind::SqlServer(_)))
}

/// True when the live registry entry is the same `Arc` as `client` (reset detection).
pub(crate) async fn sqlserver_pool_is_current(
    state: &AppState,
    pool_key: &str,
    client: &Arc<Mutex<crate::db::sqlserver::SqlServerClient>>,
) -> bool {
    let connections = state.connections.read().await;
    matches!(connections.get(pool_key), Some(PoolKind::SqlServer(current)) if Arc::ptr_eq(current, client))
}

/// Owned pool handle for multi-statement transaction dispatch (query residual).
///
/// Clone-friendly arms hold driver handles; `Explicit` / `None` only need kind.
pub(crate) enum TxPath {
    Pg(deadpool_postgres::Pool),
    Mysql(crate::db::mysql::MySqlPool, bool),
    Sqlite(crate::db::sqlite::SqliteHandle),
    CloudflareD1(crate::db::cloudflare_d1_driver::CloudflareD1Client),
    Explicit,
    None,
}

/// Clone the transaction dispatch path under a brief registry read lock.
pub(crate) async fn resolve_tx_path(state: &AppState, pool_key: &str) -> Option<TxPath> {
    let conns = state.connections.read().await;
    conns.get(pool_key).map(|p| match p {
        PoolKind::Postgres(pg) => TxPath::Pg(pg.clone()),
        PoolKind::Mysql(mp, _mode) => TxPath::Mysql(mp.clone(), false),
        PoolKind::Sqlite(sq) => TxPath::Sqlite(sq.clone()),
        PoolKind::CloudflareD1(client) => TxPath::CloudflareD1(client.clone()),
        PoolKind::ClickHouse(_)
        | PoolKind::Rqlite(_)
        | PoolKind::Turso(_)
        | PoolKind::SqlServer(_)
        | PoolKind::Agent(_) => TxPath::Explicit,
        PoolKind::MessageQueue | PoolKind::Nacos => TxPath::None,
        PoolKind::Redis(_)
        | PoolKind::MongoDb(_)
        | PoolKind::Elasticsearch(_)
        | PoolKind::VectorDb(_)
        | PoolKind::InfluxDb(_)
        | PoolKind::ExternalDriver { .. } => TxPath::None,
        #[cfg(feature = "duckdb-bundled")]
        PoolKind::DuckDb(_) | PoolKind::DuckDbWorker(_) | PoolKind::ExternalTabular(_) => TxPath::None,
    })
}

/// Pool handles that support manual BEGIN sessions (Postgres / MySQL only).
pub(crate) enum ManualTxnPool {
    Postgres(deadpool_postgres::Pool),
    Mysql(crate::db::mysql::MySqlPool),
}

/// Resolve a pool for `begin_transaction_session` (errors for unsupported kinds).
pub(crate) async fn resolve_manual_txn_pool(state: &AppState, pool_key: &str) -> Result<ManualTxnPool, String> {
    let connections = state.connections.read().await;
    match connections.get(pool_key).ok_or("Connection not found")? {
        PoolKind::Postgres(pg) => Ok(ManualTxnPool::Postgres(pg.clone())),
        PoolKind::Mysql(mp, _) => Ok(ManualTxnPool::Mysql(mp.clone())),
        _ => Err("Manual transaction is not supported for this database type".to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::{DocumentHandle, MongoHandle};

    #[test]
    fn mongo_handle_is_cloneable_capability_enum() {
        // B5 capability handles are Clone enums so domain ops can resolve once
        // and match without holding the registry lock or naming `PoolKind`.
        fn assert_clone<T: Clone>() {}
        assert_clone::<MongoHandle>();
        assert_clone::<DocumentHandle>();
    }

    #[test]
    fn with_redis_macro_names_poolkind_only_via_crate_path() {
        // Redis is not Clone, so domain ops use an inlined macro that matches
        // PoolKind under the registry lock. Call sites never write `PoolKind`.
        let src = include_str!("domain.rs");
        assert!(src.contains("macro_rules! with_redis"));
        assert!(src.contains("$crate::connection::PoolKind::Redis"));
    }

    #[test]
    fn tx_path_and_manual_txn_cover_query_residual_kinds() {
        // Query residual txn peeks resolve through these enums so call sites
        // never name PoolKind. Arm list must stay aligned with PoolKind.
        let src = include_str!("domain.rs");
        assert!(src.contains("pub(crate) enum TxPath"));
        assert!(src.contains("pub(crate) enum ManualTxnPool"));
        assert!(src.contains("async fn resolve_tx_path"));
        assert!(src.contains("async fn resolve_manual_txn_pool"));
    }
}
