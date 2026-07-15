//! Lifecycle stage identifiers and structured stage logging (PIP-0001 observability).

use std::fmt::{self, Display, Formatter, Write};
use std::time::Duration;

use crate::models::connection::DatabaseType;

/// Named phases of a database operation across connect, query, cancel, and cleanup.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LifecycleStage {
    EnsureConnected,
    PoolCheckout,
    PoolRecycle,
    Ping,
    SchemaSet,
    QueryExecute,
    ResultFetch,
    Cancel,
    Cleanup,
}

impl LifecycleStage {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::EnsureConnected => "ensureConnected",
            Self::PoolCheckout => "pool.checkout",
            Self::PoolRecycle => "pool.recycle",
            Self::Ping => "ping",
            Self::SchemaSet => "schema.set",
            Self::QueryExecute => "query.execute",
            Self::ResultFetch => "result.fetch",
            Self::Cancel => "cancel",
            Self::Cleanup => "cleanup",
        }
    }
}

/// Outcome attached to a stage log line.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum StageOutcome {
    Start,
    Done,
    Error,
    Cancelled,
}

impl StageOutcome {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Start => "start",
            Self::Done => "done",
            Self::Error => "error",
            Self::Cancelled => "cancelled",
        }
    }
}

/// Correlation identifiers shared across stages of one database operation.
#[derive(Debug, Clone, Copy, Default)]
pub struct StageLogContext<'a> {
    pub connection_id: Option<&'a str>,
    pub pool_key: Option<&'a str>,
    pub db_type: Option<&'a str>,
    pub trace_id: Option<&'a str>,
    pub client_session_id: Option<&'a str>,
}

impl<'a> StageLogContext<'a> {
    pub const fn empty() -> Self {
        Self { connection_id: None, pool_key: None, db_type: None, trace_id: None, client_session_id: None }
    }

    /// Build correlation fields for a pool-backed operation.
    ///
    /// `db_type` must be the configured product name (e.g. `opengauss`, `redshift`,
    /// `questdb`), not the pool adapter. Use [`database_type_log_label`] so PG-wire
    /// cousins are not all logged as `postgres`.
    pub fn for_pool(pool_key: Option<&'a str>, trace_id: Option<&'a str>, db_type: Option<&'a str>) -> Self {
        let connection_id = pool_key.map(connection_id_from_pool_key).filter(|s| !s.is_empty());
        Self {
            connection_id,
            pool_key: pool_key.filter(|s| !s.is_empty()),
            db_type: db_type.filter(|s| !s.is_empty()),
            trace_id: trace_id.filter(|s| !s.is_empty()),
            client_session_id: None,
        }
    }
}

/// Best-effort connection id from a pool key (`connection_id` or `connection_id:database`).
pub fn connection_id_from_pool_key(pool_key: &str) -> &str {
    pool_key.split(':').next().unwrap_or(pool_key)
}

/// Stable log label for a configured [`DatabaseType`] (serde rename, lowercased).
///
/// Examples: `postgres`, `opengauss`, `questdb`, `gaussdb`, `redshift`, `kwdb`.
pub fn database_type_log_label(db_type: DatabaseType) -> String {
    serde_json::to_value(db_type)
        .ok()
        .and_then(|value| value.as_str().map(|s| s.to_ascii_lowercase()))
        .unwrap_or_else(|| format!("{db_type:?}").to_ascii_lowercase())
}

/// Optional form of [`database_type_log_label`].
pub fn optional_database_type_log_label(db_type: Option<DatabaseType>) -> Option<String> {
    db_type.map(database_type_log_label)
}

/// Fields for a single lifecycle stage log event.
#[derive(Debug, Clone, Copy)]
pub struct StageLog<'a> {
    pub stage: LifecycleStage,
    pub outcome: StageOutcome,
    pub elapsed_ms: u128,
    pub timeout_ms: Option<u128>,
    pub connection_id: Option<&'a str>,
    pub pool_key: Option<&'a str>,
    pub db_type: Option<&'a str>,
    pub trace_id: Option<&'a str>,
    pub client_session_id: Option<&'a str>,
    pub error: Option<&'a str>,
}

impl<'a> StageLog<'a> {
    pub fn new(stage: LifecycleStage, outcome: StageOutcome, elapsed_ms: u128) -> Self {
        Self {
            stage,
            outcome,
            elapsed_ms,
            timeout_ms: None,
            connection_id: None,
            pool_key: None,
            db_type: None,
            trace_id: None,
            client_session_id: None,
            error: None,
        }
    }

    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout_ms = Some(timeout.as_millis());
        self
    }

    pub fn with_timeout_ms(mut self, timeout_ms: u128) -> Self {
        self.timeout_ms = Some(timeout_ms);
        self
    }

    pub fn with_connection_id(mut self, connection_id: &'a str) -> Self {
        self.connection_id = Some(connection_id);
        self
    }

    pub fn with_pool_key(mut self, pool_key: &'a str) -> Self {
        self.pool_key = Some(pool_key);
        self
    }

    pub fn with_db_type(mut self, db_type: &'a str) -> Self {
        self.db_type = Some(db_type);
        self
    }

    pub fn with_trace_id(mut self, trace_id: &'a str) -> Self {
        self.trace_id = Some(trace_id);
        self
    }

    pub fn with_client_session_id(mut self, client_session_id: &'a str) -> Self {
        self.client_session_id = Some(client_session_id);
        self
    }

    pub fn with_error(mut self, error: &'a str) -> Self {
        self.error = Some(error);
        self
    }

    pub fn with_context(mut self, context: StageLogContext<'a>) -> Self {
        if self.connection_id.is_none() {
            self.connection_id = context.connection_id;
        }
        if self.pool_key.is_none() {
            self.pool_key = context.pool_key;
        }
        if self.db_type.is_none() {
            self.db_type = context.db_type;
        }
        if self.trace_id.is_none() {
            self.trace_id = context.trace_id;
        }
        if self.client_session_id.is_none() {
            self.client_session_id = context.client_session_id;
        }
        self
    }
}

/// Lazy formatter used by [`log_stage`] so disabled log levels avoid allocation.
struct StageLogDisplay<'a>(&'a StageLog<'a>);

impl Display for StageLogDisplay<'_> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write_stage_log(f, self.0)
    }
}

fn write_stage_log(f: &mut impl Write, fields: &StageLog<'_>) -> fmt::Result {
    write!(f, "[db:{}:{}] elapsed_ms={}", fields.stage.as_str(), fields.outcome.as_str(), fields.elapsed_ms)?;
    if let Some(timeout_ms) = fields.timeout_ms {
        write!(f, " timeout_ms={timeout_ms}")?;
    }
    if let Some(trace_id) = fields.trace_id.filter(|s| !s.is_empty()) {
        write!(f, " trace_id={trace_id}")?;
    }
    if let Some(connection_id) = fields.connection_id.filter(|s| !s.is_empty()) {
        write!(f, " connection_id={connection_id}")?;
    }
    if let Some(pool_key) = fields.pool_key.filter(|s| !s.is_empty()) {
        write!(f, " pool_key={pool_key}")?;
    }
    if let Some(db_type) = fields.db_type.filter(|s| !s.is_empty()) {
        write!(f, " db_type={db_type}")?;
    }
    if let Some(client_session_id) = fields.client_session_id.filter(|s| !s.is_empty()) {
        write!(f, " client_session_id={client_session_id}")?;
    }
    if let Some(error) = fields.error.filter(|s| !s.is_empty()) {
        write!(f, " error={error}")?;
    }
    Ok(())
}

/// Format a stage log line without emitting it (for tests and custom sinks).
pub fn format_stage_log(fields: &StageLog<'_>) -> String {
    let mut out = String::with_capacity(160);
    let _ = write_stage_log(&mut out, fields);
    out
}

/// Emit a structured lifecycle stage log at an appropriate level for the outcome.
///
/// Formatting is skipped when the target log level is disabled so hot paths such as
/// successful checkout do not allocate under typical production log configuration.
pub fn log_stage(fields: StageLog<'_>) {
    match fields.outcome {
        StageOutcome::Error if log::log_enabled!(log::Level::Warn) => {
            log::warn!("{}", StageLogDisplay(&fields));
        }
        StageOutcome::Cancelled if log::log_enabled!(log::Level::Info) => {
            log::info!("{}", StageLogDisplay(&fields));
        }
        StageOutcome::Start | StageOutcome::Done if log::log_enabled!(log::Level::Debug) => {
            log::debug!("{}", StageLogDisplay(&fields));
        }
        StageOutcome::Error | StageOutcome::Cancelled | StageOutcome::Start | StageOutcome::Done => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stage_names_match_pip_vocabulary() {
        assert_eq!(LifecycleStage::EnsureConnected.as_str(), "ensureConnected");
        assert_eq!(LifecycleStage::PoolCheckout.as_str(), "pool.checkout");
        assert_eq!(LifecycleStage::PoolRecycle.as_str(), "pool.recycle");
        assert_eq!(LifecycleStage::Ping.as_str(), "ping");
        assert_eq!(LifecycleStage::SchemaSet.as_str(), "schema.set");
        assert_eq!(LifecycleStage::QueryExecute.as_str(), "query.execute");
        assert_eq!(LifecycleStage::ResultFetch.as_str(), "result.fetch");
        assert_eq!(LifecycleStage::Cancel.as_str(), "cancel");
        assert_eq!(LifecycleStage::Cleanup.as_str(), "cleanup");
    }

    #[test]
    fn format_stage_log_sequence_is_parseable_for_hung_query_triage() {
        // Engineer workflow: last stage with matching trace_id names the stuck phase.
        let lines = [
            format_stage_log(
                &StageLog::new(LifecycleStage::QueryExecute, StageOutcome::Start, 0)
                    .with_trace_id("exec-9")
                    .with_connection_id("c1")
                    .with_pool_key("c1:app")
                    .with_db_type("postgres"),
            ),
            format_stage_log(
                &StageLog::new(LifecycleStage::PoolCheckout, StageOutcome::Error, 10_012)
                    .with_timeout(Duration::from_secs(10))
                    .with_trace_id("exec-9")
                    .with_connection_id("c1")
                    .with_pool_key("c1:app")
                    .with_db_type("postgres")
                    .with_error("checkout timed out"),
            ),
            format_stage_log(
                &StageLog::new(LifecycleStage::QueryExecute, StageOutcome::Error, 10_015)
                    .with_trace_id("exec-9")
                    .with_connection_id("c1")
                    .with_error("checkout timed out"),
            ),
        ];
        assert!(lines[0].starts_with("[db:query.execute:start]"));
        assert!(lines[1].contains("pool.checkout:error"));
        assert!(lines[1].contains("error=checkout timed out"));
        assert!(lines.iter().all(|line| line.contains("trace_id=exec-9")));
        // Stuck stage name from the first error line:
        assert!(lines[1].contains("[db:pool.checkout:error]"));
    }

    #[test]
    fn format_stage_log_includes_core_fields() {
        let line = format_stage_log(
            &StageLog::new(LifecycleStage::PoolCheckout, StageOutcome::Error, 42)
                .with_timeout(Duration::from_secs(5))
                .with_connection_id("conn-1")
                .with_pool_key("conn-1")
                .with_db_type("postgres")
                .with_trace_id("abc")
                .with_error("checkout timed out"),
        );
        assert!(line.starts_with("[db:pool.checkout:error]"));
        assert!(line.contains("elapsed_ms=42"));
        assert!(line.contains("timeout_ms=5000"));
        assert!(line.contains("connection_id=conn-1"));
        assert!(line.contains("pool_key=conn-1"));
        assert!(line.contains("db_type=postgres"));
        assert!(line.contains("trace_id=abc"));
        assert!(line.contains("error=checkout timed out"));
    }

    #[test]
    fn format_stage_log_skips_empty_optional_ids() {
        let line = format_stage_log(
            &StageLog::new(LifecycleStage::Ping, StageOutcome::Done, 1).with_connection_id("").with_trace_id(""),
        );
        assert_eq!(line, "[db:ping:done] elapsed_ms=1");
    }

    #[test]
    fn log_stage_does_not_panic_on_empty_ids() {
        log_stage(StageLog::new(LifecycleStage::Cleanup, StageOutcome::Start, 0));
        log_stage(
            StageLog::new(LifecycleStage::Cancel, StageOutcome::Cancelled, 3).with_connection_id("").with_error(""),
        );
    }

    #[test]
    fn stage_log_context_for_pool_derives_connection_id() {
        let ctx = StageLogContext::for_pool(Some("conn-1:app"), Some("exec-9"), Some("opengauss"));
        assert_eq!(ctx.connection_id, Some("conn-1"));
        assert_eq!(ctx.pool_key, Some("conn-1:app"));
        assert_eq!(ctx.db_type, Some("opengauss"));
        assert_eq!(ctx.trace_id, Some("exec-9"));
    }

    #[test]
    fn database_type_log_label_uses_serde_rename_lowercased() {
        assert_eq!(database_type_log_label(DatabaseType::Postgres), "postgres");
        // PoolKind::Postgres also hosts these products — labels must stay distinct.
        assert_eq!(database_type_log_label(DatabaseType::OpenGauss), "opengauss");
        assert_eq!(database_type_log_label(DatabaseType::Questdb), "questdb");
        assert_eq!(database_type_log_label(DatabaseType::Redshift), "redshift");
        assert_eq!(database_type_log_label(DatabaseType::Gaussdb), "gaussdb");
        assert_eq!(database_type_log_label(DatabaseType::Kwdb), "kwdb");
    }

    #[test]
    fn with_context_fills_missing_correlation_fields() {
        let line =
            format_stage_log(
                &StageLog::new(LifecycleStage::SchemaSet, StageOutcome::Done, 7)
                    .with_context(StageLogContext::for_pool(Some("c1:db"), Some("t1"), Some("gaussdb"))),
            );
        assert!(line.contains("connection_id=c1"));
        assert!(line.contains("pool_key=c1:db"));
        assert!(line.contains("trace_id=t1"));
        assert!(line.contains("db_type=gaussdb"));
        assert!(!line.contains("db_type=postgres"));
    }
}
