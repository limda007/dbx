//! Connect / test_connection orchestration (PR-A3).
//!
//! Owns driver dispatch for desktop and web adapters. Tauri/web commands
//! call these entry points and do not host `match config.db_type` for connect.

use std::sync::Arc;
use std::time::Instant;

use crate::agent_connection::{
    agent_connect_params, mongo_legacy_error_with_auth_hint, mongo_uses_legacy_driver, oracle_alternate_connect_config,
    oracle_error_with_driver_hint, should_retry_mongo_with_legacy_driver,
};
use crate::connection::{
    agent_connect_timeout, connect_bare_metadata_pool, connect_mysql_metadata_pool, connection_url_for_endpoint,
    metadata_connection_config, prestosql_jdbc_config_for_endpoint, probe_connection_endpoint,
    redacted_connection_url_for_endpoint, AppState, MysqlMode, PoolKind,
};
use crate::connection_lifecycle::{
    database_type_log_label, log_stage, LifecycleStage, StageLog, StageLogContext, StageOutcome,
};
use crate::database_capabilities;
use crate::db;
use crate::db::agent_driver::AgentMethod;
use crate::models::connection::{
    database_info_from_protocol_value, rewrite_jdbc_url_host, ConnectionConfig, ConnectionTestResult,
    DatabaseConnectionInfo, DatabaseType,
};
use crate::path_utils::expand_tilde;

pub const MONGO_LEGACY_DRIVER_PROFILE: &str = "mongodb-legacy";
pub const MONGO_LEGACY_DRIVER_LABEL: &str = "MongoDB (Legacy)";

pub fn mongo_legacy_connect_params(config: &ConnectionConfig, host: &str, port: u16) -> serde_json::Value {
    serde_json::json!({
        "connection": agent_connect_params(config, host, port, config.effective_database().unwrap_or(""))
    })
}

pub fn mark_mongo_legacy_driver(config: &mut ConnectionConfig) -> bool {
    if config.db_type != DatabaseType::MongoDb {
        return false;
    }
    let changed = config.driver_profile.as_deref() != Some(MONGO_LEGACY_DRIVER_PROFILE)
        || config.driver_label.as_deref() != Some(MONGO_LEGACY_DRIVER_LABEL);
    config.driver_profile = Some(MONGO_LEGACY_DRIVER_PROFILE.to_string());
    config.driver_label = Some(MONGO_LEGACY_DRIVER_LABEL.to_string());
    changed
}

async fn persist_mongo_legacy_driver_profile(state: &AppState, config: &ConnectionConfig) -> Result<(), String> {
    if config.one_time {
        return Ok(());
    }

    let mut configs: Vec<ConnectionConfig> =
        state.storage.load_connections().await?.into_iter().map(|config| config.canonicalized()).collect();
    let Some(saved_config) = configs.iter_mut().find(|saved_config| saved_config.id == config.id) else {
        return Ok(());
    };
    if !mark_mongo_legacy_driver(saved_config) {
        return Ok(());
    }
    state.storage.save_connections(&configs).await?;
    // Keep runtime cache aligned when the profile is permanently upgraded.
    state.configs.write().await.insert(config.id.clone(), config.clone());
    Ok(())
}

async fn test_agent_connection(
    state: &AppState,
    config: &ConnectionConfig,
    host: &str,
    port: u16,
) -> Result<ConnectionTestResult, String> {
    let connect_params = agent_connect_params(config, host, port, config.database.as_deref().unwrap_or(""));
    let result = state
        .agent_manager
        .call_daemon_method_with_timeout::<serde_json::Value>(
            &config.db_type,
            config.driver_profile.as_deref(),
            AgentMethod::TestConnection,
            connect_params,
            Some(agent_connect_timeout(config)),
        )
        .await;

    let response = match result {
        Ok(response) => response,
        Err(err) => {
            if let Some(alternate_config) = oracle_alternate_connect_config(config, &err) {
                state
                    .agent_manager
                    .call_daemon_method_with_timeout::<serde_json::Value>(
                        &alternate_config.db_type,
                        alternate_config.driver_profile.as_deref(),
                        AgentMethod::TestConnection,
                        agent_connect_params(
                            &alternate_config,
                            host,
                            port,
                            alternate_config.database.as_deref().unwrap_or(""),
                        ),
                        Some(agent_connect_timeout(&alternate_config)),
                    )
                    .await
                    .map_err(|alternate_err| {
                        format!("{err}\n\nFallback with alternate Oracle descriptor failed: {alternate_err}")
                    })?
            } else {
                return Err(oracle_error_with_driver_hint(config, &err));
            }
        }
    };

    Ok(ConnectionTestResult::success("Connection successful")
        .with_database_info(database_info_from_protocol_value(&response)))
}

async fn optional_mysql_database_info(
    pool: &db::mysql::MySqlPool,
    config: &ConnectionConfig,
) -> Option<DatabaseConnectionInfo> {
    match db::mysql::database_connection_info(pool, db::mysql::protocol_product_name(config)).await {
        Ok(info) => Some(info),
        Err(error) => {
            log::warn!("Failed to read optional MySQL database information: {error}");
            None
        }
    }
}

async fn connect_agent_pool(
    state: &AppState,
    config: &ConnectionConfig,
    host: &str,
    port: u16,
) -> Result<PoolKind, String> {
    let connect_params = agent_connect_params(config, host, port, config.effective_database().unwrap_or(""));
    let mut client = state.agent_manager.spawn(&config.db_type, config.driver_profile.as_deref()).await?;
    let connect_result = client
        .call_method_with_timeout::<serde_json::Value>(
            AgentMethod::Connect,
            connect_params,
            Some(agent_connect_timeout(config)),
        )
        .await;

    if let Err(err) = connect_result {
        if let Some(alternate_config) = oracle_alternate_connect_config(config, &err) {
            client
                .call_method_with_timeout::<serde_json::Value>(
                    AgentMethod::Connect,
                    agent_connect_params(
                        &alternate_config,
                        host,
                        port,
                        alternate_config.effective_database().unwrap_or(""),
                    ),
                    Some(agent_connect_timeout(&alternate_config)),
                )
                .await
                .map_err(|alternate_err| {
                    format!("{err}\n\nFallback with alternate Oracle descriptor failed: {alternate_err}")
                })?;
        } else {
            return Err(oracle_error_with_driver_hint(config, &err));
        }
    }

    Ok(PoolKind::Agent(Arc::new(tokio::sync::Mutex::new(client))))
}

fn sqlite_extension_specs_from_config(config: &ConnectionConfig) -> Vec<db::sqlite::SqliteExtensionSpec> {
    db::sqlite::sqlite_extension_specs_from_url_params(config.url_params.as_deref())
        .into_iter()
        .map(|mut extension| {
            extension.path = expand_tilde(&extension.path);
            extension
        })
        .collect()
}

pub async fn connect_sqlite_from_config(config: &ConnectionConfig) -> Result<db::sqlite::SqliteHandle, String> {
    db::sqlite::connect_path_with_cipher_key_and_extensions(
        &expand_tilde(&config.host),
        &config.password,
        sqlite_extension_specs_from_config(config),
    )
    .await
}

pub async fn test_connection(state: &AppState, config: ConnectionConfig) -> Result<String, String> {
    test_connection_with_info(state, config).await.map(|result| result.message)
}

/// Test connectivity and optionally return product/database metadata for the dialog.
pub async fn test_connection_with_info(
    state: &AppState,
    config: ConnectionConfig,
) -> Result<ConnectionTestResult, String> {
    let tunnel_id = format!("{}:test", config.id);
    let has_transport_layers = config.has_effective_transport_layers();
    // Dispatch lives in an inner future so `?` / early returns cannot skip tunnel cleanup.
    let result = async {
        let connection_id = if has_transport_layers { tunnel_id.as_str() } else { config.id.as_str() };
        let (host, port) = state.connection_host_port(connection_id, &config).await?;
        let probe_result = probe_connection_endpoint(&config, &host, port).await;
        let url = connection_url_for_endpoint(&config, &host, port);
        let target = redacted_connection_url_for_endpoint(&config, &host, port);
        let connect_timeout = std::time::Duration::from_secs(config.effective_connect_timeout_secs());
        let idle_timeout = std::time::Duration::from_secs(config.idle_timeout_secs);
        log::info!("[test_connection] db_type={:?} target={}", config.db_type, target);
        let mut database_info = None;
        let message = match probe_result {
            Err(e) => return Err(e),
            Ok(()) => match config.db_type {
                DatabaseType::Mysql if config.needs_bare_mysql() && !config.bare_mysql_uses_tls() => {
                    match db::mysql::connect_bare(&url, connect_timeout).await {
                        Ok(pool) => {
                            database_info = optional_mysql_database_info(&pool, &config).await;
                            let _ = pool.disconnect().await;
                            "Connection successful".to_string()
                        }
                        Err(e) => return Err(e),
                    }
                }
                DatabaseType::Mysql if config.needs_bare_mysql() && config.bare_mysql_uses_tls() => {
                    match db::mysql::connect_compatible_with_ca_cert_pool_limit_idle_and_setup(
                        &url,
                        Some(&config.ca_cert_path),
                        connect_timeout,
                        10,
                        None,
                        &[],
                    )
                    .await
                    {
                        Ok(pool) => {
                            database_info = optional_mysql_database_info(&pool, &config).await;
                            let _ = pool.disconnect().await;
                            "Connection successful".to_string()
                        }
                        Err(e) => return Err(e),
                    }
                }
                DatabaseType::Mysql => {
                    match db::mysql::connect_with_ca_cert(&url, Some(&config.ca_cert_path), connect_timeout).await {
                        Ok(pool) => {
                            database_info = optional_mysql_database_info(&pool, &config).await;
                            let _ = pool.disconnect().await;
                            "Connection successful".to_string()
                        }
                        Err(e) => return Err(e),
                    }
                }
                DatabaseType::Doris | DatabaseType::ManticoreSearch => {
                    match db::mysql::connect_bare(&url, connect_timeout).await {
                        Ok(pool) => {
                            database_info = optional_mysql_database_info(&pool, &config).await;
                            let _ = pool.disconnect().await;
                            "Connection successful".to_string()
                        }
                        Err(e) => return Err(e),
                    }
                }
                DatabaseType::StarRocks => {
                    let connect = if config.bare_mysql_uses_tls() {
                        db::mysql::connect_compatible_with_ca_cert_pool_limit_idle_and_setup(
                            &url,
                            Some(&config.ca_cert_path),
                            connect_timeout,
                            10,
                            None,
                            &[],
                        )
                        .await
                    } else {
                        db::mysql::connect_bare(&url, connect_timeout).await
                    };
                    match connect {
                        Ok(pool) => {
                            database_info = optional_mysql_database_info(&pool, &config).await;
                            let _ = pool.disconnect().await;
                            "Connection successful".to_string()
                        }
                        Err(e) => return Err(e),
                    }
                }
                DatabaseType::Postgres
                | DatabaseType::Redshift
                | DatabaseType::Gaussdb
                | DatabaseType::Kwdb
                | DatabaseType::Questdb
                | DatabaseType::OpenGauss => match db::postgres::connect(&url, connect_timeout).await {
                    Ok(pool) => {
                        pool.close();
                        "Connection successful".to_string()
                    }
                    Err(e) => return Err(e),
                },
                DatabaseType::Sqlite => match connect_sqlite_from_config(&config).await {
                    Ok(_) => "Connection successful".to_string(),
                    Err(e) => return Err(e),
                },
                DatabaseType::Redis => {
                    if config.uses_redis_cluster() {
                        state.connect_redis_cluster(&tunnel_id, &config).await?;
                        "Connection successful".to_string()
                    } else if config.uses_redis_sentinel() {
                        state.connect_redis_sentinel(&tunnel_id, &config).await?;
                        "Connection successful".to_string()
                    } else {
                        let con = db::redis_driver::connect(&url, connect_timeout).await?;
                        drop(con);
                        "Connection successful".to_string()
                    }
                }
                #[cfg(feature = "duckdb-bundled")]
                DatabaseType::DuckDb => {
                    state.test_duckdb_connection_config(&config).await?;
                    "Connection successful".to_string()
                }
                #[cfg(not(feature = "duckdb-bundled"))]
                DatabaseType::DuckDb => {
                    return Err("DuckDB support not compiled (enable duckdb-bundled feature)".to_string())
                }
                DatabaseType::MongoDb => {
                    if mongo_uses_legacy_driver(&config) {
                        let am = &state.agent_manager;
                        let mut client = am.spawn(&config.db_type, config.driver_profile.as_deref()).await?;
                        client
                            .connect(mongo_legacy_connect_params(&config, &host, port))
                            .await
                            .map_err(|err| mongo_legacy_error_with_auth_hint(&err))?;
                        client.disconnect().await.ok();
                        "Connection successful (via legacy driver)".to_string()
                    } else {
                        let native_err = match db::mongo_driver::connect(&url, connect_timeout, idle_timeout).await {
                            Ok(client) => {
                                match db::mongo_driver::test_connection(
                                    &client,
                                    connect_timeout,
                                    config.effective_database(),
                                )
                                .await
                                {
                                    Ok(()) => return Ok(ConnectionTestResult::success("Connection successful")),
                                    Err(e) => e,
                                }
                            }
                            Err(e) => e,
                        };
                        if should_retry_mongo_with_legacy_driver(&native_err) {
                            let am = &state.agent_manager;
                            let mut client = am.spawn(&config.db_type, Some("mongodb-legacy")).await?;
                            client.connect(mongo_legacy_connect_params(&config, &host, port)).await.map_err(|err| {
                                format!(
                                    "{native_err}\n\nFallback with MongoDB (Legacy) driver failed: {}",
                                    mongo_legacy_error_with_auth_hint(&err)
                                )
                            })?;
                            client.disconnect().await.ok();
                            "Connection successful (via legacy driver)".to_string()
                        } else {
                            return Err(native_err);
                        }
                    }
                }
                DatabaseType::ClickHouse => {
                    let username = if config.username.is_empty() { None } else { Some(config.username.clone()) };
                    let password = if config.password.is_empty() { None } else { Some(config.password.clone()) };
                    let client = db::clickhouse_driver::ChClient::new_with_ca_cert(
                        &url,
                        username,
                        password,
                        Some(&config.ca_cert_path),
                        config.url_params.as_deref(),
                        connect_timeout,
                    )?;
                    db::clickhouse_driver::test_connection(&client, connect_timeout).await?;
                    "Connection successful".to_string()
                }
                DatabaseType::SqlServer => {
                    let details = state
                        .test_sqlserver_connection_with_legacy_fallback_with_info(&config, &host, port, connect_timeout)
                        .await?;
                    database_info = details.database_info;
                    details.message
                }
                DatabaseType::Elasticsearch => {
                    let mut client = db::elasticsearch_driver::EsClient::from_config(
                        &url,
                        Some(&config.username),
                        Some(&config.password),
                        config.ssl,
                        config.url_params.as_deref(),
                        config.external_config.as_ref(),
                        connect_timeout,
                    );
                    db::elasticsearch_driver::test_connection(&mut client, connect_timeout).await?;
                    "Connection successful".to_string()
                }
                DatabaseType::Qdrant | DatabaseType::Milvus | DatabaseType::Weaviate | DatabaseType::ChromaDb => {
                    let kind = match config.db_type {
                        DatabaseType::Qdrant => db::vector_driver::VectorDbKind::Qdrant,
                        DatabaseType::Milvus => db::vector_driver::VectorDbKind::Milvus,
                        DatabaseType::Weaviate => db::vector_driver::VectorDbKind::Weaviate,
                        DatabaseType::ChromaDb => db::vector_driver::VectorDbKind::ChromaDb,
                        _ => unreachable!(),
                    };
                    let client = db::vector_driver::VectorClient::new(
                        kind,
                        &url,
                        Some(&config.username),
                        Some(&config.password),
                        config.ssl,
                        connect_timeout,
                    );
                    db::vector_driver::test_connection(&client, connect_timeout).await?;
                    "Connection successful".to_string()
                }
                DatabaseType::Rqlite => {
                    let client = db::rqlite_driver::RqliteClient::new(
                        &url,
                        config.url_params.as_deref(),
                        &config.username,
                        &config.password,
                        config.ssl,
                        connect_timeout,
                    )?;
                    db::rqlite_driver::test_connection(&client, connect_timeout).await?;
                    "Connection successful".to_string()
                }
                DatabaseType::Turso => {
                    let auth_token = if !config.password.is_empty() {
                        config.password.clone()
                    } else {
                        config
                            .url_params
                            .as_deref()
                            .and_then(|p| {
                                p.trim()
                                    .trim_start_matches('?')
                                    .split('&')
                                    .filter_map(|pair| pair.split_once('='))
                                    .find(|(key, _)| {
                                        let k = key.trim().to_ascii_lowercase();
                                        k == "auth_token" || k == "authtoken" || k == "auth-token"
                                    })
                                    .map(|(_, value)| value.trim().to_string())
                            })
                            .unwrap_or_default()
                    };
                    let client = db::turso_driver::TursoClient::new(&url, &auth_token, config.ssl, connect_timeout)?;
                    db::turso_driver::test_connection(&client, connect_timeout).await?;
                    "Connection successful".to_string()
                }
                DatabaseType::CloudflareD1 => {
                    db::cloudflare_d1_driver::connect(&config, connect_timeout).await?;
                    "Connection successful".to_string()
                }
                DatabaseType::InfluxDb => {
                    let client = db::influxdb_driver::InfluxdbClient::new_for_config(&url, &config, connect_timeout)?;
                    db::influxdb_driver::test_connection(&client, connect_timeout).await?;
                    "Connection successful".to_string()
                }
                DatabaseType::Nacos => {
                    let admin_config = state.nacos_admin_config_for_connection(connection_id, &config).await?;
                    let adapter = state.nacos_registry.build_transient_config(admin_config).await?;
                    adapter.test_connection().await?;
                    "Connection successful".to_string()
                }
                #[cfg(feature = "mq-admin")]
                DatabaseType::MessageQueue => {
                    // Transient adapter only — do not cache under the real connection id.
                    let mqc = state.mq_admin_config_for_connection(connection_id, &config).await?;
                    let agent_launch = crate::mq::service::resolve_mq_agent_launch_spec(&mqc, &state);
                    let adapter = state.mq_registry.build_transient_config(mqc, agent_launch).await?;
                    adapter.test_connection().await?;
                    "Connection successful".to_string()
                }
                #[cfg(not(feature = "mq-admin"))]
                DatabaseType::MessageQueue => return Err(
                    "Message queue admin support is not compiled in this build. Rebuild with the 'mq-admin' feature."
                        .to_string(),
                ),
                db_type if database_capabilities::is_agent_type(&db_type) => {
                    let details = test_agent_connection(state, &config, &host, port).await?;
                    database_info = details.database_info;
                    details.message
                }
                DatabaseType::PrestoSql => {
                    let jdbc_config = prestosql_jdbc_config_for_endpoint(&config, &host, port);
                    let details = state.test_external_driver_with_info("jdbc", &jdbc_config).await?;
                    database_info = details.database_info;
                    details.message
                }
                DatabaseType::Jdbc => {
                    let mut jdbc_config = config.clone();
                    if host != config.host || port != config.port {
                        if let Some(ref url) = jdbc_config.connection_string {
                            jdbc_config.connection_string = Some(rewrite_jdbc_url_host(url, &host, port));
                        }
                    }
                    let details = state.test_external_driver_with_info("jdbc", &jdbc_config).await?;
                    database_info = details.database_info;
                    details.message
                }
                db_type => return Err(format!("Unsupported database type: {db_type:?}")),
            },
        };
        Ok(ConnectionTestResult::success(message).with_database_info(database_info))
    }
    .await;

    if has_transport_layers {
        state.reset_connection_transport_for_config(&tunnel_id, &config).await;
    }

    result
}

/// Connect and register the base pool for `config.id`.
/// Returns the connection id on success.
pub async fn connect(
    state: &AppState,
    config: ConnectionConfig,
    client_attempt: Option<u64>,
) -> Result<String, String> {
    let config = config.canonicalized();
    let id = config.id.clone();
    let db_config = metadata_connection_config(&config);
    let connect_timeout = std::time::Duration::from_secs(db_config.effective_connect_timeout_secs());
    let db_type_label = database_type_log_label(db_config.db_type);
    let stage_started = Instant::now();
    let log_context = StageLogContext {
        connection_id: Some(id.as_str()),
        pool_key: Some(id.as_str()),
        database: None,
        db_type: Some(db_type_label.as_str()),
        trace_id: None,
        client_session_id: None,
    };
    log_stage(
        StageLog::new(LifecycleStage::EnsureConnected, StageOutcome::Start, 0)
            .with_timeout(connect_timeout)
            .with_context(log_context),
    );

    let result = connect_inner(state, config, client_attempt, &id, db_config, connect_timeout).await;
    match &result {
        Ok(_) => {
            log_stage(
                StageLog::new(LifecycleStage::EnsureConnected, StageOutcome::Done, stage_started.elapsed().as_millis())
                    .with_timeout(connect_timeout)
                    .with_context(log_context),
            );
        }
        Err(err) => {
            log_stage(
                StageLog::new(
                    LifecycleStage::EnsureConnected,
                    StageOutcome::Error,
                    stage_started.elapsed().as_millis(),
                )
                .with_timeout(connect_timeout)
                .with_context(log_context)
                .with_error(err),
            );
        }
    }
    result
}

async fn connect_inner(
    state: &AppState,
    config: ConnectionConfig,
    client_attempt: Option<u64>,
    id: &str,
    db_config: ConnectionConfig,
    connect_timeout: std::time::Duration,
) -> Result<String, String> {
    let attempt = state.begin_connection_attempt_with_client_attempt(id, client_attempt).await;

    state.remove_connection_pools_detached(id).await;
    state.reset_connection_transport_for_config(id, &db_config).await;

    // Driver dispatch lives in an inner future so probe/`?` failures still hit the
    // unified transport cleanup below. Cleanup is attempt-gated: a superseded attempt
    // must not tear down tunnels owned by a newer attempt on the same connection id.
    let result = async {
        let mut connected_config = config.clone();
        let mut connected_db_config = db_config.clone();

        let (host, port) = state.connection_host_port(id, &db_config).await?;
        state.ensure_current_connection_attempt(id, Some(attempt)).await?;
        probe_connection_endpoint(&db_config, &host, port).await?;
        state.ensure_current_connection_attempt(id, Some(attempt)).await?;
        let url = connection_url_for_endpoint(&db_config, &host, port);
        let idle_timeout = std::time::Duration::from_secs(db_config.idle_timeout_secs);

        let pool = match db_config.db_type {
            DatabaseType::Mysql => {
                let (pool, mode) =
                    connect_mysql_metadata_pool(&config, &db_config, &host, port, connect_timeout, 3).await?;
                PoolKind::Mysql(pool, mode)
            }
            DatabaseType::Doris | DatabaseType::StarRocks | DatabaseType::ManticoreSearch => PoolKind::Mysql(
                connect_bare_metadata_pool(&db_config, &host, port, connect_timeout, 3).await?,
                MysqlMode::Bare,
            ),
            DatabaseType::Postgres
            | DatabaseType::Redshift
            | DatabaseType::Gaussdb
            | DatabaseType::Kwdb
            | DatabaseType::Questdb
            | DatabaseType::OpenGauss => PoolKind::Postgres(db::postgres::connect(&url, connect_timeout).await?),
            DatabaseType::Sqlite => PoolKind::Sqlite(connect_sqlite_from_config(&db_config).await?),
            DatabaseType::Redis => {
                let con = if db_config.uses_redis_cluster() {
                    PoolKind::Redis(db::redis_driver::RedisConnection::Cluster(
                        state.connect_redis_cluster(id, &db_config).await?,
                    ))
                } else if db_config.uses_redis_sentinel() {
                    PoolKind::Redis(db::redis_driver::RedisConnection::Direct(tokio::sync::Mutex::new(
                        state.connect_redis_sentinel(id, &db_config).await?,
                    )))
                } else {
                    PoolKind::Redis(db::redis_driver::RedisConnection::Direct(tokio::sync::Mutex::new(
                        db::redis_driver::connect(&url, connect_timeout).await?,
                    )))
                };
                con
            }
            #[cfg(feature = "duckdb-bundled")]
            DatabaseType::DuckDb => {
                let con = db::duckdb_driver::connect_path(&expand_tilde(&db_config.host))?;
                {
                    let locked = con.lock().map_err(|e| e.to_string())?;
                    for attached in &db_config.attached_databases {
                        crate::schema::duckdb_attach_database(&locked, &attached.name, &expand_tilde(&attached.path))?;
                    }
                    if let Some(script) = db_config.init_script.as_deref() {
                        db::duckdb_driver::run_init_script(&locked, script)?;
                    }
                }
                PoolKind::DuckDb(con)
            }
            #[cfg(not(feature = "duckdb-bundled"))]
            DatabaseType::DuckDb => {
                return Err("DuckDB support not compiled (enable duckdb-bundled feature)".to_string())
            }
            DatabaseType::MongoDb => {
                if mongo_uses_legacy_driver(&db_config) {
                    let mut client =
                        state.agent_manager.spawn(&db_config.db_type, Some(MONGO_LEGACY_DRIVER_PROFILE)).await?;
                    state.ensure_current_connection_attempt(id, Some(attempt)).await?;
                    client
                        .connect(mongo_legacy_connect_params(&db_config, &host, port))
                        .await
                        .map_err(|err| mongo_legacy_error_with_auth_hint(&err))?;
                    state.ensure_current_connection_attempt(id, Some(attempt)).await?;
                    PoolKind::Agent(std::sync::Arc::new(tokio::sync::Mutex::new(client)))
                } else {
                    let native_err = match db::mongo_driver::connect(&url, connect_timeout, idle_timeout).await {
                        Ok(client) => {
                            state.ensure_current_connection_attempt(id, Some(attempt)).await?;
                            match db::mongo_driver::test_connection(
                                &client,
                                connect_timeout,
                                db_config.effective_database(),
                            )
                            .await
                            {
                                Ok(()) => {
                                    state.ensure_current_connection_attempt(id, Some(attempt)).await?;
                                    // On insert failure (including superseded attempt), do not reset
                                    // transport here — outer attempt-gated cleanup owns that path.
                                    state
                                        .insert_connection_pool_for_attempt(
                                            id,
                                            attempt,
                                            id.to_string(),
                                            PoolKind::MongoDb(client),
                                            &db_config,
                                        )
                                        .await?;
                                    state.configs.write().await.insert(id.to_string(), config);
                                    return Ok(id.to_string());
                                }
                                Err(e) => e,
                            }
                        }
                        Err(e) => e,
                    };
                    if should_retry_mongo_with_legacy_driver(&native_err) {
                        log::info!("Native MongoDB driver failed ({native_err}), falling back to agent driver");
                        let mut client =
                            state.agent_manager.spawn(&db_config.db_type, Some(MONGO_LEGACY_DRIVER_PROFILE)).await?;
                        state.ensure_current_connection_attempt(id, Some(attempt)).await?;
                        client.connect(mongo_legacy_connect_params(&db_config, &host, port)).await.map_err(|err| {
                            format!(
                                "{native_err}\n\nFallback with MongoDB (Legacy) driver failed: {}",
                                mongo_legacy_error_with_auth_hint(&err)
                            )
                        })?;
                        state.ensure_current_connection_attempt(id, Some(attempt)).await?;
                        mark_mongo_legacy_driver(&mut connected_config);
                        connected_db_config = metadata_connection_config(&connected_config);
                        persist_mongo_legacy_driver_profile(state, &connected_config).await?;
                        PoolKind::Agent(std::sync::Arc::new(tokio::sync::Mutex::new(client)))
                    } else {
                        return Err(native_err);
                    }
                }
            }
            DatabaseType::ClickHouse => {
                let username = if db_config.username.is_empty() { None } else { Some(db_config.username.clone()) };
                let password = if db_config.password.is_empty() { None } else { Some(db_config.password.clone()) };
                log::info!("[connect_db] ClickHouse url={url} user={:?} has_pass={}", username, password.is_some());
                let client = db::clickhouse_driver::ChClient::new_with_ca_cert(
                    &url,
                    username,
                    password,
                    Some(&db_config.ca_cert_path),
                    db_config.url_params.as_deref(),
                    connect_timeout,
                )?;
                db::clickhouse_driver::test_connection(&client, connect_timeout).await?;
                PoolKind::ClickHouse(client)
            }
            DatabaseType::SqlServer => {
                state.connect_sqlserver_pool_with_legacy_fallback(&db_config, &host, port, connect_timeout).await?
            }
            DatabaseType::Elasticsearch => {
                let mut client = db::elasticsearch_driver::EsClient::from_config(
                    &url,
                    Some(&db_config.username),
                    Some(&db_config.password),
                    db_config.ssl,
                    db_config.url_params.as_deref(),
                    db_config.external_config.as_ref(),
                    connect_timeout,
                );
                db::elasticsearch_driver::test_connection(&mut client, connect_timeout).await?;
                PoolKind::Elasticsearch(client)
            }
            DatabaseType::Qdrant | DatabaseType::Milvus | DatabaseType::Weaviate | DatabaseType::ChromaDb => {
                let kind = match db_config.db_type {
                    DatabaseType::Qdrant => db::vector_driver::VectorDbKind::Qdrant,
                    DatabaseType::Milvus => db::vector_driver::VectorDbKind::Milvus,
                    DatabaseType::Weaviate => db::vector_driver::VectorDbKind::Weaviate,
                    DatabaseType::ChromaDb => db::vector_driver::VectorDbKind::ChromaDb,
                    _ => unreachable!(),
                };
                let client = db::vector_driver::VectorClient::new(
                    kind,
                    &url,
                    Some(&db_config.username),
                    Some(&db_config.password),
                    db_config.ssl,
                    connect_timeout,
                );
                db::vector_driver::test_connection(&client, connect_timeout).await?;
                PoolKind::VectorDb(client)
            }
            DatabaseType::Rqlite => {
                let client = db::rqlite_driver::RqliteClient::new(
                    &url,
                    db_config.url_params.as_deref(),
                    &db_config.username,
                    &db_config.password,
                    db_config.ssl,
                    connect_timeout,
                )?;
                db::rqlite_driver::test_connection(&client, connect_timeout).await?;
                PoolKind::Rqlite(client)
            }
            DatabaseType::Turso => {
                let auth_token = if !db_config.password.is_empty() {
                    db_config.password.clone()
                } else {
                    db_config
                        .url_params
                        .as_deref()
                        .and_then(|p| {
                            p.trim()
                                .trim_start_matches('?')
                                .split('&')
                                .filter_map(|pair| pair.split_once('='))
                                .find(|(key, _)| {
                                    let k = key.trim().to_ascii_lowercase();
                                    k == "auth_token" || k == "authtoken" || k == "auth-token"
                                })
                                .map(|(_, value)| value.trim().to_string())
                        })
                        .unwrap_or_default()
                };
                let client = db::turso_driver::TursoClient::new(&url, &auth_token, db_config.ssl, connect_timeout)?;
                db::turso_driver::test_connection(&client, connect_timeout).await?;
                PoolKind::Turso(client)
            }
            DatabaseType::CloudflareD1 => {
                PoolKind::CloudflareD1(db::cloudflare_d1_driver::connect(&db_config, connect_timeout).await?)
            }
            DatabaseType::InfluxDb => {
                let client = db::influxdb_driver::InfluxdbClient::new_for_config(&url, &db_config, connect_timeout)?;
                db::influxdb_driver::test_connection(&client, connect_timeout).await?;
                PoolKind::InfluxDb(client)
            }
            DatabaseType::Nacos => {
                let admin_config = state.nacos_admin_config_for_connection(id, &config).await?;
                let adapter = state.nacos_registry.build_transient_config(admin_config).await?;
                adapter.test_connection().await?;
                PoolKind::Nacos
            }
            #[cfg(feature = "mq-admin")]
            DatabaseType::MessageQueue => {
                let mqc = state.mq_admin_config_for_connection(id, &config).await?;
                let agent_launch = crate::mq::service::resolve_mq_agent_launch_spec(&mqc, &state);
                let adapter = match state.mq_registry.get_or_build_config(id, mqc, agent_launch).await {
                    Ok(adapter) => adapter,
                    Err(err) => {
                        state.mq_registry.drop_connection(id).await;
                        return Err(err);
                    }
                };
                if let Err(err) = state.ensure_current_connection_attempt(id, Some(attempt)).await {
                    state.mq_registry.drop_connection(id).await;
                    return Err(err);
                }
                if let Err(err) = adapter.test_connection().await {
                    state.mq_registry.drop_connection(id).await;
                    return Err(err);
                }
                if let Err(err) = state.ensure_current_connection_attempt(id, Some(attempt)).await {
                    state.mq_registry.drop_connection(id).await;
                    return Err(err);
                }
                PoolKind::MessageQueue
            }
            #[cfg(not(feature = "mq-admin"))]
            DatabaseType::MessageQueue => {
                return Err(
                    "Message queue admin support is not compiled in this build. Rebuild with the 'mq-admin' feature."
                        .to_string(),
                );
            }
            db_type if database_capabilities::is_agent_type(&db_type) => {
                connect_agent_pool(state, &db_config, &host, port).await?
            }
            DatabaseType::PrestoSql => {
                let jdbc_config = prestosql_jdbc_config_for_endpoint(&db_config, &host, port);
                state.external_driver_pool("jdbc", &jdbc_config).await?
            }
            DatabaseType::Jdbc => state.external_driver_pool("jdbc", &db_config).await?,
            db_type => return Err(format!("Unsupported database type: {db_type:?}")),
        };

        state.insert_connection_pool_for_attempt(id, attempt, id.to_string(), pool, &connected_db_config).await?;
        state.configs.write().await.insert(id.to_string(), connected_config);

        Ok(id.to_string())
    }
    .await;

    if result.is_err() {
        reset_transport_if_attempt_current(state, id, attempt, &db_config).await;
    }

    result
}

/// Reset tunnels only when `attempt` is still the current connection attempt.
/// Atomic check+stop — see [`AppState::reset_connection_transport_if_attempt_current`].
async fn reset_transport_if_attempt_current(
    state: &AppState,
    connection_id: &str,
    attempt: u64,
    config: &ConnectionConfig,
) {
    state.reset_connection_transport_if_attempt_current(connection_id, attempt, config).await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::connection::{
        default_connect_timeout_secs, default_redis_key_separator, ProxyTunnelConfig, ProxyType, TransportLayerConfig,
    };
    use crate::storage::Storage;

    fn minimal_mongo_config() -> ConnectionConfig {
        ConnectionConfig {
            id: "m1".into(),
            name: "m".into(),
            db_type: DatabaseType::MongoDb,
            driver_profile: None,
            driver_label: None,
            url_params: None,
            agent_java_options: vec![],
            host: "h".into(),
            port: 27017,
            username: String::new(),
            password: String::new(),
            database: None,
            visible_databases: None,
            visible_schemas: None,
            attached_databases: vec![],
            init_script: None,
            color: None,
            transport_layers: vec![],
            connect_timeout_secs: 10,
            query_timeout_secs: 30,
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
            redis_key_separator: default_redis_key_separator(),
            redis_scan_page_size: None,
            etcd_endpoints: String::new(),
            gbase_server: String::new(),
            informix_server: String::new(),
            external_config: None,
            jdbc_driver_class: None,
            jdbc_driver_paths: vec![],
            database_info: None,
            one_time: false,
            read_only: false,
            is_production: false,
            production_databases: vec![],
        }
    }

    fn base_config(id: &str, db_type: DatabaseType) -> ConnectionConfig {
        ConnectionConfig {
            id: id.into(),
            name: id.into(),
            db_type,
            driver_profile: None,
            driver_label: None,
            url_params: None,
            agent_java_options: vec![],
            host: "127.0.0.1".into(),
            port: 3306,
            username: "root".into(),
            password: "secret".into(),
            database: None,
            visible_databases: None,
            visible_schemas: None,
            attached_databases: vec![],
            init_script: None,
            color: None,
            transport_layers: vec![],
            connect_timeout_secs: default_connect_timeout_secs(),
            query_timeout_secs: 30,
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
            redis_key_separator: default_redis_key_separator(),
            redis_scan_page_size: None,
            etcd_endpoints: String::new(),
            gbase_server: String::new(),
            informix_server: String::new(),
            external_config: None,
            jdbc_driver_class: None,
            jdbc_driver_paths: vec![],
            database_info: None,
            one_time: false,
            read_only: false,
            is_production: false,
            production_databases: vec![],
        }
    }

    fn proxy_layer() -> TransportLayerConfig {
        TransportLayerConfig::Proxy(ProxyTunnelConfig {
            profile_id: String::new(),
            id: "proxy".into(),
            name: String::new(),
            enabled: true,
            proxy_type: ProxyType::Socks5,
            // Unreachable proxy: local listener still starts; forward fails later.
            host: "127.0.0.1".into(),
            port: 65000,
            username: String::new(),
            password: String::new(),
            test_target: None,
        })
    }

    fn transport_layer_id(connection_id: &str) -> String {
        format!("{connection_id}:transport:0")
    }

    async fn test_app_state() -> (AppState, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!("dbx-lifecycle-connect-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let storage = Storage::open(&dir.join("storage.db")).await.unwrap();
        (AppState::new(storage), dir)
    }

    #[test]
    fn mark_mongo_legacy_driver_updates_profile_and_label() {
        let mut config = minimal_mongo_config();
        assert!(mark_mongo_legacy_driver(&mut config));
        assert_eq!(config.driver_profile.as_deref(), Some(MONGO_LEGACY_DRIVER_PROFILE));
        assert_eq!(config.driver_label.as_deref(), Some(MONGO_LEGACY_DRIVER_LABEL));
        assert!(!mark_mongo_legacy_driver(&mut config));
    }

    #[test]
    fn connect_timeout_errors_are_classifiable() {
        // PIP-0001: timeout messages must remain greppable for is_connection_error.
        for msg in [
            "PostgreSQL connection pool checkout timed out (5s)",
            "MySQL get connection timed out",
            "connection timed out",
            "TCP probe timed out",
        ] {
            let lower = msg.to_ascii_lowercase();
            assert!(lower.contains("timed out") || lower.contains("timeout"), "expected timeout keyword in {msg}");
        }
    }

    #[tokio::test]
    async fn test_connection_resets_transport_after_driver_failure() {
        let (state, dir) = test_app_state().await;
        let mut config = base_config("test-fail", DatabaseType::Mysql);
        config.connect_timeout_secs = 1;
        config.transport_layers = vec![proxy_layer()];

        let err = test_connection(&state, config.clone()).await.unwrap_err();
        assert!(!err.is_empty());

        let layer_id = transport_layer_id("test-fail:test");
        assert!(
            state.proxy_tunnels.local_port(&layer_id).await.is_none(),
            "failed test_connection must reset proxy tunnel {layer_id}"
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn test_connection_resets_transport_after_success() {
        let (state, dir) = test_app_state().await;
        let db_path = dir.join("ok.db");
        // test_connection opens SQLite without create-if-missing.
        db::sqlite::connect_path_create_if_missing(&db_path.to_string_lossy()).await.expect("seed sqlite file");
        let mut config = base_config("test-ok", DatabaseType::Sqlite);
        config.host = db_path.to_string_lossy().to_string();
        // Empty password avoids SQLCipher path (feature not enabled in mq-admin tests).
        config.password = String::new();
        config.transport_layers = vec![proxy_layer()];

        // SQLite skips TCP probe and uses the file path (not the tunnel endpoint), so
        // the driver can succeed while a transport layer was still opened for the test id.
        let msg = test_connection(&state, config).await.unwrap();
        assert!(msg.contains("successful"), "unexpected: {msg}");

        let layer_id = transport_layer_id("test-ok:test");
        assert!(
            state.proxy_tunnels.local_port(&layer_id).await.is_none(),
            "successful test_connection must still reset proxy tunnel {layer_id}"
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn connect_resets_transport_after_driver_failure() {
        let (state, dir) = test_app_state().await;
        let mut config = base_config("conn-fail", DatabaseType::Mysql);
        config.connect_timeout_secs = 1;
        config.transport_layers = vec![proxy_layer()];

        let err = connect(&state, config.clone(), None).await.unwrap_err();
        assert!(!err.is_empty());

        let layer_id = transport_layer_id("conn-fail");
        assert!(
            state.proxy_tunnels.local_port(&layer_id).await.is_none(),
            "failed connect must reset proxy tunnel {layer_id}"
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn failed_connect_cleanup_skips_transport_owned_by_newer_attempt() {
        let (state, dir) = test_app_state().await;
        let mut config = base_config("stale-conn", DatabaseType::Mysql);
        config.transport_layers = vec![proxy_layer()];

        // Simulate a tunnel owned by a newer attempt on the same connection id.
        let _ = state.connection_host_port("stale-conn", &config).await.unwrap();
        let layer_id = transport_layer_id("stale-conn");
        assert!(state.proxy_tunnels.local_port(&layer_id).await.is_some());

        let old_attempt = state.begin_connection_attempt("stale-conn").await;
        let _new_attempt = state.begin_connection_attempt("stale-conn").await;

        // Old attempt fails cleanup gate — must not tear down the current attempt's tunnel.
        reset_transport_if_attempt_current(&state, "stale-conn", old_attempt, &config).await;
        assert!(
            state.proxy_tunnels.local_port(&layer_id).await.is_some(),
            "superseded attempt must not reset tunnel of newer attempt"
        );

        // Current attempt may clean up.
        reset_transport_if_attempt_current(&state, "stale-conn", _new_attempt, &config).await;
        assert!(state.proxy_tunnels.local_port(&layer_id).await.is_none());

        let _ = std::fs::remove_dir_all(dir);
    }

    /// Deterministic proof that the attempt write lock spans check → tunnel reset → unlock.
    ///
    /// Barriers:
    /// 1. after check (before reset): spawn concurrent `begin` — it must stay pending
    /// 2. after reset (before unlock): tunnel is already gone, `begin` still pending
    /// 3. unlock: only then may `begin` complete
    ///
    /// A non-atomic check-then-act (or unlock-before-reset) would let `begin` finish
    /// between check and reset, open a tunnel, and lose it to a late stop.
    #[tokio::test]
    async fn cleanup_holds_attempt_lock_until_reset_completes() {
        let (state, dir) = test_app_state().await;
        let mut config = base_config("race-conn", DatabaseType::Mysql);
        config.transport_layers = vec![proxy_layer()];
        let connection_id = config.id.clone();

        // Tunnel currently owned by the old attempt.
        let _ = state.connection_host_port(&connection_id, &config).await.unwrap();
        let layer_id = transport_layer_id(&connection_id);
        assert!(state.proxy_tunnels.local_port(&layer_id).await.is_some());

        let old_attempt = state.begin_connection_attempt(&connection_id).await;
        let (after_check_rx, release_for_reset_tx, after_reset_rx, release_for_unlock_tx) =
            state.arm_transport_reset_hold_for_test().await;

        let state = Arc::new(state);
        let state_cleanup = Arc::clone(&state);
        let config_cleanup = config.clone();
        let id_cleanup = connection_id.clone();
        let cleanup = tokio::spawn(async move {
            state_cleanup.reset_connection_transport_if_attempt_current(&id_cleanup, old_attempt, &config_cleanup).await
        });

        after_check_rx.await.expect("cleanup must park after current-attempt check with lock held");

        let state_begin = Arc::clone(&state);
        let id_begin = connection_id.clone();
        let begin = tokio::spawn(async move { state_begin.begin_connection_attempt(&id_begin).await });

        // Allow tunnel stop; still hold the attempt lock until the after-reset barrier.
        release_for_reset_tx.send(()).expect("release for reset");
        after_reset_rx.await.expect("cleanup must park after tunnel reset with lock still held");

        // Reset has finished; begin must still be blocked (lock not dropped yet).
        assert!(!begin.is_finished(), "begin_connection_attempt must stay blocked after reset until unlock");
        assert!(
            state.proxy_tunnels.local_port(&layer_id).await.is_none(),
            "tunnel must already be stopped while lock is still held"
        );
        assert!(!cleanup.is_finished(), "cleanup must still hold the attempt lock at the after-reset barrier");

        // Unlock: cleanup finishes and begin can acquire the attempt map.
        release_for_unlock_tx.send(()).expect("release for unlock");
        let cleaned = cleanup.await.expect("cleanup task");
        assert!(cleaned, "old attempt was still current so cleanup should reset transport");

        let new_attempt = begin.await.expect("begin after unlock");
        assert_ne!(new_attempt, old_attempt);

        // Newer attempt can open a fresh tunnel; stale cleanup must not tear it down.
        let _ = state.connection_host_port(&connection_id, &config).await.unwrap();
        assert!(state.proxy_tunnels.local_port(&layer_id).await.is_some());
        let stale_cleaned =
            state.reset_connection_transport_if_attempt_current(&connection_id, old_attempt, &config).await;
        assert!(!stale_cleaned);
        assert!(state.proxy_tunnels.local_port(&layer_id).await.is_some());

        state.reset_connection_transport_for_config(&connection_id, &config).await;
        let _ = std::fs::remove_dir_all(dir);
    }
}
