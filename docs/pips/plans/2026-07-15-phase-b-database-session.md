# Phase B: Hide `PoolKind` Behind DatabaseSession

**Status:** **Done** (B1–B4 + B2.1 schema tree; B5 traits deferred)  
**Date:** 2026-07-15 / closed 2026-07-16  
**Branch:** `feat/connection-lifecycle`

**Anchors:**

- Architecture review candidate #2: *Hide `PoolKind` behind driver traits / `DatabaseSession`*
- Phase A (done): [2026-07-14-phase-a-connection-lifecycle.md](./2026-07-14-phase-a-connection-lifecycle.md)
- [PIP-0001](../PIP-0001-database-connection-timeout-recovery.md) (lifecycle budgets remain; B is about dispatch depth)

## Goal

Deepen a **DatabaseSession** seam so that:

- query / schema / transfer / export do **not** `match PoolKind`
- driver-specific checkout, execute, list-*, cancel glue live behind one session interface
- `PoolKind` becomes an **implementation detail** of connection registration (crate-private over time)

Phase A owns *when* and *how long* pool ops run (budget, stage, cleanup).  
Phase B owns *which driver code* runs for a live connection (dispatch).

## Baseline (2026-07-15)

| Surface | Approx. `PoolKind::` refs | Notes |
| --- | --- | --- |
| `connection.rs` | ~154 | Factory + map owner — stays the registry |
| `schema.rs` + `schema/providers/native.rs` | ~180 | Many list_* matches |
| `query.rs` | ~68 | **`do_execute` mega-match** is the #1 seam |
| `mongo_ops` / `redis_ops` / `document_ops` | ~90 | Domain-specific; later slices |
| `transfer.rs` | ~22 | Secondary |
| `connection_lifecycle/*` | ~22 | Connect/cleanup may keep private matches |

**Deletion test:** Removing `DatabaseSession` re-spreads `match PoolKind` into every product path. Removing a single `query.rs` arm should no longer be the place where “how to run SQL for MySQL vs PG” lives.

## Non-goals (Phase B)

- Do **not** rewrite all agent / plugin drivers in one PR.
- Do **not** change SQL semantics, timeouts, or PIP budgets (call lifecycle as today).
- Do **not** force `async_trait` dyn objects if enum/static dispatch is clearer; trait *or* private enum behind methods is fine.
- Do **not** merge Phase B into a single big-bang PR; ship vertical slices like Phase A.

## Target architecture

```
UI / Tauri / web
       │
       ▼
query / schema / transfer  ──► DatabaseSession (deep module)
       │                              │
       │                              ├── resolve(pool_key) → session handle
       │                              ├── execute / list_* / transfer helpers
       │                              ├── uses connection_lifecycle budgets
       │                              └── private: PoolKind + db/* adapters
       ▼
AppState.connections: HashMap<String, PoolKind>   (registry only)
```

**Preferred shape (Rust):**

1. **B1–B3:** static dispatch — `database_session::execute(...)` owns the match; callers never name `PoolKind`.
2. **B4+:** optional `trait SqlSession` / capability traits for PG+MySQL first; other variants stay internal enum arms.
3. **End state:** `PoolKind` is `pub(crate)` (or module-private); only `connection` + `database_session` construct/match it.

## Module layout (proposed)

```
crates/dbx-core/src/
  database_session/
    mod.rs           // public facade: resolve, execute, list helpers
    execute.rs       // moved do_execute driver match
    schema.rs        // list_databases / list_tables / … (later)
    transfer.rs      // later
    handle.rs        // optional SessionHandle / capabilities
  query.rs           // orchestration + multi-statement + lifecycle logs → calls session
  schema.rs          // thin wrappers → session
  connection.rs      // keeps PoolKind + insert/remove
```

## PR plan (small slices)

### PR-B1 — Session facade + move `do_execute` dispatch ✅

**Intent:** Create the seam; main SQL execute path no longer matches `PoolKind` in `query.rs`.

**Files:**

- add `database_session/{mod,execute}.rs`
- `lib.rs` `pub mod database_session`
- move `do_execute`’s `match pool` body into `database_session::execute_sql`
- `query::do_execute` keeps: activity touch, budget, read-only check, stage logs, then calls session
- `ExecuteSqlError::{AlreadyLogged, Unlogged}` preserves Phase A single end-log semantics

**Acceptance (verified 2026-07-15):**

- `query.rs` ~45 residual `PoolKind::` (txn/agent/helpers); execute mega-match lives in `database_session/execute.rs` (~23 arms)
- `do_execute` → `database_session::execute_sql`
- Safe: `connection_lifecycle` 26 ok, `query_cancel` 4 ok (`mq-admin`)
- `cargo check -p dbx-core --lib` (default + duckdb-bundled) green
- no intentional behavior change

**Out of scope:** schema/transfer; making `PoolKind` private.

---

### PR-B2 — Schema hot paths via session ✅

**Intent:** `list_databases` / `list_schemas` / `list_tables` (and primary tree loaders) call session helpers.

**Files:**

- add `database_session/schema.rs` with `list_databases` / `list_schemas` / `list_tables`
- `schema::{list_databases_once,list_schemas_once,list_tables_once}` final multi-arm matches → session
- orchestration (retry, ExternalDriver, Agent, SqlServer, ClickHouse, DuckDb early paths) stays in `schema.rs`
- note: `schema/providers/native.rs` is **not compiled** (orphan under `schema/` dir while `schema.rs` is the module root); real seam is `schema.rs`

**Acceptance (verified 2026-07-15):**

- final `match pool` for the three list hot paths lives in `database_session/schema.rs` (~23 arms)
- `schema.rs` residual `PoolKind::` ~81 (agent/external/columns/objects/DDL — later slices)
- `cargo check` mq-admin + default/duckdb green
- `schema::tests` 42 ok; `connection_lifecycle` still green
- external driver / plugin path still handled before session dispatch

---

### PR-B3 — Transfer + export checkout via session ✅

**Intent:** transfer/export obtain driver handles through session APIs (budgeted checkout already from A).

**Files:**

- add `database_session/transfer.rs`:
  - `execute_transfer_sql` — transfer `execute_on_pool_once` multi-arm match
  - `get_columns_for_transfer` — transfer column lookup dispatch
  - `stream_native_table_rows` — table-export MySQL/PG/SqlServer stream path
- `transfer.rs` / `table_export.rs` keep retries, agent session, cancel, product orchestration

**Acceptance (verified 2026-07-15):**

- transfer multi-arm execute + columns + native stream matches live in session (~20 `PoolKind::`)
- residual: transfer PG-only sequence helpers (4), table_export Agent session (4), query_result_export typed streams (later)
- `cargo check` mq-admin + default/duckdb green
- `transfer::tests` 109 ok; `table_export::tests` 19 ok; `connection_lifecycle` 26 ok

---

### PR-B4 — Shrink public `PoolKind` ✅

**Intent:** `PoolKind` `pub(crate)` or re-export only for tests; Tauri/web never import it.

**Changes:**

- `PoolKind` → `pub(crate)`
- `AppState.connections` → `pub(crate)`
- `insert_connection_pool` / `insert_connection_pool_for_attempt` / `external_driver_pool` /
  `connect_sqlserver_pool_with_legacy_fallback` / `close_pool_kind` → `pub(crate)`
- Public helpers without exposing `PoolKind`:
  - `has_pool`, `insert_message_queue_pool_marker`, `insert_sqlite_pool`,
    `insert_postgres_pool`, `insert_sqlserver_pool`, `sqlserver_client`, `sqlite_pool_if_open`
- Tauri `sqlite_backup` uses `sqlite_pool_if_open`
- Tauri/web/live tests no longer name `PoolKind` or touch `.connections`

**Gate (verified 2026-07-15):**

```text
rg 'PoolKind' apps/ src-tauri/ crates/dbx-web/ crates/dbx-core/tests  # empty
rg '\.connections' src-tauri/ crates/dbx-web/ crates/dbx-core/tests   # empty
```

**Checks:** `dbx-core` mq-admin + default green; `dbx-web` mq-admin green.
(`cargo check -p dbx` needs host glib for Tauri; not a B4 regression.)

---

### PR-B2.1 — Schema tree residual hot paths ✅

**Intent:** After B2 list_* , move remaining tree multi-arm matches into session.

**Session APIs added:** `list_objects`, `list_completion_objects`, `list_object_statistics`,
`get_columns`, `list_indexes`, `list_foreign_keys`, `list_triggers`, `get_table_ddl`,
plus PG extras (`list_functions` / `sequences` / `rules` / `extensions` / `owners`).

**Acceptance (verified 2026-07-16):**

- `schema.rs` residual `PoolKind::` ~31 (agent/external/early paths, completion helpers, tests)
- `database_session/schema.rs` ~74 arms (owns final native dispatch)
- `schema::tests` 42 ok; lifecycle green

---

### PR-B5 — Domain capability handles (landed 2026-07-21)

**Intent:** Hide `PoolKind` from domain ops without forcing `async_trait` dyn objects.

**Approach (capability enums, not traits):**

- `database_session/domain.rs`:
  - `MongoHandle` / `DocumentHandle` — Clone capability enums
  - `resolve_mongo_handle` / `resolve_document_handle`
  - `with_redis!` macro — Redis is not `Clone`; body runs under registry read lock
  - `resolve_postgres_pool` / `resolve_mysql_pool` / `resolve_clickhouse_client` /
    `resolve_sqlserver_client` for typed export streams
- `mongo_ops` / `document_ops` match handles only
- `redis_ops` uses `with_redis!` only
- `query_result_export` typed streams use resolve helpers (no `PoolKind` at call site)

**Decision note:** Full `SqlExecute` / `SchemaBrowse` traits remain optional. Domain
handles give the same deletion-test property (product files do not name `PoolKind`)
with less indirection than dyn traits.

## Compatibility rules

1. Keep Phase A budgets and stage log vocabulary.
2. `query_timeout_secs = 0` still means unbounded SQL; infra still bounded.
3. MySQL session-scoped pools: no cleanup-after-every-query.
4. Plugin / external driver invoke paths stay timeout-wrapped.

## Verification

```bash
# Safe (WSL) — cargo accepts only one TESTNAME per invocation:
CARGO_BUILD_JOBS=1 cargo test -p dbx-core --lib \
  --no-default-features --features mq-admin -j 1 \
  connection_lifecycle -- --test-threads=1
CARGO_BUILD_JOBS=1 cargo test -p dbx-core --lib \
  --no-default-features --features mq-admin -j 1 \
  schema::tests -- --test-threads=1

# Full when memory allows:
CARGO_BUILD_JOBS=1 cargo test -p dbx-core --lib -j 1 -- --test-threads=1
```

## Definition of done (Phase B)

1. Product hot paths (query execute, schema tree, transfer) do not `match PoolKind` outside `database_session` / `connection`. ✅
2. Deletion test: removing `database_session` re-spreads driver matches into query/schema/transfer. ✅
3. Phase A lifecycle + tests remain green. ✅
4. This plan’s status updated; PIP-0001 notes Phase B progress. ✅

## Residual (out of Phase B)

- `schema.rs` still uses `extract_pool!` / `try_sqlserver!` for ClickHouse / Influx / DuckDb /
  SqlServer / Agent peeks that need lock-held early returns (not always Clone-friendly)
- `query.rs` tests still seed / assert `PoolKind` (DuckDb drain fixtures); product paths use session resolves
- orphan uncompiled `schema/providers/native.rs` (cleanup later)

## Immediate next step

Schema residual slices landed 2026-07-21:

- **S5** `get_object_source` → session
- **S2/S3** Doris `resolve_mysql_pool` + vector `resolve_vector_client`
- **S4** completion assistant waterfall → `try_completion_assistant_search`
- **S6** native table comment → `get_table_comment`
- **S1** ExternalDriver / Agent / Postgres peeks → `resolve_external_driver` /
  `resolve_agent_client` / `resolve_postgres_pool`

Query residual (product peeks) landed 2026-07-21:

- close_query_session / drop-database / multi MySQL+SQL Server / agent batch+explicit txn /
  TxPath / manual txn → `resolve_*` + `TxPath` / `ManualTxnPool` in `database_session/domain`

Next: optional further `extract_pool!` collapse.
