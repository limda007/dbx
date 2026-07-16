//! Database session facade (Phase B).
//!
//! Hides `PoolKind` dispatch behind a small surface so query/schema/transfer
//! callers do not match driver variants. See
//! `docs/pips/plans/2026-07-15-phase-b-database-session.md`.

mod execute;
mod schema;
mod transfer;

pub(crate) use execute::execute_sql;
pub(crate) use schema::{
    get_columns, get_table_ddl, list_available_extensions, list_completion_objects, list_databases, list_extensions,
    list_foreign_keys, list_functions, list_indexes, list_object_statistics, list_objects, list_owners, list_rules,
    list_schemas, list_sequences, list_tables, list_triggers,
};
pub(crate) use transfer::{execute_transfer_sql, get_columns_for_transfer, stream_native_table_rows};
