//! Database session facade (Phase B).
//!
//! Hides `PoolKind` dispatch behind a small surface so query/schema/transfer
//! callers do not match driver variants. See
//! `docs/pips/plans/2026-07-15-phase-b-database-session.md`.

mod execute;
mod schema;

pub(crate) use execute::execute_sql;
pub(crate) use schema::{list_databases, list_schemas, list_tables};
