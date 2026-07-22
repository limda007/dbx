//! Schema browse dispatch (Phase B2 / B2.1).
//!
//! Owns the final multi-arm `PoolKind` matches for schema tree hot paths.
//! Callers in `schema.rs` keep orchestration: retries, agent/external special
//! cases, visible-schema filters, and pool creation.

use crate::connection::{AppState, MysqlMode, PoolKind};
use crate::db;
use crate::models::connection::ConnectionConfig;

use super::traits::SchemaBrowse;

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
        PoolKind::Mysql(p, _) => {
            // B5 SchemaBrowse default path (non-Doris / non-OB-Oracle special cases).
            super::MysqlSession::new(p.clone(), false).list_databases().await
        }
        PoolKind::Postgres(p) => super::PostgresSession::new(p.clone()).list_databases().await,
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
        PoolKind::Postgres(p) => super::PostgresSession::new(p.clone()).list_schemas().await,
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
        PoolKind::Mysql(p, _) if db_config.is_some_and(crate::schema::is_starrocks_config) => {
            db::mysql::list_starrocks_tables(p, database)
                .await
                .map(|tables| crate::schema::filter_table_infos(tables, filter, limit, offset, object_types))
        }
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
                // B5 SchemaBrowse default MySQL path.
                super::MysqlSession::new(p.clone(), *mode == MysqlMode::Bare)
                    .list_tables(database, schema, filter, limit, offset, object_types)
                    .await
            }
        }
        PoolKind::Postgres(p) if db_config.is_some_and(crate::schema::is_questdb_config) => {
            db::questdb::list_tables(p, schema)
                .await
                .map(|tables| crate::schema::filter_table_infos(tables, filter, limit, offset, object_types))
        }
        PoolKind::Postgres(p) => {
            super::PostgresSession::new(p.clone())
                .list_tables(database, schema, filter, limit, offset, object_types)
                .await
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

/// Final native-pool dispatch for listing schema objects (tables/views/…).
///
/// Returns `Ok(None)` when the caller should fall back to `list_tables` mapping
/// (same behavior as the previous `_` arm in `list_objects_once`).
pub(crate) async fn list_objects(
    state: &AppState,
    pool_key: &str,
    db_config: Option<&ConnectionConfig>,
    database: &str,
    schema: &str,
    object_types: Option<&[String]>,
    mysql_limit: Option<usize>,
    mysql_offset: Option<usize>,
) -> Result<Option<crate::schema::ObjectListOutcome>, String> {
    let connections = state.connections.read().await;
    let pool = connections.get(pool_key).ok_or("Pool not found")?;

    match pool {
        PoolKind::Mysql(p, mode) => {
            if *mode == MysqlMode::OceanBaseOracle {
                db::ob_oracle::list_objects(p, schema).await.map(crate::schema::unpaged_object_list).map(Some)
            } else if db_config.is_some_and(crate::schema::is_manticoresearch_config) {
                db::manticoresearch::list_objects(p, database).await.map(crate::schema::unpaged_object_list).map(Some)
            } else if db_config.is_some_and(crate::schema::is_starrocks_config) {
                db::mysql::list_starrocks_table_objects(p, database)
                    .await
                    .map(crate::schema::unpaged_object_list)
                    .map(Some)
            } else if db_config.is_some_and(crate::schema::is_doris_family_config) {
                db::mysql::list_table_objects_show(p, database).await.map(crate::schema::unpaged_object_list).map(Some)
            } else {
                db::mysql::list_objects(p, database, object_types, mysql_limit, mysql_offset).await.map(|result| {
                    Some(crate::schema::ObjectListOutcome {
                        objects: result.objects,
                        paging_applied: result.paging_applied,
                    })
                })
            }
        }
        PoolKind::Postgres(p) if db_config.is_some_and(crate::schema::is_questdb_config) => {
            db::questdb::list_objects(p, schema).await.map(crate::schema::unpaged_object_list).map(Some)
        }
        PoolKind::Postgres(p) => {
            db::postgres::list_objects(p, schema).await.map(crate::schema::unpaged_object_list).map(Some)
        }
        _ => Ok(None),
    }
}

/// Final native-pool dispatch for completion-assistant object lists.
///
/// Returns `Ok(None)` when the pool is SqlServer (caller reuses `list_objects_once`)
/// or another unsupported type (empty list).
pub(crate) async fn list_completion_objects(
    state: &AppState,
    pool_key: &str,
    db_config: Option<&ConnectionConfig>,
    database: &str,
    schema: &str,
) -> Result<Option<Vec<db::ObjectInfo>>, String> {
    let connections = state.connections.read().await;
    let pool = connections.get(pool_key).ok_or("Pool not found")?;

    match pool {
        PoolKind::Mysql(p, mode) if *mode != MysqlMode::OceanBaseOracle => {
            db::mysql::list_completion_objects(p, database).await.map(Some)
        }
        PoolKind::Mysql(p, mode) if *mode == MysqlMode::OceanBaseOracle => {
            db::ob_oracle::list_objects(p, schema).await.map(crate::schema::filter_completion_objects).map(Some)
        }
        PoolKind::Postgres(p) if db_config.is_some_and(crate::schema::is_questdb_config) => {
            db::questdb::list_objects(p, schema).await.map(crate::schema::filter_completion_objects).map(Some)
        }
        PoolKind::Postgres(p) => {
            db::postgres::list_objects(p, schema).await.map(crate::schema::filter_completion_objects).map(Some)
        }
        PoolKind::SqlServer(_) => Ok(None),
        _ => Ok(Some(Vec::new())),
    }
}

/// Final native-pool dispatch for object statistics.
pub(crate) async fn list_object_statistics(
    state: &AppState,
    pool_key: &str,
    db_config: Option<&ConnectionConfig>,
    database: &str,
    schema: &str,
) -> Result<Vec<db::ObjectStatistics>, String> {
    let connections = state.connections.read().await;
    let pool = connections.get(pool_key).ok_or("Pool not found")?;

    match pool {
        PoolKind::Mysql(p, mode) => {
            if *mode == MysqlMode::OceanBaseOracle || db_config.is_some_and(crate::schema::is_manticoresearch_config) {
                Ok(vec![])
            } else {
                db::mysql::list_object_statistics(p, database).await
            }
        }
        PoolKind::Postgres(p) if db_config.is_some_and(crate::schema::is_questdb_config) => Ok(vec![]),
        PoolKind::Postgres(p) => db::postgres::list_object_statistics(p, schema).await,
        PoolKind::ClickHouse(client) => {
            let metadata_database = if database.is_empty() { schema } else { database };
            db::clickhouse_driver::list_object_statistics(client, metadata_database).await
        }
        _ => Ok(vec![]),
    }
}

/// Final native-pool dispatch for table columns.
pub(crate) async fn get_columns(
    state: &AppState,
    pool_key: &str,
    db_config: Option<&ConnectionConfig>,
    database: &str,
    schema: &str,
    table: &str,
) -> Result<Vec<db::ColumnInfo>, String> {
    let connections = state.connections.read().await;
    let pool = connections.get(pool_key).ok_or("Pool not found")?;

    match pool {
        PoolKind::Mysql(p, _) if db_config.is_some_and(crate::schema::is_manticoresearch_config) => {
            let metadata_database = crate::schema::mysql_show_metadata_database_for_config(db_config, database);
            db::manticoresearch::get_columns(p, metadata_database, table)
                .await
                .map(crate::schema::deduplicate_column_infos)
        }
        PoolKind::Mysql(p, _) if db_config.is_some_and(crate::schema::is_doris_family_config) => {
            let metadata_database = crate::schema::mysql_show_metadata_database_for_config(db_config, database);
            db::mysql::get_columns(p, metadata_database, table).await.map(crate::schema::deduplicate_column_infos)
        }
        PoolKind::Mysql(p, mode) if *mode == MysqlMode::OceanBaseOracle => {
            let effective_db = crate::schema::mysql_table_metadata_catalog(database, schema);
            db::ob_oracle::get_columns(p, effective_db, table).await.map(crate::schema::deduplicate_column_infos)
        }
        PoolKind::Mysql(p, _) => {
            let effective_db = crate::schema::mysql_table_metadata_catalog(database, schema);
            db::mysql::get_columns(p, effective_db, table).await.map(crate::schema::deduplicate_column_infos)
        }
        PoolKind::Postgres(p) if db_config.is_some_and(crate::schema::is_questdb_config) => {
            db::questdb::get_columns(p, schema, table).await.map(crate::schema::deduplicate_column_infos)
        }
        PoolKind::Postgres(p) => {
            db::postgres::get_columns(p, schema, table).await.map(crate::schema::deduplicate_column_infos)
        }
        PoolKind::Sqlite(p) => {
            db::sqlite::get_columns(p, schema, table).await.map(crate::schema::deduplicate_column_infos)
        }
        PoolKind::Rqlite(client) => {
            db::rqlite_driver::get_columns(client, schema, table).await.map(crate::schema::deduplicate_column_infos)
        }
        PoolKind::CloudflareD1(client) => db::cloudflare_d1_driver::get_columns(client, schema, table)
            .await
            .map(crate::schema::deduplicate_column_infos),
        PoolKind::Elasticsearch(client) => {
            db::elasticsearch_driver::get_columns(client, table).await.map(crate::schema::deduplicate_column_infos)
        }
        _ => Ok(vec![]),
    }
}

/// Final native-pool dispatch for indexes.
pub(crate) async fn list_indexes(
    state: &AppState,
    pool_key: &str,
    db_config: Option<&ConnectionConfig>,
    database: &str,
    schema: &str,
    table: &str,
) -> Result<Vec<db::IndexInfo>, String> {
    let connections = state.connections.read().await;
    let pool = connections.get(pool_key).ok_or("Pool not found")?;

    match pool {
        PoolKind::Mysql(p, mode) => {
            if db_config.is_some_and(crate::schema::is_manticoresearch_config) {
                return db::manticoresearch::list_indexes(p, table).await;
            }
            if *mode == MysqlMode::OceanBaseOracle {
                db::ob_oracle::list_indexes(p, schema, table).await
            } else if db_config.is_some_and(crate::schema::is_doris_family_config) {
                db::mysql::list_doris_family_indexes(
                    p,
                    crate::schema::mysql_table_metadata_catalog(database, schema),
                    table,
                )
                .await
            } else {
                db::mysql::list_indexes(p, crate::schema::mysql_table_metadata_catalog(database, schema), table).await
            }
        }
        PoolKind::Postgres(p) if db_config.is_some_and(crate::schema::is_questdb_config) => {
            db::questdb::list_indexes(p, schema, table).await
        }
        PoolKind::Postgres(p) => db::postgres::list_indexes(p, schema, table).await,
        PoolKind::Sqlite(p) => db::sqlite::list_indexes(p, schema, table).await,
        PoolKind::Rqlite(client) => db::rqlite_driver::list_indexes(client, schema, table).await,
        PoolKind::MongoDb(client) => db::mongo_driver::list_indexes(client, database, table).await,
        PoolKind::CloudflareD1(client) => db::cloudflare_d1_driver::list_indexes(client, schema, table).await,
        _ => Ok(vec![]),
    }
}

/// Final native-pool dispatch for foreign keys.
pub(crate) async fn list_foreign_keys(
    state: &AppState,
    pool_key: &str,
    database: &str,
    schema: &str,
    table: &str,
) -> Result<Vec<db::ForeignKeyInfo>, String> {
    let connections = state.connections.read().await;
    let pool = connections.get(pool_key).ok_or("Pool not found")?;

    match pool {
        PoolKind::Mysql(p, mode) => {
            if *mode == MysqlMode::OceanBaseOracle {
                db::ob_oracle::list_foreign_keys(p, schema, table).await
            } else {
                db::mysql::list_foreign_keys(p, crate::schema::mysql_table_metadata_catalog(database, schema), table)
                    .await
            }
        }
        PoolKind::Postgres(p) => db::postgres::list_foreign_keys(p, schema, table).await,
        PoolKind::Sqlite(p) => db::sqlite::list_foreign_keys(p, schema, table).await,
        PoolKind::Rqlite(client) => db::rqlite_driver::list_foreign_keys(client, schema, table).await,
        PoolKind::CloudflareD1(client) => db::cloudflare_d1_driver::list_foreign_keys(client, schema, table).await,
        _ => Ok(vec![]),
    }
}

/// Final native-pool dispatch for triggers.
pub(crate) async fn list_triggers(
    state: &AppState,
    pool_key: &str,
    database: &str,
    schema: &str,
    table: &str,
) -> Result<Vec<db::TriggerInfo>, String> {
    let connections = state.connections.read().await;
    let pool = connections.get(pool_key).ok_or("Pool not found")?;

    match pool {
        PoolKind::Mysql(p, mode) => {
            if *mode == MysqlMode::OceanBaseOracle {
                db::ob_oracle::list_triggers(p, schema, table).await
            } else {
                db::mysql::list_triggers(p, crate::schema::mysql_table_metadata_catalog(database, schema), table).await
            }
        }
        PoolKind::Postgres(p) => db::postgres::list_triggers(p, schema, table).await,
        PoolKind::Sqlite(p) => db::sqlite::list_triggers(p, schema, table).await,
        PoolKind::Rqlite(client) => db::rqlite_driver::list_triggers(client, schema, table).await,
        PoolKind::CloudflareD1(client) => db::cloudflare_d1_driver::list_triggers(client, schema, table).await,
        _ => Ok(vec![]),
    }
}

/// Final native-pool dispatch for PostgreSQL-only schema extras.
pub(crate) async fn list_functions(
    state: &AppState,
    pool_key: &str,
    schema: &str,
) -> Result<Vec<db::FunctionInfo>, String> {
    let connections = state.connections.read().await;
    let pool = connections.get(pool_key).ok_or("Pool not found")?;
    match pool {
        PoolKind::Postgres(p) => db::postgres::list_functions(p, schema).await,
        _ => Ok(vec![]),
    }
}

pub(crate) async fn list_sequences(
    state: &AppState,
    pool_key: &str,
    schema: &str,
    with_last_values: bool,
) -> Result<Vec<db::SequenceInfo>, String> {
    let connections = state.connections.read().await;
    let pool = connections.get(pool_key).ok_or("Pool not found")?;
    match pool {
        PoolKind::Postgres(p) => db::postgres::list_sequences(p, schema, with_last_values).await,
        _ => Ok(vec![]),
    }
}

pub(crate) async fn list_rules(state: &AppState, pool_key: &str, schema: &str) -> Result<Vec<db::RuleInfo>, String> {
    let connections = state.connections.read().await;
    let pool = connections.get(pool_key).ok_or("Pool not found")?;
    match pool {
        PoolKind::Postgres(p) => db::postgres::list_rules(p, schema).await,
        _ => Ok(vec![]),
    }
}

pub(crate) async fn list_extensions(
    state: &AppState,
    pool_key: &str,
    schema: Option<&str>,
) -> Result<Vec<db::ExtensionInfo>, String> {
    let connections = state.connections.read().await;
    let pool = connections.get(pool_key).ok_or("Pool not found")?;
    match pool {
        PoolKind::Postgres(p) => db::postgres::list_extensions(p, schema).await,
        _ => Ok(vec![]),
    }
}

pub(crate) async fn list_available_extensions(
    state: &AppState,
    pool_key: &str,
) -> Result<Vec<db::ExtensionInfo>, String> {
    let connections = state.connections.read().await;
    let pool = connections.get(pool_key).ok_or("Pool not found")?;
    match pool {
        PoolKind::Postgres(p) => db::postgres::list_available_extensions(p).await,
        _ => Ok(vec![]),
    }
}

pub(crate) async fn list_owners(state: &AppState, pool_key: &str, schema: &str) -> Result<Vec<db::OwnerInfo>, String> {
    let connections = state.connections.read().await;
    let pool = connections.get(pool_key).ok_or("Pool not found")?;
    match pool {
        PoolKind::Postgres(p) => db::postgres::list_owners(p, schema).await,
        _ => Ok(vec![]),
    }
}

/// Final native-pool dispatch for table DDL.
pub(crate) async fn get_table_ddl(
    state: &AppState,
    pool_key: &str,
    db_config: Option<&ConnectionConfig>,
    database: &str,
    schema: &str,
    table: &str,
) -> Result<String, String> {
    let connections = state.connections.read().await;
    let pool = connections.get(pool_key).ok_or("Pool not found")?;

    match pool {
        PoolKind::Mysql(p, _) => {
            crate::schema::mysql_ddl(p, crate::schema::mysql_table_metadata_catalog(database, schema), table).await
        }
        PoolKind::Postgres(p) if db_config.is_some_and(crate::schema::is_opengauss_family_config) => {
            match crate::schema::opengauss_table_ddl(p, schema, table).await {
                Ok(ddl) => Ok(ddl),
                Err(_) => crate::schema::pg_ddl(p, schema, table).await,
            }
        }
        PoolKind::Postgres(p) if db_config.is_some_and(crate::schema::is_questdb_config) => {
            match db::questdb::questdb_table_or_view_ddl(p, table).await {
                Ok(ddl) => Ok(ddl),
                Err(_) => crate::schema::pg_ddl(p, schema, table).await,
            }
        }
        PoolKind::Postgres(p) => crate::schema::pg_ddl(p, schema, table).await,
        PoolKind::Sqlite(p) => crate::schema::sqlite_ddl(p, schema, table).await,
        PoolKind::Rqlite(client) => db::rqlite_driver::table_ddl(client, table).await,
        PoolKind::CloudflareD1(client) => db::cloudflare_d1_driver::table_ddl(client, table).await,
        _ => Err("DDL not supported for this database type".to_string()),
    }
}

/// Table comment lookup for native MySQL/Postgres (and related exclusions).
///
/// SqlServer / Oracle-agent special cases stay in `schema.rs` orchestration
/// (linked-server guard, agent SQL). Returns `Err` when the pool type is
/// unsupported so callers can surface the product error string.
pub(crate) async fn get_table_comment(
    state: &AppState,
    pool_key: &str,
    db_config: Option<&ConnectionConfig>,
    schema: &str,
    table: &str,
) -> Result<Option<String>, String> {
    let connections = state.connections.read().await;
    let pool = connections.get(pool_key).ok_or("Pool not found")?;
    match pool {
        PoolKind::Mysql(p, mode)
            if *mode != MysqlMode::OceanBaseOracle
                && !db_config.is_some_and(crate::schema::is_doris_family_config)
                && !db_config.is_some_and(crate::schema::is_manticoresearch_config) =>
        {
            db::mysql::get_table_comment(p, schema, table).await
        }
        PoolKind::Postgres(p) if !db_config.is_some_and(crate::schema::is_questdb_config) => {
            db::postgres::get_table_comment(p, schema, table).await
        }
        _ => Err("Table comment lookup is not supported for this connection".to_string()),
    }
}

/// Driver waterfall for completion-assistant catalog search.
///
/// Returns `Ok(Some(response))` when a pool/driver handled the request.
/// Returns `Ok(None)` when no matching driver exists or the agent method is
/// unsupported — callers should run `completion_assistant_fallback_core`.
pub(crate) async fn try_completion_assistant_search(
    state: &AppState,
    pool_key: &str,
    request: &db::CompletionAssistantRequest,
    db_config: Option<&ConnectionConfig>,
    #[cfg_attr(not(feature = "duckdb-bundled"), allow(unused_variables))] duckdb_attached_names: &[String],
) -> Result<Option<db::CompletionAssistantResponse>, String> {
    {
        let connections = state.connections.read().await;
        if let Some(client) = connections.get(pool_key).and_then(|pool| match pool {
            PoolKind::SqlServer(client) => Some(client.clone()),
            _ => None,
        }) {
            drop(connections);
            let mut client = client.lock().await;
            return Ok(Some(db::sqlserver::completion_assistant_search(&mut client, request).await?));
        }
    }

    {
        let connections = state.connections.read().await;
        if let Some(pool) = connections.get(pool_key).and_then(|pool| match pool {
            PoolKind::Sqlite(pool) => Some(pool.clone()),
            _ => None,
        }) {
            drop(connections);
            return Ok(Some(db::sqlite::completion_assistant_search(&pool, request).await?));
        }
    }

    #[cfg(feature = "duckdb-bundled")]
    {
        let connections = state.connections.read().await;
        if let Some(con) = connections.get(pool_key).and_then(|pool| match pool {
            PoolKind::DuckDb(con) => Some(con.clone()),
            _ => None,
        }) {
            drop(connections);
            let con = con.lock().map_err(|e| e.to_string())?;
            return Ok(Some(crate::schema::duckdb_completion_assistant_search(&con, request, duckdb_attached_names)?));
        }
    }

    {
        let connections = state.connections.read().await;
        if let Some(pool) = connections.get(pool_key).and_then(|pool| match pool {
            PoolKind::Postgres(pool) => Some(pool.clone()),
            _ => None,
        }) {
            drop(connections);
            return Ok(Some(db::postgres::completion_assistant_search(&pool, request).await?));
        }
    }

    {
        let connections = state.connections.read().await;
        if let Some(pool) = connections.get(pool_key).and_then(|pool| match pool {
            PoolKind::Mysql(pool, mode) if *mode != MysqlMode::OceanBaseOracle => Some(pool.clone()),
            _ => None,
        }) {
            drop(connections);
            return Ok(Some(db::mysql::completion_assistant_search(&pool, request).await?));
        }
    }

    {
        let connections = state.connections.read().await;
        if let Some(client) = connections.get(pool_key).and_then(|pool| match pool {
            PoolKind::Agent(client) => Some(client.clone()),
            _ => None,
        }) {
            drop(connections);
            let mut client = client.lock().await;
            match client
                .completion_assistant_search::<db::CompletionAssistantResponse>(
                    request,
                    crate::schema::agent_metadata_timeout(db_config),
                )
                .await
            {
                Ok(mut response) => {
                    response.fallback_used = false;
                    return Ok(Some(response));
                }
                Err(error) if crate::schema::is_agent_completion_assistant_unsupported(&error) => {
                    log::debug!(
                        "[schema][completion_assistant:agent-fallback] connection_id={} reason={}",
                        request.connection_id,
                        error
                    );
                }
                Err(error) => return Err(error),
            }
        }
    }

    Ok(None)
}

/// Dispatch for object source (view/function/procedure body text).
///
/// Owns ExternalDriver / Agent / SqlServer / native `PoolKind` matches so
/// `schema::get_object_source_once` stays orchestration-only (pool key + retry).
#[allow(clippy::too_many_arguments)]
pub(crate) async fn get_object_source(
    state: &AppState,
    pool_key: &str,
    db_config: Option<&ConnectionConfig>,
    database: &str,
    schema: &str,
    name: &str,
    object_type: db::ObjectSourceKind,
    signature: Option<&str>,
    #[cfg_attr(not(feature = "duckdb-bundled"), allow(unused_variables))] duckdb_attached_names: &[String],
) -> Result<db::ObjectSource, String> {
    use crate::models::connection::DatabaseType;
    use crate::query::should_discard_pool_after_error;

    let source = {
        let connections = state.connections.read().await;
        if let Some(PoolKind::ExternalDriver { config, session, .. }) = connections.get(pool_key) {
            let config = config.clone();
            let session = session.clone();
            drop(connections);
            let result: db::ObjectSource = session
                .invoke_with_timeout(
                    "getObjectSource",
                    serde_json::json!({
                        "connection": config.as_ref(),
                        "database": database,
                        "schema": schema,
                        "name": name,
                        "object_type": &object_type,
                    }),
                    crate::schema::agent_metadata_timeout(Some(config.as_ref())),
                )
                .await?;
            return Ok(result);
        }
        if let Some(client) = connections.get(pool_key).and_then(|pool| match pool {
            PoolKind::SqlServer(client) => Some(client.clone()),
            _ => None,
        }) {
            drop(connections);
            let mut client = client.lock().await;
            let result = db::sqlserver::execute_query(
                &mut client,
                &crate::schema::sqlserver_object_source_sql(schema, name, &object_type),
            )
            .await;
            drop(client);
            if matches!(
                result.as_ref(),
                Err(err) if should_discard_pool_after_error(Some(DatabaseType::SqlServer), err)
            ) {
                state.remove_pool_by_key(pool_key).await;
            }
            crate::schema::first_string_cell(result?)?
        } else if let Some(client) = connections.get(pool_key).and_then(|pool| match pool {
            PoolKind::Agent(client) => Some(client.clone()),
            _ => None,
        }) {
            drop(connections);
            if db_config.is_some_and(|config| config.db_type == DatabaseType::Oracle)
                && matches!(object_type, db::ObjectSourceKind::Package | db::ObjectSourceKind::PackageBody)
            {
                crate::schema::oracle_agent_object_source(
                    client,
                    database,
                    schema,
                    name,
                    &object_type,
                    crate::schema::agent_metadata_timeout(db_config),
                )
                .await?
            } else {
                let mut client = client.lock().await;
                let result: db::ObjectSource = client
                    .get_object_source(
                        database,
                        schema,
                        name,
                        &object_type,
                        crate::schema::agent_metadata_timeout(db_config),
                    )
                    .await?;
                return Ok(result);
            }
        } else {
            match connections.get(pool_key).ok_or("Pool not found")? {
                PoolKind::Mysql(pool, _) => {
                    crate::schema::mysql_object_source(
                        pool,
                        crate::schema::mysql_table_metadata_catalog(database, schema),
                        name,
                        &object_type,
                    )
                    .await?
                }
                PoolKind::Postgres(pool) if db_config.is_some_and(crate::schema::is_questdb_config) => {
                    // only view
                    db::questdb::questdb_object_source(pool, name).await?
                }
                PoolKind::Postgres(pool) => {
                    let unwrap_opengauss_record = db_config.is_some_and(crate::schema::is_opengauss_family_config);
                    crate::schema::postgres_object_source(
                        pool,
                        schema,
                        name,
                        &object_type,
                        signature,
                        unwrap_opengauss_record,
                    )
                    .await?
                }
                PoolKind::Sqlite(pool) => crate::schema::first_string_cell(
                    db::sqlite::execute_query(
                        pool,
                        &crate::schema::sqlite_object_source_sql(schema, name, &object_type),
                    )
                    .await?,
                )?,
                #[cfg(feature = "duckdb-bundled")]
                PoolKind::DuckDb(con) => {
                    let con = con.lock().map_err(|e| e.to_string())?;
                    crate::schema::duckdb_object_source_with_attached(
                        &con,
                        database,
                        schema,
                        name,
                        &object_type,
                        duckdb_attached_names,
                    )?
                }
                #[cfg(feature = "duckdb-bundled")]
                PoolKind::DuckDbWorker(client) => {
                    let client = client.clone();
                    let database = database.to_string();
                    let schema = schema.to_string();
                    let name = name.to_string();
                    let object_type = object_type.clone();
                    drop(connections);
                    client.get_object_source(database, schema, name, object_type).await?
                }
                PoolKind::Rqlite(client) => {
                    return db::rqlite_driver::object_source(client, name, &object_type).await;
                }
                PoolKind::ClickHouse(client) if matches!(object_type, db::ObjectSourceKind::View) => {
                    let result = db::clickhouse_driver::execute_query(
                        client,
                        database,
                        &format!("SHOW CREATE TABLE {}", crate::schema::mysql_ident(name)),
                    )
                    .await?;
                    crate::schema::first_string_cell(result)?
                }
                PoolKind::CloudflareD1(client) => {
                    return db::cloudflare_d1_driver::object_source(client, name, &object_type).await;
                }
                _ => return Err("Object source is not supported for this database type".to_string()),
            }
        }
    };

    Ok(db::ObjectSource {
        name: name.to_string(),
        object_type,
        schema: if schema.is_empty() { None } else { Some(schema.to_string()) },
        source,
        editable: None,
    })
}
