//! Transfer / export pool dispatch (Phase B3).
//!
//! Owns `PoolKind` matches for transfer SQL execution, transfer column lookup,
//! and native table-export streaming. Callers keep retries, cancellation, and
//! product orchestration.

use std::sync::atomic::AtomicBool;

use serde_json::Value;
use tokio_util::sync::CancellationToken;

use crate::connection::{AppState, MysqlMode, PoolKind};
use crate::db;
use crate::models::connection::DatabaseType;
use crate::query::{agent_execute_query_params, QueryExecutionOptions};
#[cfg(feature = "duckdb-bundled")]
use crate::sql::starts_with_executable_sql_keyword;

use super::traits::SqlExecute;

fn database_from_pool_key(pool_key: &str) -> Option<&str> {
    pool_key
        .split_once(":session:")
        .map(|(base, _)| base)
        .unwrap_or(pool_key)
        .split_once(':')
        .map(|(_, database)| database)
        .filter(|database| !database.is_empty())
}

/// Run SQL on a transfer/export pool (one attempt; caller owns retry policy).
///
/// Caller should run read-only checks before calling.
pub(crate) async fn execute_transfer_sql(
    state: &AppState,
    pool_key: &str,
    sql: &str,
    max_rows: Option<usize>,
) -> Result<db::QueryResult, String> {
    // B5 first-class families: resolve capability handles, then SqlExecute.
    if let Some(session) = super::resolve_mysql_session(state, pool_key).await? {
        return session.execute_with_max_rows(sql, max_rows).await;
    }
    if let Some(session) = super::resolve_postgres_session(state, pool_key).await? {
        return session.execute_with_max_rows(sql, max_rows).await;
    }

    let connections = state.connections.read().await;
    let pool = connections.get(pool_key).ok_or("Connection not found")?;

    match pool {
        // Mysql / Postgres handled above via SqlSession resolvers.
        PoolKind::Mysql(..) | PoolKind::Postgres(_) => {
            unreachable!("MySQL/Postgres transfer SQL should resolve via SqlSession")
        }
        PoolKind::Sqlite(p) => {
            let p = p.clone();
            drop(connections);
            db::sqlite::execute_query_with_max_rows(&p, sql, max_rows).await
        }
        PoolKind::ClickHouse(client) => {
            let client = client.clone();
            let database = database_from_pool_key(pool_key).unwrap_or("default").to_string();
            drop(connections);
            db::clickhouse_driver::execute_query_with_max_rows(&client, &database, sql, max_rows).await
        }
        PoolKind::SqlServer(client) => {
            let client = client.clone();
            drop(connections);
            let mut client = client.lock().await;
            let result = db::sqlserver::execute_query_with_max_rows(&mut client, sql, max_rows).await;
            drop(client);
            result
        }
        PoolKind::Agent(client) => {
            let client = client.clone();
            let database = database_from_pool_key(pool_key).map(str::to_string);
            let sql = sql.to_string();
            drop(connections);
            let mut client = client.lock().await;
            let params = agent_execute_query_params(
                &sql,
                database.as_deref(),
                None,
                QueryExecutionOptions { max_rows, fetch_size: max_rows, ..QueryExecutionOptions::default() },
            );
            client.execute_query(params).await
        }
        #[cfg(feature = "duckdb-bundled")]
        PoolKind::DuckDb(con) => {
            let con = con.clone();
            let sql = sql.to_string();
            drop(connections);
            tokio::task::spawn_blocking(move || {
                let con = con.lock().map_err(|e| e.to_string())?;
                if max_rows.is_some()
                    && starts_with_executable_sql_keyword(&sql, &["SELECT", "SHOW", "DESCRIBE", "WITH", "PRAGMA"])
                {
                    return crate::query::duckdb_execute_with_max_rows(&con, &sql, max_rows);
                }
                let start = std::time::Instant::now();
                if starts_with_executable_sql_keyword(&sql, &["SELECT", "SHOW", "DESCRIBE", "WITH", "PRAGMA"]) {
                    let mut stmt = con.prepare(&sql).map_err(|e| e.to_string())?;
                    let mut rows = stmt.query([]).map_err(|e| e.to_string())?;
                    let stmt_ref = rows.as_ref().ok_or("DuckDB statement unavailable")?;
                    let col_count = stmt_ref.column_count();
                    let columns: Vec<String> = (0..col_count)
                        .map(|i| stmt_ref.column_name(i).map(|s| s.to_string()).unwrap_or_else(|_| "?".to_string()))
                        .collect();
                    let mut result_rows = Vec::new();
                    while let Some(row) = rows.next().map_err(|e| e.to_string())? {
                        let vals: Vec<serde_json::Value> = (0..col_count)
                            .map(|i| {
                                row.get::<_, String>(i)
                                    .map(serde_json::Value::String)
                                    .or_else(|_| row.get::<_, i64>(i).map(|v| serde_json::Value::Number(v.into())))
                                    .or_else(|_| {
                                        row.get::<_, f64>(i).map(|v| {
                                            serde_json::Number::from_f64(v)
                                                .map(serde_json::Value::Number)
                                                .unwrap_or(serde_json::Value::Null)
                                        })
                                    })
                                    .or_else(|_| row.get::<_, bool>(i).map(serde_json::Value::Bool))
                                    .unwrap_or(serde_json::Value::Null)
                            })
                            .collect();
                        result_rows.push(vals);
                    }
                    Ok(db::QueryResult {
                        columns,
                        column_types: Vec::new(),
                        column_sortables: vec![],
                        rows: result_rows,
                        affected_rows: 0,
                        execution_time_ms: start.elapsed().as_millis(),
                        truncated: false,
                        session_id: None,
                        has_more: false,
                    })
                } else {
                    let affected = con.execute(&sql, []).map_err(|e| e.to_string())?;
                    Ok(db::QueryResult {
                        columns: vec![],
                        column_types: Vec::new(),
                        column_sortables: vec![],
                        rows: vec![],
                        affected_rows: affected as u64,
                        execution_time_ms: start.elapsed().as_millis(),
                        truncated: false,
                        session_id: None,
                        has_more: false,
                    })
                }
            })
            .await
            .map_err(|e| e.to_string())?
        }
        #[cfg(feature = "duckdb-bundled")]
        PoolKind::ExternalTabular(ext_pool) => {
            let con = ext_pool.cache.clone();
            let sql = sql.to_string();
            drop(connections);
            tokio::task::spawn_blocking(move || {
                let con = con.lock().map_err(|e| e.to_string())?;
                crate::query::duckdb_execute_with_max_rows(&con, &sql, max_rows)
            })
            .await
            .map_err(|e| e.to_string())?
        }
        _ => Err("Unsupported database type for transfer".to_string()),
    }
}

/// Resolve columns for a transfer source/target table.
pub(crate) async fn get_columns_for_transfer(
    state: &AppState,
    pool_key: &str,
    database: &str,
    schema: &str,
    table: &str,
) -> Result<Vec<db::ColumnInfo>, String> {
    let connections = state.connections.read().await;

    #[cfg(feature = "duckdb-bundled")]
    if let Some(PoolKind::DuckDb(con)) = connections.get(pool_key) {
        let con = con.clone();
        drop(connections);
        let table = table.to_string();
        let schema = schema.to_string();
        return tokio::task::spawn_blocking(move || {
            let con = con.lock().map_err(|e| e.to_string())?;
            crate::schema::duckdb_query_columns_in_database(&con, "main", &schema, &table)
        })
        .await
        .map_err(|e| e.to_string())?;
    }

    #[cfg(feature = "duckdb-bundled")]
    if let Some(PoolKind::ExternalTabular(ext_pool)) = connections.get(pool_key) {
        let con = ext_pool.cache.clone();
        drop(connections);
        let table = table.to_string();
        let schema = schema.to_string();
        return tokio::task::spawn_blocking(move || {
            let con = con.lock().map_err(|e| e.to_string())?;
            crate::schema::duckdb_query_columns_in_database(&con, "main", &schema, &table)
        })
        .await
        .map_err(|e| e.to_string())?;
    }

    if let Some(PoolKind::ClickHouse(client)) = connections.get(pool_key) {
        let client = client.clone();
        let database = database.to_string();
        let table = table.to_string();
        drop(connections);
        return db::clickhouse_driver::get_columns(&client, &database, &table).await;
    }
    if let Some(PoolKind::SqlServer(client)) = connections.get(pool_key) {
        let client = client.clone();
        let schema = schema.to_string();
        let table = table.to_string();
        drop(connections);
        let mut client = client.lock().await;
        return db::sqlserver::get_columns(&mut client, &schema, &table).await;
    }
    if let Some(PoolKind::InfluxDb(client)) = connections.get(pool_key) {
        let client = client.clone();
        let database = database.to_string();
        let table = table.to_string();
        drop(connections);
        return db::influxdb_driver::get_columns(&client, &database, &table).await;
    }
    if let Some(PoolKind::Agent(client)) = connections.get(pool_key) {
        let client = client.clone();
        let database = database.to_string();
        let schema = schema.to_string();
        let table = table.to_string();
        drop(connections);
        let mut client = client.lock().await;
        return client.get_columns(&database, &schema, &table, None).await;
    }
    let pool = connections.get(pool_key).ok_or("Pool not found")?;
    let schema = schema.to_string();
    let table = table.to_string();
    match pool {
        PoolKind::Mysql(p, _) => {
            let p = p.clone();
            drop(connections);
            db::mysql::get_columns(&p, &schema, &table).await
        }
        PoolKind::Postgres(p) => {
            let p = p.clone();
            drop(connections);
            db::postgres::get_columns(&p, &schema, &table).await
        }
        PoolKind::Sqlite(p) => {
            let p = p.clone();
            drop(connections);
            db::sqlite::get_columns(&p, &schema, &table).await
        }
        _ => Err("Unsupported database type".to_string()),
    }
}

/// Stream native table-export rows for MySQL / Postgres / SqlServer.
///
/// Returns `Ok(false)` when the pool is not a supported streaming driver
/// (caller falls back to paginated export).
pub(crate) async fn stream_native_table_rows(
    state: &AppState,
    pool_key: &str,
    db_type: &DatabaseType,
    sql: &str,
    row_limit: Option<usize>,
    cancelled: &AtomicBool,
    cancel_token: CancellationToken,
    on_row: impl FnMut(&[Value]) -> Result<(), String>,
) -> Result<bool, String> {
    let connections = state.connections.read().await;
    match connections.get(pool_key) {
        Some(PoolKind::Mysql(pool, mode)) => {
            let pool = pool.clone();
            let bare = *mode == MysqlMode::Bare;
            drop(connections);
            crate::db::mysql::stream_query_rows(
                &pool,
                sql,
                bare,
                row_limit,
                crate::db::mysql::MySqlQueryDialect::for_connection(*db_type, None),
                cancelled,
                on_row,
            )
            .await?;
            Ok(true)
        }
        Some(PoolKind::Postgres(pool)) => {
            let pool = pool.clone();
            drop(connections);
            crate::db::postgres::stream_query_rows(&pool, sql, row_limit, cancelled, on_row).await?;
            Ok(true)
        }
        Some(PoolKind::SqlServer(client)) => {
            let client = client.clone();
            drop(connections);
            let mut on_row = on_row;
            let mut client = client.lock().await;
            crate::db::sqlserver::stream_first_result_set(&mut client, sql, row_limit, Some(cancel_token), |item| {
                if let crate::db::sqlserver::SqlServerStreamItem::Row(row) = item {
                    on_row(row)?;
                }
                Ok(())
            })
            .await?;
            Ok(true)
        }
        _ => Ok(false),
    }
}
