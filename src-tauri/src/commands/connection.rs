use std::collections::HashSet;
use std::sync::Arc;
use tauri::State;

pub use dbx_core::connection::{metadata_connection_config, AppState};
use dbx_core::models::connection::{ConnectionConfig, ConnectionTestResult, DatabaseType};

#[cfg(test)]
mod tests {
    #[cfg(feature = "sqlite-sqlcipher")]
    use dbx_core::connection_lifecycle::connect::connect_sqlite_from_config;
    use dbx_core::connection_lifecycle::connect::{
        mark_mongo_legacy_driver, mongo_legacy_connect_params, MONGO_LEGACY_DRIVER_LABEL, MONGO_LEGACY_DRIVER_PROFILE,
    };
    use dbx_core::models::connection::{ConnectionConfig, DatabaseType};
    #[cfg(feature = "mq-admin")]
    use {
        super::{load_connection_configs, save_connection_configs},
        dbx_core::connection::{AppState, PoolKind},
        dbx_core::storage::Storage,
    };

    fn mongodb_config() -> ConnectionConfig {
        ConnectionConfig {
            id: "mongo".to_string(),
            name: "MongoDB".to_string(),
            db_type: DatabaseType::MongoDb,
            driver_profile: Some("mongodb".to_string()),
            driver_label: Some("MongoDB".to_string()),
            url_params: Some("authSource=admin&authMechanism=SCRAM-SHA-1".to_string()),
            agent_java_options: Vec::new(),
            host: "172.22.4.42".to_string(),
            port: 27017,
            username: "mongouser".to_string(),
            password: "secret".to_string(),
            database: Some("RestCloud_V45PUB_Gateway".to_string()),
            visible_databases: None,
            visible_schemas: None,
            attached_databases: Vec::new(),
            init_script: None,
            color: None,
            transport_layers: Vec::new(),
            connect_timeout_secs: dbx_core::models::connection::default_connect_timeout_secs(),
            query_timeout_secs: dbx_core::models::connection::default_query_timeout_secs(),
            idle_timeout_secs: dbx_core::models::connection::default_idle_timeout_secs(),
            keepalive_interval_secs: dbx_core::models::connection::default_keepalive_interval_secs(),
            ssl: false,
            ca_cert_path: String::new(),
            client_cert_path: String::new(),
            client_key_path: String::new(),
            sysdba: false,
            oracle_connection_type: None,
            connection_string: Some(
                "mongodb://mongouser:secret@172.22.4.42:27017/RestCloud_V45PUB_Gateway?authSource=admin".to_string(),
            ),
            redis_connection_mode: None,
            redis_sentinel_master: String::new(),
            redis_sentinel_nodes: String::new(),
            redis_sentinel_username: String::new(),
            redis_sentinel_password: String::new(),
            redis_sentinel_tls: false,
            redis_cluster_nodes: String::new(),
            redis_key_separator: dbx_core::models::connection::default_redis_key_separator(),
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
            database_info: None,
        }
    }

    #[cfg(feature = "sqlite-sqlcipher")]
    fn sqlite_config(path: &std::path::Path, password: &str) -> ConnectionConfig {
        let mut config = mongodb_config();
        config.id = "sqlite".to_string();
        config.name = "SQLite".to_string();
        config.db_type = DatabaseType::Sqlite;
        config.driver_profile = None;
        config.driver_label = None;
        config.url_params = None;
        config.host = path.to_string_lossy().to_string();
        config.port = 0;
        config.username = String::new();
        config.password = password.to_string();
        config.database = None;
        config.connection_string = None;
        config
    }

    #[cfg(feature = "mq-admin")]
    fn mq_config(id: &str, admin_url: &str) -> ConnectionConfig {
        let mut config = mongodb_config();
        config.id = id.to_string();
        config.name = "Pulsar".to_string();
        config.db_type = DatabaseType::MessageQueue;
        config.driver_profile = None;
        config.driver_label = None;
        config.url_params = None;
        config.host = String::new();
        config.port = 0;
        config.username = String::new();
        config.password = String::new();
        config.database = None;
        config.connection_string = None;
        config.external_config = Some(serde_json::json!({
            "systemKind": "pulsar",
            "adminUrl": admin_url,
            "auth": { "kind": "none" },
            "pinnedVersion": "3.1"
        }));
        config
    }

    #[test]
    fn mongo_legacy_connect_params_preserve_auth_options() {
        let config = mongodb_config();

        let params = mongo_legacy_connect_params(&config, "172.22.4.42", 27017);

        assert_eq!(params["connection"]["database"], "RestCloud_V45PUB_Gateway");
        assert_eq!(params["connection"]["url_params"], "authSource=admin&authMechanism=SCRAM-SHA-1");
        assert_eq!(
            params["connection"]["connection_string"],
            "mongodb://mongouser:secret@172.22.4.42:27017/RestCloud_V45PUB_Gateway?authSource=admin"
        );
    }

    #[test]
    fn mark_mongo_legacy_driver_updates_profile_and_label() {
        let mut config = mongodb_config();

        assert!(mark_mongo_legacy_driver(&mut config));
        assert_eq!(config.driver_profile.as_deref(), Some(MONGO_LEGACY_DRIVER_PROFILE));
        assert_eq!(config.driver_label.as_deref(), Some(MONGO_LEGACY_DRIVER_LABEL));
        assert!(!mark_mongo_legacy_driver(&mut config));
    }

    #[cfg(feature = "sqlite-sqlcipher")]
    #[tokio::test]
    async fn sqlite_connect_from_config_uses_sqlcipher_key() {
        let path = std::env::temp_dir().join(format!("dbx-tauri-sqlcipher-{}.db", uuid::Uuid::new_v4()));
        let key = "dbx-pass";

        {
            let pool =
                dbx_core::db::sqlite::connect_path_create_if_missing_with_cipher_key(path.to_str().unwrap(), key)
                    .await
                    .expect("create encrypted sqlite");
            pool.with_connection(|conn| {
                conn.execute_batch(
                    "CREATE TABLE users(id INTEGER PRIMARY KEY, name TEXT); INSERT INTO users(name) VALUES ('Ada'), ('Grace');",
                )
                .map_err(|err| err.to_string())
            })
            .expect("write encrypted sqlite");
        }

        let config = sqlite_config(&path, key);
        let pool = connect_sqlite_from_config(&config).await.expect("open encrypted sqlite");
        let count = pool
            .with_connection(|conn| {
                conn.query_row("SELECT count(*) FROM users", [], |row| row.get::<_, i64>(0))
                    .map_err(|err| err.to_string())
            })
            .expect("read encrypted sqlite");
        assert_eq!(count, 2);

        let wrong_key = match connect_sqlite_from_config(&sqlite_config(&path, "wrong-key")).await {
            Ok(_) => panic!("wrong SQLCipher key must fail"),
            Err(err) => err,
        };
        assert!(wrong_key.contains("SQLCipher database unlock failed"));

        let missing_key = match connect_sqlite_from_config(&sqlite_config(&path, "")).await {
            Ok(_) => panic!("missing SQLCipher key must fail"),
            Err(err) => err,
        };
        assert!(missing_key.contains("not a valid SQLite database"));

        let _ = std::fs::remove_file(path);
    }

    #[cfg(feature = "mq-admin")]
    #[tokio::test]
    async fn save_connection_configs_updates_runtime_cache_and_drops_mq_adapter() {
        let dir = std::env::temp_dir().join(format!("dbx-tauri-conn-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let storage = Storage::open(&dir.join("storage.db")).await.unwrap();
        let state = AppState::new_with_plugin_dir(storage, dir.join("plugins"));
        let initial = mq_config("mq-conn", "http://127.0.0.1:8080");
        state.configs.write().await.insert(initial.id.clone(), initial.clone());
        state.connections.write().await.insert(initial.id.clone(), PoolKind::MessageQueue);
        let first = state.mq_registry.get_or_build(&initial).await.unwrap();

        let updated = mq_config("mq-conn", "http://127.0.0.1:8081");
        save_connection_configs(&state, &[updated.clone()]).await.unwrap();

        let cached_admin_url = state
            .configs
            .read()
            .await
            .get("mq-conn")
            .and_then(|config| config.external_config.as_ref())
            .and_then(|external| external.get("adminUrl"))
            .and_then(serde_json::Value::as_str)
            .map(str::to_string);
        assert_eq!(cached_admin_url.as_deref(), Some("http://127.0.0.1:8081"));

        let second = state.mq_registry.get_or_build(&updated).await.unwrap();
        assert!(!std::sync::Arc::ptr_eq(&first, &second));
        assert!(!state.connections.read().await.contains_key(&initial.id));

        let _ = std::fs::remove_dir_all(dir);
    }

    #[cfg(feature = "mq-admin")]
    #[tokio::test]
    async fn load_connection_configs_syncs_runtime_cache_and_drops_stale_pool() {
        let dir = std::env::temp_dir().join(format!("dbx-tauri-conn-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let storage = Storage::open(&dir.join("storage.db")).await.unwrap();
        let state = AppState::new_with_plugin_dir(storage, dir.join("plugins"));
        let initial = mq_config("mq-conn", "http://127.0.0.1:8080");
        let updated = mq_config("mq-conn", "http://127.0.0.1:8081");
        state.storage.save_connections(&[updated.clone()]).await.unwrap();
        state.configs.write().await.insert(initial.id.clone(), initial.clone());
        state.connections.write().await.insert(initial.id.clone(), PoolKind::MessageQueue);

        let loaded = load_connection_configs(&state).await.unwrap();

        assert_eq!(loaded.len(), 1);
        let cached_admin_url = state
            .configs
            .read()
            .await
            .get("mq-conn")
            .and_then(|config| config.external_config.as_ref())
            .and_then(|external| external.get("adminUrl"))
            .and_then(serde_json::Value::as_str)
            .map(str::to_string);
        assert_eq!(cached_admin_url.as_deref(), Some("http://127.0.0.1:8081"));
        assert!(!state.connections.read().await.contains_key(&initial.id));

        let _ = std::fs::remove_dir_all(dir);
    }

    #[cfg(feature = "mq-admin")]
    #[tokio::test]
    async fn save_connection_configs_removes_deleted_runtime_config_and_mq_adapter() {
        let dir = std::env::temp_dir().join(format!("dbx-tauri-conn-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let storage = Storage::open(&dir.join("storage.db")).await.unwrap();
        let state = AppState::new_with_plugin_dir(storage, dir.join("plugins"));
        let kept = mongodb_config();
        let removed = mq_config("removed-mq", "http://127.0.0.1:8080");
        {
            let mut configs = state.configs.write().await;
            configs.insert(kept.id.clone(), kept.clone());
            configs.insert(removed.id.clone(), removed.clone());
        }
        let stale = state.mq_registry.get_or_build(&removed).await.unwrap();

        save_connection_configs(&state, &[kept.clone()]).await.unwrap();

        let configs = state.configs.read().await;
        assert!(configs.contains_key(&kept.id));
        assert!(!configs.contains_key("removed-mq"));
        drop(configs);

        let rebuilt = state.mq_registry.get_or_build(&removed).await.unwrap();
        assert!(!std::sync::Arc::ptr_eq(&stale, &rebuilt));

        let _ = std::fs::remove_dir_all(dir);
    }

    #[cfg(feature = "mq-admin")]
    #[tokio::test]
    async fn save_connection_configs_removes_deleted_connection_pools() {
        let dir = std::env::temp_dir().join(format!("dbx-tauri-conn-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let storage = Storage::open(&dir.join("storage.db")).await.unwrap();
        let state = AppState::new_with_plugin_dir(storage, dir.join("plugins"));
        let kept = mongodb_config();
        let removed = mq_config("removed-mq", "http://127.0.0.1:8080");
        {
            let mut configs = state.configs.write().await;
            configs.insert(kept.id.clone(), kept.clone());
            configs.insert(removed.id.clone(), removed.clone());
        }
        state.connections.write().await.insert(removed.id.clone(), PoolKind::MessageQueue);

        save_connection_configs(&state, &[kept.clone()]).await.unwrap();

        assert!(!state.connections.read().await.contains_key(&removed.id));

        let _ = std::fs::remove_dir_all(dir);
    }
}

#[tauri::command]
pub async fn save_connections(state: State<'_, Arc<AppState>>, configs: Vec<ConnectionConfig>) -> Result<(), String> {
    let configs: Vec<ConnectionConfig> = configs.into_iter().map(|config| config.canonicalized()).collect();
    save_connection_configs(state.inner(), &configs).await
}

async fn save_connection_configs(state: &AppState, configs: &[ConnectionConfig]) -> Result<(), String> {
    state.storage.save_connections(configs).await?;
    let sync = sync_connection_configs(state, configs).await;
    remove_connection_pools_for_connection_ids(state, &sync.connection_pool_ids_to_drop).await;
    drop_nacos_adapters_for_connection_ids(state, &sync.nacos_adapter_ids_to_drop).await;
    drop_mq_adapters_for_connection_ids(state, &sync.mq_adapter_ids_to_drop).await;
    Ok(())
}

struct ConnectionConfigSync {
    nacos_adapter_ids_to_drop: Vec<String>,
    mq_adapter_ids_to_drop: Vec<String>,
    connection_pool_ids_to_drop: Vec<String>,
}

async fn sync_connection_configs(state: &AppState, configs: &[ConnectionConfig]) -> ConnectionConfigSync {
    let saved_ids: HashSet<&str> = configs.iter().map(|config| config.id.as_str()).collect();
    let mut nacos_adapter_ids_to_drop = HashSet::new();
    let mut mq_adapter_ids_to_drop = HashSet::new();
    let mut connection_pool_ids_to_drop = HashSet::new();
    let mut runtime_configs = state.configs.write().await;
    runtime_configs.retain(|id, existing| {
        if saved_ids.contains(id.as_str()) || is_transient_runtime_config_id(id) {
            true
        } else {
            connection_pool_ids_to_drop.insert(id.clone());
            if existing.db_type == DatabaseType::Nacos {
                nacos_adapter_ids_to_drop.insert(id.clone());
            }
            if existing.db_type == DatabaseType::MessageQueue {
                mq_adapter_ids_to_drop.insert(id.clone());
            }
            false
        }
    });
    for config in configs {
        if config.db_type == DatabaseType::Nacos {
            nacos_adapter_ids_to_drop.insert(config.id.clone());
        }
        if config.db_type == DatabaseType::MessageQueue {
            mq_adapter_ids_to_drop.insert(config.id.clone());
        }
        if let Some(previous) = runtime_configs.insert(config.id.clone(), config.clone()) {
            if previous.db_type == DatabaseType::Nacos {
                nacos_adapter_ids_to_drop.insert(config.id.clone());
            }
            if previous.db_type == DatabaseType::MessageQueue {
                mq_adapter_ids_to_drop.insert(config.id.clone());
            }
            if &previous != config {
                connection_pool_ids_to_drop.insert(config.id.clone());
            }
        }
    }
    ConnectionConfigSync {
        nacos_adapter_ids_to_drop: nacos_adapter_ids_to_drop.into_iter().collect(),
        mq_adapter_ids_to_drop: mq_adapter_ids_to_drop.into_iter().collect(),
        connection_pool_ids_to_drop: connection_pool_ids_to_drop.into_iter().collect(),
    }
}

fn is_transient_runtime_config_id(id: &str) -> bool {
    id.starts_with("__test_") || id.starts_with("__visible_draft_") || id.starts_with("__visible_schema_draft_")
}

async fn drop_nacos_adapters_for_connection_ids(state: &AppState, connection_ids: &[String]) {
    for connection_id in connection_ids {
        state.nacos_registry.drop_connection(connection_id).await;
    }
}

#[cfg(feature = "mq-admin")]
async fn drop_mq_adapters_for_connection_ids(state: &AppState, connection_ids: &[String]) {
    for connection_id in connection_ids {
        state.mq_registry.drop_connection(connection_id).await;
    }
}

#[cfg(not(feature = "mq-admin"))]
async fn drop_mq_adapters_for_connection_ids(_state: &AppState, _connection_ids: &[String]) {}

async fn remove_connection_pools_for_connection_ids(state: &AppState, connection_ids: &[String]) {
    for connection_id in connection_ids {
        state.remove_connection_pools_detached(connection_id).await;
    }
}

#[tauri::command]
pub async fn load_connections(state: State<'_, Arc<AppState>>) -> Result<Vec<ConnectionConfig>, String> {
    load_connection_configs(state.inner()).await
}

async fn load_connection_configs(state: &AppState) -> Result<Vec<ConnectionConfig>, String> {
    let configs: Vec<ConnectionConfig> =
        state.storage.load_connections().await?.into_iter().map(|config| config.canonicalized()).collect();
    let sync = sync_connection_configs(state, &configs).await;
    remove_connection_pools_for_connection_ids(state, &sync.connection_pool_ids_to_drop).await;
    drop_nacos_adapters_for_connection_ids(state, &sync.nacos_adapter_ids_to_drop).await;
    drop_mq_adapters_for_connection_ids(state, &sync.mq_adapter_ids_to_drop).await;
    Ok(configs)
}

#[tauri::command]
pub async fn save_sidebar_layout(state: State<'_, Arc<AppState>>, layout: serde_json::Value) -> Result<(), String> {
    state.storage.save_sidebar_layout(&layout).await
}

#[tauri::command]
pub async fn load_sidebar_layout(state: State<'_, Arc<AppState>>) -> Result<Option<serde_json::Value>, String> {
    state.storage.load_sidebar_layout().await
}

#[tauri::command]
pub async fn test_connection(state: State<'_, Arc<AppState>>, config: ConnectionConfig) -> Result<String, String> {
    // Shared with web: driver match lives in connection_lifecycle (PR-A3).
    dbx_core::connection_lifecycle::test_connection(state.inner().as_ref(), config).await
}

#[tauri::command]
pub async fn test_connection_with_info(
    state: State<'_, Arc<AppState>>,
    config: ConnectionConfig,
) -> Result<ConnectionTestResult, String> {
    // Database-info enrichment is optional for adapters that only need connectivity.
    // Full with-info dispatch remains a follow-up once lifecycle returns ConnectionTestResult.
    let message = dbx_core::connection_lifecycle::test_connection(state.inner().as_ref(), config).await?;
    Ok(ConnectionTestResult::success(message))
}

#[tauri::command]
pub async fn connect_db(
    state: State<'_, Arc<AppState>>,
    config: ConnectionConfig,
    client_attempt: Option<u64>,
) -> Result<String, String> {
    dbx_core::connection_lifecycle::connect(state.inner().as_ref(), config, client_attempt).await
}

#[tauri::command]
pub async fn connection_final_proxy_port(
    state: State<'_, Arc<AppState>>,
    config: ConnectionConfig,
) -> Result<u16, String> {
    let runtime_config = config.canonicalized();
    if !runtime_config.has_effective_transport_layers() {
        return Err("Connection has no configured transport layers".to_string());
    }

    let connection_id = runtime_config.id.clone();
    let db_config = metadata_connection_config(&runtime_config);
    state.configs.write().await.insert(connection_id.clone(), runtime_config);

    let (_, port) = state.connection_host_port(&connection_id, &db_config).await?;
    Ok(port)
}

#[tauri::command]
pub async fn disconnect_db(
    state: State<'_, Arc<AppState>>,
    connection_id: String,
    client_attempt: Option<u64>,
) -> Result<(), String> {
    let should_disconnect = if let Some(client_attempt) = client_attempt {
        state.supersede_connection_attempt_if_client_attempt(&connection_id, client_attempt).await
    } else {
        state.supersede_connection_attempt(&connection_id).await;
        true
    };
    if !should_disconnect {
        return Ok(());
    }
    state.running_queries.cancel_connection(&connection_id);
    state.remove_connection_pools_detached(&connection_id).await;
    drop_nacos_adapters_for_connection_ids(state.inner(), std::slice::from_ref(&connection_id)).await;
    drop_mq_adapters_for_connection_ids(state.inner(), std::slice::from_ref(&connection_id)).await;
    state.reset_connection_transport(&connection_id).await;
    if connection_id.starts_with("__visible_draft_") || connection_id.starts_with("__visible_schema_draft_") {
        state.configs.write().await.remove(&connection_id);
    }
    Ok(())
}

#[tauri::command]
pub async fn close_database_connection(
    state: State<'_, Arc<AppState>>,
    connection_id: String,
    database: String,
) -> Result<bool, String> {
    let database = database.trim();
    let database = if database.is_empty() { None } else { Some(database) };
    state.close_database_pool(&connection_id, database).await
}

#[tauri::command]
pub async fn refresh_connections(state: State<'_, Arc<AppState>>) -> Result<(), String> {
    state.refresh_connections().await;
    Ok(())
}

#[tauri::command]
pub async fn check_connection_health(state: State<'_, Arc<AppState>>, connection_id: String) -> Result<(), String> {
    state.check_connection_health(&connection_id).await
}

#[tauri::command]
pub async fn connection_identifier_quote(
    state: State<'_, Arc<AppState>>,
    connection_id: String,
    database: Option<String>,
) -> Result<Option<String>, String> {
    state.connection_identifier_quote(&connection_id, database.as_deref()).await
}

#[tauri::command]
pub async fn connection_database_info(
    state: State<'_, Arc<AppState>>,
    connection_id: String,
    database: Option<String>,
) -> Result<Option<DatabaseConnectionInfo>, String> {
    state.connection_database_info(&connection_id, database.as_deref()).await
}

#[tauri::command]
pub async fn save_connection_database_info(
    state: State<'_, Arc<AppState>>,
    connection_id: String,
    database_info: Option<DatabaseConnectionInfo>,
) -> Result<(), String> {
    state.save_connection_database_info(&connection_id, database_info).await
}

/// Check whether a connection has read-only protection enabled.
/// Returns an error if the connection is read-only, preventing write operations.
pub async fn ensure_connection_writable(
    state: &Arc<AppState>,
    connection_id: &str,
    action: &str,
) -> Result<(), String> {
    if let Some(name) = dbx_core::query::connection_readonly_name(state, connection_id).await {
        return Err(format!(
            "Read-only mode: connection '{}' has read-only protection enabled. {} blocked.",
            name, action
        ));
    }
    Ok(())
}
