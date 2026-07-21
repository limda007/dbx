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
}
