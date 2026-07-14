# Phase A: Collapse Connection Lifecycle Dispatch

**Status:** Ready for implementation  
**Date:** 2026-07-14  
**Anchors:**

- Architecture review candidate #1 (top recommendation): *Collapse connection lifecycle dispatch*
- [PIP-0001](../PIP-0001-database-connection-timeout-recovery.md)
- Existing plan: [2026-06-24-database-connection-timeout-recovery.md](./2026-06-24-database-connection-timeout-recovery.md)

## Goal

Deepen **one connection lifecycle module** in `dbx-core` so that:

- phase ordering (probe → connect → pool register → health → keepalive → cleanup)
- execution budgets
- recovery (stale pool remove, reconnect)
- stage logging

live behind a single seam. Tauri commands, web routes, and the desktop store become **adapters** that call that module—they do not own driver dispatch or timeout policy.

This is architecture review #1. It deliberately does **not** complete #2 (hide `PoolKind` behind driver traits). #2 starts after A lands.

## Why this is still open (baseline audit, 2026-07-14)

PIP-0001 **stage-1 bleed-stop work has largely landed**. Do not re-implement these:

| PIP-0001 item | Current state | Evidence |
| --- | --- | --- |
| `DbOperationBudget` | **Done** | `crates/dbx-core/src/query.rs` (`from_config`, cancel/cleanup hard limits) |
| Postgres deadpool wait/create/recycle timeout | **Done** | `db/postgres.rs` pool builder |
| `checkout_postgres_client` + cancel | **Done** | `db/postgres.rs` |
| Postgres TLS cancel | **Done** | `cancel_postgres_query` + `PostgresCancelContext` |
| MySQL checkout with health + cancel | **Partial / Done on main query path** | `get_conn_with_health_check_with_cancel` used from `query.rs` |
| Frontend `ensureConnected` / health timeout | **Done** | `withConnectionHealthTimeout`, `connectionAttemptTimeoutMs` |
| Frontend cancel UI timeout | **Done** | `withCancelQueryTimeout` in `queryStore` |
| Keepalive default 30s (Rust + dialog) | **Done** | `default_keepalive_interval_secs`, `ConnectionDialog` |

**What is still missing** (Phase A scope):

1. **No lifecycle owner.** Budget, checkout helpers, connect match, health, cleanup, and Tauri `test_connection`/`connect_db` still sit in separate files with duplicated knowledge.
2. **Dual connect dispatch.** `src-tauri/src/commands/connection.rs` still contains a large `match config.db_type` for `test_connection` / connect paths; core `connection.rs` has another connect factory. Deleting either path mostly moves the match—architecture review deletion test fails.
3. **Bare checkout still leaks.** Grep still finds unwrapped `pool.get().await` / `get_conn().await` on secondary paths (`transfer`, `database_export`, `questdb`, `ob_oracle`, `manticoresearch`, some `query.rs` branches, keepalive/health internals).
4. **Health is not a full lifecycle phase.** `AppState::check_connection_health` is mostly “pool exists + stale remove,” while `refresh_connections` has a separate ad-hoc per-`PoolKind` ping ladder—not one budgeted health API.
5. **Stage logging is incomplete / inconsistent.** Some checkout logs exist (`[db:pool.checkout:…]`); there is no single `LifecycleStage` + `trace_id` contract across ensureConnected → checkout → query → cancel → cleanup.
6. **Frontend is not yet a pure adapter.** `connectionStore.ensureConnected` works, but recovery actions (force clear pool / reconnect) and diagnostics are not centralized as lifecycle client operations.

## Non-goals (Phase A)

- Do **not** introduce `DatabaseSession` / driver trait for query/schema/transfer (that is Phase B / review #2).
- Do **not** collapse dialect registries or DataGrid session (reviews #3/#5).
- Do **not** change connection config serde shape unless defaults only (PIP compatibility).
- Do **not** silently change `query_timeout_secs = 0` SQL semantics.
- Do **not** rewrite all agents / non-PG-MySQL drivers; first-class coverage remains PostgreSQL, openGauss, MySQL, with shared lifecycle plumbing used by others when cheap.

## Target architecture

```
UI (connectionStore) ──┐
Tauri command adapter ─┼──► ConnectionLifecycle (deep module)
Web route adapter ─────┘         │
                                 ├── budgets (DbOperationBudget)
                                 ├── phases: test | connect | health | checkout | cancel | cleanup
                                 ├── recovery: remove_pool / reconnect policy
                                 ├── stage log (trace_id, stage, elapsed_ms, timeout_ms)
                                 └── driver adapters (existing db/* + PoolKind factory, private)
```

**Deletion test:** Removing `ConnectionLifecycle` re-spreads phase ordering and budgets into every caller. Removing a single Tauri match arm should no longer be the place where connect policy lives.

## Module layout (proposed)

Keep public re-exports stable where possible; new code under:

```
crates/dbx-core/src/
  connection_lifecycle/
    mod.rs                 // public facade
    budget.rs              // re-export or thin wrap of DbOperationBudget
    stage.rs               // LifecycleStage + structured log helper
    health.rs              // budgeted health check / stale classification
    checkout.rs            // postgres/mysql checkout wrappers re-export + audit API
    connect.rs             // connect + test_connection orchestration (move from mega-match over time)
    cleanup.rs             // remove_pool_by_key / close with cleanup_timeout
    recovery.rs            // pool_error_action glue for lifecycle stages
  connection.rs            // AppState + pool map (shrink over PRs; not a big-bang move)
  query.rs                 // keeps SQL execute; calls lifecycle for checkout/budget
```

`src-tauri/src/commands/connection.rs` and `crates/dbx-web/src/routes/connection.rs` should call:

- `connection_lifecycle::test_connection(state, config)`
- `connection_lifecycle::connect(state, config, attempt)`
- `connection_lifecycle::check_health(state, connection_id)`
- `connection_lifecycle::disconnect / close_database_pool`

…instead of owning `match db_type`.

Frontend adapter (later PR in this phase):

- `lib/connection/lifecycleClient.ts` (or keep in store but thin): `ensureConnected`, `forceReconnect`, `checkHealth` with the PIP timeout table as the only place timeouts are defined for UI.

## PR plan (small, mergeable slices)

### PR-A1 — Lifecycle facade + stage logging (no behavior change)

**Intent:** Create the seam and route existing calls through it without changing outcomes.

**Files (expected):**

- add `crates/dbx-core/src/connection_lifecycle/{mod,stage,budget}.rs`
- wire `lib.rs` `pub mod connection_lifecycle`
- move or re-export `DbOperationBudget` from `query.rs` → `connection_lifecycle::budget` (keep `query::DbOperationBudget` type alias for one release if needed)
- add `LifecycleStage` enum + `log_stage(trace_id, connection_id, stage, elapsed_ms, timeout_ms, error)`
- instrument existing postgres checkout + main query path to use the helper (log format only)

**Tests:**

- unit: budget defaults still match PIP (query 0 → `query_timeout=None`; infra hard limits present)
- unit: stage log formatting does not panic on empty ids

**Acceptance:**

- `cargo test -p dbx-core --lib` budget/stage tests green
- no intentional behavior change; grep shows new module used from at least checkout + one query path

**Out of scope:** moving connect match; frontend changes.

---

### PR-A2 — Budgeted health + cleanup as lifecycle APIs

**Intent:** One health and one cleanup path with `DbOperationBudget`, used by Tauri/web.

**Files:**

- `connection_lifecycle/health.rs`
  - replace dual logic of `check_connection_health` vs parts of `refresh_connections` for PG/MySQL/openGauss first
  - health uses checkout helper + `SELECT 1` / MySQL ping under `checkout_timeout` / `recycle_timeout`
- `connection_lifecycle/cleanup.rs`
  - wrap `remove_pool_by_key` / `close_database_pool` with `cleanup_timeout` (already partially present as `close_pool_kind_with_timeout`—centralize)
- thin `AppState::check_connection_health` to call lifecycle
- Tauri/web remain one-liners

**Tests:**

- unit/integration: health returns error within timeout when pool get hangs (mock or fake pool if available; otherwise unit-test timeout wrapper)
- unit: cleanup is idempotent (double remove)

**Acceptance (PIP stage 1 remaining):**

- health check never hangs unbounded for PG/MySQL
- frontend existing 5s health timeout still works; backend also finishes within budget

**Out of scope:** force-reconnect UI.

---

### PR-A3 — Collapse connect / test_connection dispatch into core lifecycle

**Intent:** Pass the architecture deletion test for connect: adapters stop owning the driver match.

**Files:**

- move connect factory orchestration from `src-tauri/src/commands/connection.rs` into `connection_lifecycle/connect.rs` (or extract from `connection.rs` connect match into that module)
- `test_connection` and `connect_db` become:

  ```rust
  connection_lifecycle::test_connection(&state, config).await
  connection_lifecycle::connect(&state, config, client_session_id).await
  ```

- web routes use the same functions
- keep Mongo legacy fallback, agent timeout, tunnel probe **inside** lifecycle (not in Tauri)

**Tests:**

- existing connection tests still pass
- add/adjust unit tests that `test_connection` error messages for timeout contain classifiable connection keywords (`checkout timed out` / `timed out`) per PIP table

**Acceptance:**

- `src-tauri/src/commands/connection.rs` has **no** large `match config.db_type` for connect/test (grep gate)
- desktop + web share one connect implementation

**Risk:** large diff—split by native SQL families first (PG/MySQL/SQLite/Redis) then agents if needed in a follow-up commit within the same PR only if CI stays green; prefer two commits, one PR.

---

### PR-A4 — Checkout audit: kill bare `pool.get` on hot paths

**Intent:** PIP P0-3 completion for paths that can hang the product.

**Priority order:**

1. `query.rs` remaining bare get/get_conn  
2. `connection.rs` keepalive / detect paths  
3. `transfer.rs`, `database_export.rs`  
4. `db/questdb.rs`, `db/ob_oracle.rs`, `db/manticoresearch.rs` (adapter-local helpers OK if they call shared checkout)

**Gate:**

```text
# hot paths must not use bare checkout
rg 'pool\.get\(\)\.await|get_conn\(\)\.await' crates/dbx-core/src/query.rs crates/dbx-core/src/connection.rs
# allow only inside checkout_* helpers and tests
```

**Tests:**

- extend `is_connection_error` cases for any new timeout message formats
- optional: unit test cancel-during-checkout for MySQL path parity with Postgres

**Acceptance:**

- main query + connect + health + cancel paths only checkout via helpers that honor budget + cancel token

---

### PR-A5 — Frontend lifecycle adapter + force recovery

**Intent:** UI becomes adapter; recovery without restart (PIP stage 2 UI).

**Files:**

- `apps/desktop/src/lib/connection/lifecycleClient.ts` (or similar)
  - single place for: ensureConnected timeouts, health timeout, cancel timeout constants (from PIP table)
- thin `connectionStore.ensureConnected` to call client
- add `forceClearPoolsAndReconnect(connectionId)` → backend cleanup + connect
- wire sidebar / connection error banner action (minimal UI)
- diagnostics snippet: last health error, connected flag, db_type (no huge panel required)

**Tests (existing patterns):**

- `connectionStore.timeout.spec.ts` — keep green; add force-reconnect clears loading
- `queryStore` cancel timeout still clears `isCancelling`

**Acceptance:**

- PIP stage 2 UI: user can clear pool and reconnect without restart
- no permanent tree loading / isExecuting on mocked hung health/connect

---

### PR-A6 — Stage logging completeness + docs

**Intent:** PIP stage 4 observability lite.

**Files:**

- ensure stages: `ensureConnected`, `pool.checkout`, `ping`, `schema.set`, `query.execute`, `cancel`, `cleanup` emit consistent fields
- update PIP-0001 status notes or this plan’s “Done” table when complete
- short QA note under `docs/pips/plans/` or existing troubleshooting doc: how to read stage logs

**Acceptance:**

- from one hung query log line sequence, engineer can name the stuck stage

## Compatibility rules (copy from PIP)

1. `query_timeout_secs = 0` → SQL execution unbounded; infra phases always bounded.  
2. MySQL session-scoped pools: cleanup only on idle expiry, connection loss, tab close, or user force—not after every query.  
3. PostgreSQL `search_path` behavior unchanged; SET/RESET stay inside budgeted execute path.  
4. Cleanup never holds `connections` write lock while awaiting driver close I/O.

## Verification matrix

| Scenario | PR | Pass criteria |
| --- | --- | --- |
| PG/MySQL unreachable on query | A2–A4 | error within budget; UI not forever executing |
| Cancel `pg_sleep` / `SLEEP` | already mostly done; A4/A5 | UI exits cancelling in 2–5s |
| Health hung | A2/A5 | frontend + backend timeout; tree not spinning |
| DB back after outage | A3/A5 | force reconnect or auto recovery works without app restart |
| `query_timeout_secs = 0` long SQL | all | SQL not killed by infra budget |
| Session temp table (MySQL) | A4/A5 | normal multi-query session still works |

Commands (per PR as applicable):

```bash
cargo test -p dbx-core --lib
# if live DBs available:
# cargo test -p dbx-core --test live_* -- --ignored
pnpm exec vitest run apps/desktop/src/stores/__tests__/connectionStore.timeout.spec.ts
pnpm exec vitest run apps/desktop/src/stores/__tests__/queryStore*.spec.ts
```

## Mapping: architecture review ↔ this plan

| Review #1 claim | Phase A response |
| --- | --- |
| Test/connect/health/timeout/pool/cleanup cross seams | PR-A1–A3 single lifecycle module |
| Tauri/drivers as adapters | PR-A3 moves match out of Tauri |
| Deletion test | PR-A3 grep gate on command match |
| Leverage all driver adapters | Connect factory remains multi-driver; budgets apply first to PG/MySQL/openGauss |

| Explicitly deferred | Next phase |
| --- | --- |
| Hide `PoolKind` from query/schema/transfer | Phase B / review #2 |
| Data Grid session | review #3 |
| Database behavior single registry | review #5 |
| Production-safety single policy | review #6 |

## Suggested issue / PR titles

1. `refactor(connection): add connection_lifecycle facade and stage logs`  
2. `fix(connection): budgeted health and cleanup via lifecycle`  
3. `refactor(connection): move connect/test dispatch into dbx-core lifecycle`  
4. `fix(connection): require budgeted checkout on hot paths`  
5. `feat(connection): force reconnect lifecycle client on desktop`  
6. `docs(connection): stage log guide for idle timeout recovery`

## Definition of done (Phase A)

1. Architecture review #1 deletion test passes: connect/test/health/cleanup policy lives in `connection_lifecycle`, not in Tauri/web command matches.  
2. PIP-0001 acceptance criteria 1–4 remain true (no permanent executing/cancelling/loading; reconnect without restart for PG/openGauss/MySQL).  
3. Hot-path bare checkout eliminated (PR-A4 gate).  
4. Stage logs can identify stuck phase (PR-A6).  
5. No new public API breakage for MCP/CLI/web without adapters updated in the same PR.  
6. This document’s “Done” baseline table updated in a final docs commit.

## Immediate next step

Start **PR-A1** only: module skeleton + budget re-export + stage log helper + wire one existing checkout path. Keep the PR under ~300 lines of meaningful change so review stays on the seam, not on driver behavior.

---

## Implementation log

### PR-A1 — landed (2026-07-14)

**Done:**

- Added `crates/dbx-core/src/connection_lifecycle/` with:
  - `budget.rs` — `DbOperationBudget`, `resolve_query_timeout`, default cancel/cleanup constants
  - `stage.rs` — `LifecycleStage`, `StageOutcome`, `StageLog`, `format_stage_log`, `log_stage`
  - `mod.rs` — public facade
- Wired `pub mod connection_lifecycle` in `lib.rs`
- `query::DbOperationBudget` is a re-export of the lifecycle type (import path stable)
- Instrumented:
  - `db/postgres.rs` `checkout_postgres_client` → lifecycle stage logs
  - `query.rs` `exec_tx_pg_inner` schema.set path → lifecycle stage logs
- Unit tests for budget semantics and stage formatting live under the new module

**Not in A1 (by design):** connect/test dispatch move, health/cleanup APIs, bare-checkout audit, frontend force reconnect.
