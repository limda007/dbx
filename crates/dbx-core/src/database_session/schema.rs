//! Schema browse dispatch (Phase B2).
//!
//! Owns the final multi-arm `PoolKind` matches for list_databases / list_schemas /
//! list_tables. Callers in `schema.rs` keep orchestration: retries, agent/external
//! special cases, visible-schema filters, and pool creation.

use crate::connection::{AppState, MysqlMode, PoolKind};
use crate::db;
use crate::models::connection::ConnectionConfig;

/// Final native-pool dispatch for listing databases.
///
/// Callers must already handle ExternalDriver / Agent / ClickHouse / SqlServer /
/// ExternalTabular special cases. `pool_key` is usually the connection_id for
/// the default pool.
pub(crate) async fn list_databases(
    state: &AppState,
    pool_key: &str,
    db_config: Option<&ConnectionConfig>,
    #[cfg_attr(not(feature = "duckdb-bundled"), allow(unused_variables))] duckdb_attached_names: &[String],
) -> Result<Vec<db::DatabaseInfo>, String> {
    let connections = state.connections.read().await;
    let pool = connections.get(pool_key).ok_or("Connection not found")?;

    match pool {
        PoolKind::Mysql(p, _) if db_config.is_some_and(crate::schema::is_doris_family_config) => {
            db::mysql::list_databases_show(p)
                .await
                .map(|databases| crate::schema::filter_mysql_system_databases_for_config(databases, db_config))
        }
        PoolKind::Mysql(p, mode) if *mode == MysqlMode::OceanBaseOracle => db::ob_oracle::list_databases(p).await,
        PoolKind::Mysql(p, _) => db::mysql::list_databases(p).await,
        PoolKind::Postgres(p) => db::postgres::list_databases(p).await,
        PoolKind::Sqlite(p) => db::sqlite::list_databases(p).await,
        PoolKind::Rqlite(client) => db::rqlite_driver::list_databases(client).await,
        #[cfg(feature = "duckdb-bundled")]
        PoolKind::DuckDb(con) => {
            let con = con.lock().map_err(|e| e.to_string())?;
            crate::schema::duckdb_list_databases_with_attached(&con, duckdb_attached_names)
        }
        #[cfg(feature = "duckdb-bundled")]
        PoolKind::DuckDbWorker(client) => {
            let client = client.clone();
            drop(connections);
            client.list_databases().await
        }
        PoolKind::CloudflareD1(client) => db::cloudflare_d1_driver::list_databases(client).await,
        _ => Ok(vec![]),
    }
}

/// Final native-pool dispatch for listing schemas.
///
/// Returns raw schema names; callers apply visible-schema filtering.
pub(crate) async fn list_schemas(
    state: &AppState,
    pool_key: &str,
    #[cfg_attr(not(feature = "duckdb-bundled"), allow(unused_variables))] database: &str,
    #[cfg_attr(not(feature = "duckdb-bundled"), allow(unused_variables))] connection_id: &str,
) -> Result<Vec<String>, String> {
    let connections = state.connections.read().await;
    let pool = connections.get(pool_key).ok_or("Pool not found")?;

    match pool {
        PoolKind::Mysql(p, mode) if *mode == MysqlMode::OceanBaseOracle => db::ob_oracle::list_schemas(p).await,
        PoolKind::Postgres(p) => db::postgres::list_schemas(p).await,
        #[cfg(feature = "duckdb-bundled")]
        PoolKind::DuckDb(con) => {
            let duckdb_attached_names = crate::schema::duckdb_attached_database_names(state, connection_id).await;
            let con = con.lock().map_err(|e| e.to_string())?;
            crate::schema::duckdb_list_schemas_with_attached(&con, database, &duckdb_attached_names)
        }
        #[cfg(feature = "duckdb-bundled")]
        PoolKind::DuckDbWorker(client) => {
            let client = client.clone();
            let database = database.to_string();
            drop(connections);
            client.list_schemas(database).await
        }
        _ => Ok(vec![]),
    }
}

/// Final native-pool dispatch for listing tables / collections / indices.
///
/// Applies the same filter/limit/offset semantics as the previous `schema.rs` match.
pub(crate) async fn list_tables(
    state: &AppState,
    pool_key: &str,
    db_config: Option<&ConnectionConfig>,
    database: &str,
    schema: &str,
    filter: Option<&str>,
    limit: Option<usize>,
    offset: Option<usize>,
    object_types: Option<&[String]>,
) -> Result<Vec<db::TableInfo>, String> {
    let connections = state.connections.read().await;
    let pool = connections.get(pool_key).ok_or("Pool not found")?;

    match pool {
        PoolKind::Mysql(p, _) if db_config.is_some_and(crate::schema::is_doris_family_config) => {
            db::mysql::list_tables_show(p, database)
                .await
                .map(|tables| crate::schema::filter_table_infos(tables, filter, limit, offset, object_types))
        }
        PoolKind::Mysql(p, mode) => {
            if *mode == MysqlMode::OceanBaseOracle {
                let tables = db::ob_oracle::list_tables(p, schema).await?;
                Ok(crate::schema::filter_table_infos(tables, filter, limit, offset, object_types))
            } else {
                db::mysql::list_tables_filtered(
                    p,
                    crate::schema::mysql_table_metadata_catalog(database, schema),
                    filter,
                    limit,
                    offset,
                    object_types,
                )
                .await
                .map(|tables| crate::schema::filter_table_infos(tables, None, None, None, object_types))
            }
        }
        PoolKind::Postgres(p) if db_config.is_some_and(crate::schema::is_questdb_config) => {
            db::questdb::list_tables(p, schema)
                .await
                .map(|tables| crate::schema::filter_table_infos(tables, filter, limit, offset, object_types))
        }
        PoolKind::Postgres(p) => {
            if object_types.is_some() {
                db::postgres::list_tables_filtered(p, schema, filter, None, None)
                    .await
                    .map(|tables| crate::schema::filter_table_infos(tables, filter, limit, offset, object_types))
            } else {
                db::postgres::list_tables_filtered(p, schema, filter, limit, offset).await
            }
        }
        PoolKind::Sqlite(p) => db::sqlite::list_tables(p, schema)
            .await
            .map(|tables| crate::schema::filter_table_infos(tables, filter, limit, offset, object_types)),
        PoolKind::Rqlite(client) => db::rqlite_driver::list_tables(client, schema)
            .await
            .map(|tables| crate::schema::filter_table_infos(tables, filter, limit, offset, object_types)),
        PoolKind::MongoDb(client) => db::mongo_driver::list_collections(client, database)
            .await
            .map(|names| crate::schema::collection_names_to_tables(names, "COLLECTION"))
            .map(|tables| crate::schema::filter_table_infos(tables, filter, limit, offset, object_types)),
        PoolKind::Elasticsearch(client) => db::elasticsearch_driver::list_indices(client)
            .await
            .map(|names| crate::schema::collection_names_to_tables(names, "INDEX"))
            .map(|tables| crate::schema::filter_table_infos(tables, filter, limit, offset, object_types)),
        PoolKind::VectorDb(client) => db::vector_driver::list_collections(client)
            .await
            .map(|infos| {
                crate::schema::collection_names_to_tables(infos.into_iter().map(|i| i.name).collect(), "COLLECTION")
            })
            .map(|tables| crate::schema::filter_table_infos(tables, filter, limit, offset, object_types)),
        PoolKind::CloudflareD1(client) => db::cloudflare_d1_driver::list_tables(client, schema)
            .await
            .map(|tables| crate::schema::filter_table_infos(tables, filter, limit, offset, object_types)),
        _ => Ok(vec![]),
    }
}
