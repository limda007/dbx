//! Database session facade (Phase B).
//!
//! Hides `PoolKind` dispatch behind a small surface so query/schema/transfer
//! and domain ops callers do not match driver variants. See
//! `docs/pips/plans/2026-07-15-phase-b-database-session.md`.

mod domain;
mod execute;
mod schema;
mod transfer;

pub(crate) use domain::{
    is_sqlserver_pool, resolve_agent_client, resolve_clickhouse_client, resolve_document_handle,
    resolve_external_driver, resolve_manual_txn_pool, resolve_mongo_handle, resolve_mysql_pool, resolve_postgres_pool,
    resolve_sqlserver_client, resolve_tx_path, resolve_vector_client, sqlserver_pool_is_current, DocumentHandle,
    ManualTxnPool, MongoHandle, TxPath,
};
pub(crate) use execute::execute_sql;
pub(crate) use schema::{
    get_columns, get_object_source, get_table_comment, get_table_ddl, list_available_extensions,
    list_completion_objects, list_databases, list_extensions, list_foreign_keys, list_functions, list_indexes,
    list_object_statistics, list_objects, list_owners, list_rules, list_schemas, list_sequences, list_tables,
    list_triggers, try_completion_assistant_search,
};
pub(crate) use transfer::{execute_transfer_sql, get_columns_for_transfer, stream_native_table_rows};
