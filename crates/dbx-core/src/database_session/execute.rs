//! Database session execute path (Phase B1).
//!
//! Owns the `PoolKind` dispatch for SQL execution. Callers (`query::do_execute`)
//! keep orchestration: budgets, stage logs, read-only checks.

use std::time::{Duration, Instant};

use tokio_util::sync::CancellationToken;

use crate::connection::{AppState, PoolKind};
use crate::connection_lifecycle;
use crate::db;
use crate::models::connection::DatabaseType;
use crate::query::DbOperationBudget;
use crate::query::{
    agent_execute_query_page_params, agent_execute_query_params, agent_fetch_query_page_params,
    apply_oceanbase_mysql_session_timeout, canceled_error, external_driver_fetch_query_page_params,
    external_driver_query_params, invoke_external_driver_query_page, is_canceled, schema_for_execution_context,
    should_discard_pool_after_error, sql_for_execution_context, truncate_result_with_max_rows, wait_for_query_opt,
    QueryExecutionOptions, MAX_ROWS, QUERY_CANCELED,
};

#[cfg(feature = "duckdb-bundled")]
use crate::query::is_dbx_query_timeout_error;
#[cfg(feature = "duckdb-bundled")]
use crate::sql::starts_with_duckdb_result_sql_keyword;

#[cfg(feature = "duckdb-bundled")]
use crate::query::{
    duckdb_draining_error, duckdb_execute_for_database, duckdb_execute_with_max_rows,
    wait_for_duckdb_task_with_interrupt, wait_for_duckdb_task_with_interrupt_outcome, DuckDbTaskWait,
};

/// Error from session execute. `AlreadyLogged` means stage logs were emitted
/// inside the driver arm (preserve Phase A single end-log semantics).
pub(crate) enum ExecuteSqlError {
    AlreadyLogged(String),
    Unlogged(String),
}

impl From<String> for ExecuteSqlError {
    fn from(value: String) -> Self {
        Self::Unlogged(value)
    }
}

impl ExecuteSqlError {
    pub fn already_logged(&self) -> bool {
        matches!(self, Self::AlreadyLogged(_))
    }

    pub fn as_str(&self) -> &str {
        match self {
            Self::AlreadyLogged(m) | Self::Unlogged(m) => m.as_str(),
        }
    }

    pub fn into_message(self) -> String {
        match self {
            Self::AlreadyLogged(m) | Self::Unlogged(m) => m,
        }
    }
}

/// Run SQL against the pool registered at `pool_key`.
///
/// Caller is responsible for stage logging around this call (start/done/error),
/// except when [`ExecuteSqlError::AlreadyLogged`] is returned.
pub(crate) async fn execute_sql(
    state: &AppState,
    pool_key: &str,
    mysql_dialect: db::mysql::MySqlQueryDialect,
    database: Option<&str>,
    sql: &str,
    schema: Option<&str>,
    cancel_token: Option<CancellationToken>,
    options: &QueryExecutionOptions,
    query_timeout: Option<Duration>,
    operation_budget: DbOperationBudget,
    pool_db_type: Option<DatabaseType>,
    #[cfg_attr(not(feature = "duckdb-bundled"), allow(unused_variables))] duckdb_attached_names: Vec<String>,
    execute_started: Instant,
    execute_log_context: connection_lifecycle::StageLogContext<'_>,
    db_type_label: Option<String>,
) -> Result<db::QueryResult, ExecuteSqlError> {
    let connections = state.connections.read().await;
    let pool = match connections.get(pool_key) {
        Some(pool) => pool,
        None => {
            return Err(ExecuteSqlError::Unlogged("Connection not found".to_string()));
        }
    };

    let result = match pool {
        #[cfg(feature = "duckdb-bundled")]
        PoolKind::DuckDb(con) => {
            let con = con.clone();
            if con.is_draining() {
                drop(connections);
                let error = duckdb_draining_error();
                connection_lifecycle::log_stage(
                    connection_lifecycle::StageLog::new(
                        connection_lifecycle::LifecycleStage::QueryExecute,
                        connection_lifecycle::StageOutcome::Error,
                        execute_started.elapsed().as_millis(),
                    )
                    .with_context(execute_log_context)
                    .with_error(&error),
                );
                return Err(ExecuteSqlError::AlreadyLogged(error));
            }
            let interrupt_handle = con.interrupt_handle();
            if let Some(ref execution_id) = options.execution_id {
                let cancel_interrupt_handle = interrupt_handle.clone();
                state.running_queries.register_interrupt(execution_id, move || {
                    cancel_interrupt_handle.interrupt();
                });
            }
            let sql = sql.to_string();
            let database = database.map(str::to_string);
            let attached_names = duckdb_attached_names;
            let max_rows = options.max_rows;
            drop(connections);
            let task_con = con.clone();
            let task = tokio::task::spawn_blocking(move || {
                let con = task_con.lock().map_err(|e| e.to_string())?;
                duckdb_execute_for_database(&con, &attached_names, database.as_deref(), &sql, max_rows)
            });
            let result =
                wait_for_duckdb_task_with_interrupt_outcome(cancel_token, query_timeout, interrupt_handle, task).await;
            match result {
                DuckDbTaskWait::Finished(result) => {
                    if matches!(result.as_ref(), Err(err) if err == QUERY_CANCELED || is_dbx_query_timeout_error(&err.to_lowercase()))
                    {
                        con.mark_draining();
                        state.spawn_duckdb_pool_cleanup(pool_key.to_string(), con);
                    }
                    result
                }
                DuckDbTaskWait::Draining { error, task } => {
                    con.mark_draining();
                    state.spawn_duckdb_draining_cleanup(pool_key.to_string(), con, task);
                    Err(error)
                }
            }
        }
        #[cfg(feature = "duckdb-bundled")]
        PoolKind::DuckDbWorker(client) => {
            let client = client.clone();
            if let Some(ref execution_id) = options.execution_id {
                let cancel_client = client.clone();
                state.running_queries.register_interrupt(execution_id, move || {
                    let cancel_client = cancel_client.clone();
                    tokio::spawn(async move {
                        if let Err(error) = cancel_client.cancel().await {
                            log::warn!("Failed to cancel DuckDB worker query: {error}");
                        }
                    });
                });
            }
            let sql = sql.to_string();
            let database = database.map(str::to_string);
            let max_rows = options.max_rows;
            drop(connections);
            client.execute(database, sql, max_rows, cancel_token, query_timeout).await
        }
        #[cfg(not(feature = "duckdb-bundled"))]
        PoolKind::DuckDb(_) => {
            let error = "DuckDB support is not compiled in this build".to_string();
            connection_lifecycle::log_stage(
                connection_lifecycle::StageLog::new(
                    connection_lifecycle::LifecycleStage::QueryExecute,
                    connection_lifecycle::StageOutcome::Error,
                    execute_started.elapsed().as_millis(),
                )
                .with_context(execute_log_context)
                .with_error(&error),
            );
            return Err(ExecuteSqlError::AlreadyLogged(error));
        }
        #[cfg(not(feature = "duckdb-bundled"))]
        PoolKind::DuckDbWorker(_) => {
            let error = "DuckDB worker support is not compiled in this build".to_string();
            connection_lifecycle::log_stage(
                connection_lifecycle::StageLog::new(
                    connection_lifecycle::LifecycleStage::QueryExecute,
                    connection_lifecycle::StageOutcome::Error,
                    execute_started.elapsed().as_millis(),
                )
                .with_context(execute_log_context)
                .with_error(&error),
            );
            return Err(ExecuteSqlError::AlreadyLogged(error));
        }
        PoolKind::Mysql(p, mode) => {
            let p = p.clone();
            let bare = *mode == crate::connection::MysqlMode::Bare;
            let max_rows = options.max_rows;
            drop(connections);
            let mut conn = match db::mysql::get_conn_with_health_check_with_cancel_logged(
                &p,
                operation_budget.checkout_timeout,
                operation_budget.cleanup_timeout,
                cancel_token.as_ref(),
                execute_log_context,
            )
            .await
            {
                Ok(conn) => conn,
                Err(err) if err == QUERY_CANCELED => {
                    state.remove_pool_by_key(pool_key).await;
                    connection_lifecycle::log_stage(
                        connection_lifecycle::StageLog::new(
                            connection_lifecycle::LifecycleStage::QueryExecute,
                            connection_lifecycle::StageOutcome::Cancelled,
                            execute_started.elapsed().as_millis(),
                        )
                        .with_context(execute_log_context),
                    );
                    return Err(ExecuteSqlError::AlreadyLogged(err));
                }
                Err(err) => {
                    connection_lifecycle::log_stage(
                        connection_lifecycle::StageLog::new(
                            connection_lifecycle::LifecycleStage::QueryExecute,
                            connection_lifecycle::StageOutcome::Error,
                            execute_started.elapsed().as_millis(),
                        )
                        .with_context(execute_log_context)
                        .with_error(&err),
                    );
                    return Err(ExecuteSqlError::AlreadyLogged(err));
                }
            };
            let connection_id = conn.id();
            if let Some(ref execution_id) = options.execution_id {
                let kill_opts = conn.opts().clone();
                let kill_pool_key = pool_key.to_string();
                let kill_trace_id = execution_id.clone();
                let kill_db_type = db_type_label.clone();
                state.running_queries.register_interrupt(execution_id, move || {
                    let kill_opts = kill_opts.clone();
                    let kill_pool_key = kill_pool_key.clone();
                    let kill_trace_id = kill_trace_id.clone();
                    let kill_db_type = kill_db_type.clone();
                    tokio::spawn(async move {
                        let log_context = connection_lifecycle::StageLogContext::for_pool(
                            Some(kill_pool_key.as_str()),
                            Some(kill_trace_id.as_str()),
                            kill_db_type.as_deref(),
                        );
                        if let Err(error) =
                            db::mysql::kill_query_with_opts_logged(kill_opts, connection_id, log_context).await
                        {
                            log::warn!("Failed to cancel MySQL query {connection_id}: {error}");
                        }
                    });
                });
            }
            apply_oceanbase_mysql_session_timeout(state, pool_key, &mut conn, options.timeout_secs).await?;
            wait_for_query_opt(
                cancel_token,
                query_timeout,
                db::mysql::execute_query_on_conn_with_max_rows(&mut conn, sql, bare, max_rows, mysql_dialect),
            )
            .await
        }
        PoolKind::Postgres(p) => {
            let p = p.clone();
            let schema = schema.map(|s| s.to_string());
            let max_rows = options.max_rows;
            let cancel_context = state.get_postgres_cancel_context(pool_key).await;
            // Owned label so openGauss/Redshift/etc. are not all logged as "postgres".
            let db_type_label = connection_lifecycle::optional_database_type_log_label(pool_db_type);
            let mut log_context = connection_lifecycle::StageLogContext::for_pool(
                Some(pool_key),
                options.execution_id.as_deref(),
                db_type_label.as_deref(),
            );
            if let Some(ref client_session_id) = options.client_session_id {
                log_context.client_session_id = Some(client_session_id.as_str());
            }
            drop(connections);
            if let Some(schema) = schema {
                db::postgres::execute_query_with_schema_and_max_rows_and_cancel_logged(
                    &p,
                    &schema,
                    sql,
                    max_rows,
                    cancel_token,
                    operation_budget.clone(),
                    cancel_context,
                    log_context,
                )
                .await
            } else {
                db::postgres::execute_query_with_max_rows_and_cancel_logged(
                    &p,
                    sql,
                    max_rows,
                    cancel_token,
                    operation_budget.clone(),
                    cancel_context,
                    log_context,
                )
                .await
            }
        }
        PoolKind::Sqlite(p) => {
            let p = p.clone();
            let max_rows = options.max_rows;
            drop(connections);
            wait_for_query_opt(cancel_token, query_timeout, db::sqlite::execute_query_with_max_rows(&p, sql, max_rows))
                .await
        }
        PoolKind::Rqlite(client) => {
            let client = client.clone();
            let max_rows = options.max_rows;
            drop(connections);
            wait_for_query_opt(
                cancel_token,
                query_timeout,
                db::rqlite_driver::execute_query_with_max_rows(&client, sql, max_rows),
            )
            .await
        }
        PoolKind::Turso(client) => {
            let client = client.clone();
            let max_rows = options.max_rows;
            drop(connections);
            wait_for_query_opt(
                cancel_token,
                query_timeout,
                db::turso_driver::execute_query_with_max_rows(&client, sql, max_rows),
            )
            .await
        }
        PoolKind::CloudflareD1(client) => {
            let client = client.clone();
            let max_rows = options.max_rows;
            drop(connections);
            wait_for_query_opt(
                cancel_token,
                query_timeout,
                db::cloudflare_d1_driver::execute_query_with_max_rows(&client, sql, max_rows),
            )
            .await
        }
        PoolKind::ClickHouse(client) => {
            let client = client.clone();
            let database = pool_key.split(':').nth(1).unwrap_or("default").to_string();
            let max_rows = options.max_rows;
            drop(connections);
            let result = wait_for_query_opt(
                cancel_token,
                query_timeout,
                db::clickhouse_driver::execute_query_with_max_rows(&client, &database, sql, max_rows),
            )
            .await
            .map(|result| truncate_result_with_max_rows(result, max_rows));
            if matches!(result.as_ref(), Err(err) if should_discard_pool_after_error(pool_db_type, err)) {
                state.remove_pool_by_key(pool_key).await;
            }
            result
        }
        PoolKind::SqlServer(client) => {
            let client = client.clone();
            let max_rows = options.max_rows;
            drop(connections);
            let mut client = match cancel_token.as_ref() {
                Some(token) => tokio::select! {
                    biased;
                    _ = token.cancelled() => {
                        connection_lifecycle::log_stage(
                            connection_lifecycle::StageLog::new(
                                connection_lifecycle::LifecycleStage::QueryExecute,
                                connection_lifecycle::StageOutcome::Cancelled,
                                execute_started.elapsed().as_millis(),
                            )
                            .with_context(execute_log_context),
                        );
                        return Err(ExecuteSqlError::AlreadyLogged(canceled_error()));
                    }
                    guard = client.lock() => guard,
                },
                None => client.lock().await,
            };
            let result = wait_for_query_opt(
                cancel_token,
                query_timeout,
                db::sqlserver::execute_query_with_max_rows(&mut client, sql, max_rows),
            )
            .await
            .map(|result| truncate_result_with_max_rows(result, max_rows));
            drop(client);
            if matches!(result.as_ref(), Err(err) if should_discard_pool_after_error(pool_db_type, err)) {
                state.remove_pool_by_key(pool_key).await;
            }
            result
        }
        PoolKind::Elasticsearch(client) => {
            let client = client.clone();
            let sql = sql.to_string();
            let max_rows = options.max_rows;
            drop(connections);
            let result = wait_for_query_opt(
                cancel_token,
                query_timeout,
                db::elasticsearch_driver::execute_rest_query(&client, &sql),
            )
            .await
            .map(|result| truncate_result_with_max_rows(result, max_rows));
            if matches!(result.as_ref(), Err(err) if should_discard_pool_after_error(pool_db_type, err)) {
                state.remove_pool_by_key(pool_key).await;
            }
            result
        }
        PoolKind::VectorDb(client) => {
            let client = client.clone();
            let sql = sql.to_string();
            let max_rows = options.max_rows;
            drop(connections);
            let result =
                wait_for_query_opt(cancel_token, query_timeout, db::vector_driver::execute_rest_query(&client, &sql))
                    .await
                    .map(|result| truncate_result_with_max_rows(result, max_rows));
            if matches!(result.as_ref(), Err(err) if should_discard_pool_after_error(pool_db_type, err)) {
                state.remove_pool_by_key(pool_key).await;
            }
            result
        }
        PoolKind::Redis(_) => Err("Use Redis-specific commands".to_string()),
        PoolKind::MongoDb(_) => Err("Use MongoDB-specific commands".to_string()),
        PoolKind::MessageQueue => Err("Use Message Queue-specific commands".to_string()),
        PoolKind::Nacos => Err("Use Nacos-specific commands".to_string()),
        PoolKind::InfluxDb(client) => {
            let client = client.clone();
            let database = pool_key.split(':').nth(1).unwrap_or("default").to_string();
            let max_rows = options.max_rows;
            drop(connections);
            let result = wait_for_query_opt(
                cancel_token,
                query_timeout,
                db::influxdb_driver::execute_query(&client, &database, sql),
            )
            .await
            .map(|result| truncate_result_with_max_rows(result, max_rows));
            if matches!(result.as_ref(), Err(err) if should_discard_pool_after_error(pool_db_type, err)) {
                state.remove_pool_by_key(pool_key).await;
            }
            result
        }
        PoolKind::Agent(client) => {
            let client = client.clone();
            let sql = sql_for_execution_context(pool_db_type, sql, schema);
            let database = database.map(|s| s.to_string());
            let schema = schema_for_execution_context(pool_db_type, schema).map(|s| s.to_string());
            let max_rows = options.max_rows;
            let rpc_timeout = query_timeout;
            drop(connections);
            if is_canceled(&cancel_token) {
                connection_lifecycle::log_stage(
                    connection_lifecycle::StageLog::new(
                        connection_lifecycle::LifecycleStage::QueryExecute,
                        connection_lifecycle::StageOutcome::Cancelled,
                        execute_started.elapsed().as_millis(),
                    )
                    .with_context(execute_log_context),
                );
                return Err(ExecuteSqlError::AlreadyLogged(canceled_error()));
            }
            let cancel_for_agent = cancel_token.clone();
            let options = options.clone();
            let result = async move {
                let mut client = match cancel_for_agent.as_ref() {
                    Some(token) => {
                        tokio::select! {
                            biased;
                            _ = token.cancelled() => return Err(canceled_error()),
                            guard = client.lock() => guard,
                        }
                    }
                    None => client.lock().await,
                };
                if let Some(session_id) = options.result_session_id.as_deref() {
                    let params = agent_fetch_query_page_params(session_id, options.page_size.unwrap_or(MAX_ROWS));
                    client.fetch_query_page_with_timeout_and_cancel(params, rpc_timeout, cancel_for_agent.clone()).await
                } else if options.page_size.is_some() {
                    let params =
                        agent_execute_query_page_params(&sql, database.as_deref(), schema.as_deref(), options.clone());
                    client
                        .execute_query_page_with_timeout_and_cancel(params, rpc_timeout, cancel_for_agent.clone())
                        .await
                } else {
                    let params =
                        agent_execute_query_params(&sql, database.as_deref(), schema.as_deref(), options.clone());
                    client.execute_query_with_timeout_and_cancel(params, rpc_timeout, cancel_for_agent.clone()).await
                }
            }
            .await
            .map(|result| truncate_result_with_max_rows(result, max_rows));
            if matches!(result.as_ref(), Err(err) if err == QUERY_CANCELED) {
                state.remove_pool_by_key(pool_key).await;
            }
            if matches!(result.as_ref(), Err(err) if should_discard_pool_after_error(pool_db_type, err)) {
                state.remove_pool_by_key(pool_key).await;
            }
            result
        }
        #[cfg(feature = "duckdb-bundled")]
        PoolKind::ExternalTabular(ext_pool) => {
            if !starts_with_duckdb_result_sql_keyword(sql) {
                return Err(ExecuteSqlError::Unlogged(
                    "External data sources are read-only. Only SELECT queries are supported.".to_string(),
                ));
            }
            let con = ext_pool.cache.clone();
            let interrupt_handle = con.lock().map_err(|e| e.to_string())?.interrupt_handle();
            if let Some(ref execution_id) = options.execution_id {
                let cancel_interrupt_handle = interrupt_handle.clone();
                state.running_queries.register_interrupt(execution_id, move || {
                    cancel_interrupt_handle.interrupt();
                });
            }
            let sql = sql.to_string();
            let max_rows = options.max_rows;
            drop(connections);
            let task = tokio::task::spawn_blocking(move || {
                let con = con.lock().map_err(|e| e.to_string())?;
                duckdb_execute_with_max_rows(&con, &sql, max_rows)
            });
            wait_for_duckdb_task_with_interrupt(cancel_token, query_timeout, interrupt_handle, task).await
        }
        #[cfg(not(feature = "duckdb-bundled"))]
        PoolKind::ExternalTabular(_) => {
            Err("External data sources require DuckDB support. Rebuild with default features.".to_string())
        }
        PoolKind::ExternalDriver { config, session, .. } => {
            let config = config.clone();
            let session = session.clone();
            let sql = sql.to_string();
            let schema = schema.map(str::to_string);
            let database = database.unwrap_or_else(|| config.effective_database().unwrap_or("")).to_string();
            let max_rows = options.max_rows;
            let plugin_timeout = query_timeout;
            let options = options.clone();
            drop(connections);
            wait_for_query_opt(cancel_token, query_timeout, async move {
                if let Some(session_id) = options.result_session_id.as_deref() {
                    let params = external_driver_fetch_query_page_params(
                        config.as_ref(),
                        session_id,
                        options.page_size.unwrap_or(MAX_ROWS),
                    );
                    session.invoke_with_timeout::<db::QueryResult>("fetchQueryPage", params, plugin_timeout).await
                } else if options.page_size.is_some() {
                    let params =
                        external_driver_query_params(config.as_ref(), &sql, &database, schema.as_deref(), &options);
                    invoke_external_driver_query_page(session.as_ref(), params, plugin_timeout).await
                } else {
                    let params =
                        external_driver_query_params(config.as_ref(), &sql, &database, schema.as_deref(), &options);
                    session.invoke_with_timeout::<db::QueryResult>("executeQuery", params, plugin_timeout).await
                }
            })
            .await
            .map(|result| truncate_result_with_max_rows(result, max_rows))
        }
    };

    result.map_err(ExecuteSqlError::Unlogged)
}
