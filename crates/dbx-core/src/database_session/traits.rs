//! B5 capability traits for native SQL families (Postgres + MySQL first).
//!
//! Free functions in [`super::execute`] / [`super::schema`] / [`super::transfer`]
//! still own multi-driver dispatch. These traits are the extension seam for the
//! two first-class wire protocols so product code can depend on capabilities
//! instead of concrete pool types when only PG/MySQL matter.
//!
//! Other drivers stay on internal `PoolKind` arms (no dyn mega-object required).

use async_trait::async_trait;

use crate::db;
use crate::db::mysql::MySqlQueryDialect;
use crate::types::{DatabaseInfo, TableInfo};

/// Run SQL with an optional row cap (transfer / ad-hoc execute helpers).
#[async_trait]
pub(crate) trait SqlExecute: Send + Sync {
    async fn execute_with_max_rows(&self, sql: &str, max_rows: Option<usize>) -> Result<db::QueryResult, String>;
}

/// Schema tree browse for a single live pool (databases / schemas / tables).
///
/// Product special cases (Doris SHOW, StarRocks, QuestDB, OceanBase Oracle mode)
/// stay outside the trait and are handled by session free functions before or
/// instead of these defaults.
#[async_trait]
pub(crate) trait SchemaBrowse: Send + Sync {
    async fn list_databases(&self) -> Result<Vec<DatabaseInfo>, String>;

    async fn list_schemas(&self) -> Result<Vec<String>, String>;

    async fn list_tables(
        &self,
        database: &str,
        schema: &str,
        filter: Option<&str>,
        limit: Option<usize>,
        offset: Option<usize>,
        object_types: Option<&[String]>,
    ) -> Result<Vec<TableInfo>, String>;
}

/// Combined SQL execute + schema browse (Postgres / MySQL session handles).
///
/// Marker for static bounds / tests; product code often uses the split traits.
#[allow(dead_code)] // intentional B5 marker — implemented by session handles
pub(crate) trait SqlSession: SqlExecute + SchemaBrowse {}

/// PostgreSQL (and PG-wire cousins that share the deadpool handle) session.
#[derive(Clone)]
pub(crate) struct PostgresSession {
    pool: deadpool_postgres::Pool,
}

impl PostgresSession {
    pub(crate) fn new(pool: deadpool_postgres::Pool) -> Self {
        Self { pool }
    }

    #[allow(dead_code)] // B5 seam: callers may need the raw pool for cancel/stream.
    pub(crate) fn pool(&self) -> &deadpool_postgres::Pool {
        &self.pool
    }
}

#[async_trait]
impl SqlExecute for PostgresSession {
    async fn execute_with_max_rows(&self, sql: &str, max_rows: Option<usize>) -> Result<db::QueryResult, String> {
        db::postgres::execute_query_with_max_rows(&self.pool, sql, max_rows).await
    }
}

#[async_trait]
impl SchemaBrowse for PostgresSession {
    async fn list_databases(&self) -> Result<Vec<DatabaseInfo>, String> {
        db::postgres::list_databases(&self.pool).await
    }

    async fn list_schemas(&self) -> Result<Vec<String>, String> {
        db::postgres::list_schemas(&self.pool).await
    }

    async fn list_tables(
        &self,
        _database: &str,
        schema: &str,
        filter: Option<&str>,
        limit: Option<usize>,
        offset: Option<usize>,
        object_types: Option<&[String]>,
    ) -> Result<Vec<TableInfo>, String> {
        if object_types.is_some() {
            db::postgres::list_tables_filtered(&self.pool, schema, filter, None, None)
                .await
                .map(|tables| crate::schema::filter_table_infos(tables, filter, limit, offset, object_types))
        } else {
            db::postgres::list_tables_filtered(&self.pool, schema, filter, limit, offset).await
        }
    }
}

impl SqlSession for PostgresSession {}

/// MySQL-protocol session (includes bare / pooled modes used by transfer).
#[derive(Clone)]
pub(crate) struct MysqlSession {
    pool: crate::db::mysql::MySqlPool,
    bare: bool,
    dialect: MySqlQueryDialect,
}

impl MysqlSession {
    pub(crate) fn new(pool: crate::db::mysql::MySqlPool, bare: bool) -> Self {
        Self { pool, bare, dialect: MySqlQueryDialect::default() }
    }

    #[allow(dead_code)] // B5 seam: dialect override for transfer/agent profiles.
    pub(crate) fn with_dialect(mut self, dialect: MySqlQueryDialect) -> Self {
        self.dialect = dialect;
        self
    }

    #[allow(dead_code)] // B5 seam: raw pool for checkout helpers.
    pub(crate) fn pool(&self) -> &crate::db::mysql::MySqlPool {
        &self.pool
    }

    #[allow(dead_code)] // B5 seam: bare text-protocol flag.
    pub(crate) fn bare(&self) -> bool {
        self.bare
    }
}

#[async_trait]
impl SqlExecute for MysqlSession {
    async fn execute_with_max_rows(&self, sql: &str, max_rows: Option<usize>) -> Result<db::QueryResult, String> {
        db::mysql::execute_query_with_max_rows(&self.pool, sql, self.bare, max_rows, self.dialect).await
    }
}

#[async_trait]
impl SchemaBrowse for MysqlSession {
    async fn list_databases(&self) -> Result<Vec<DatabaseInfo>, String> {
        db::mysql::list_databases(&self.pool).await
    }

    async fn list_schemas(&self) -> Result<Vec<String>, String> {
        // MySQL catalogs are databases; schemas are usually empty at this layer.
        Ok(vec![])
    }

    async fn list_tables(
        &self,
        database: &str,
        schema: &str,
        filter: Option<&str>,
        limit: Option<usize>,
        offset: Option<usize>,
        object_types: Option<&[String]>,
    ) -> Result<Vec<TableInfo>, String> {
        db::mysql::list_tables_filtered(
            &self.pool,
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

impl SqlSession for MysqlSession {}

#[cfg(test)]
mod tests {
    use super::{MysqlSession, PostgresSession, SchemaBrowse, SqlExecute, SqlSession};

    #[test]
    fn sql_session_is_object_safe_as_dyn_pair() {
        // Compile-time: both families implement the combined marker trait.
        fn assert_sql_session<T: SqlSession>() {}
        assert_sql_session::<PostgresSession>();
        assert_sql_session::<MysqlSession>();
    }

    #[test]
    fn trait_object_bounds_hold_for_execute_and_browse() {
        fn assert_execute<T: SqlExecute>() {}
        fn assert_browse<T: SchemaBrowse>() {}
        assert_execute::<PostgresSession>();
        assert_execute::<MysqlSession>();
        assert_browse::<PostgresSession>();
        assert_browse::<MysqlSession>();
    }

    #[test]
    fn session_type_names_are_stable() {
        // Real pools need a server; assert type surface only.
        assert!(std::any::type_name::<MysqlSession>().contains("MysqlSession"));
        assert!(std::any::type_name::<PostgresSession>().contains("PostgresSession"));
    }
}
