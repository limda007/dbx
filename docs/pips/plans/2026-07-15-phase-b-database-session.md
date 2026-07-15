# Phase B: Hide `PoolKind` Behind DatabaseSession

**Status:** In progress — **PR-B1 done** (B2 next: schema hot paths)  
**Date:** 2026-07-15  
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

### PR-B2 — Schema hot paths via session

**Intent:** `list_databases` / `list_schemas` / `list_tables` (and primary tree loaders) call session helpers.

**Acceptance:**

- native schema provider no longer owns multi-arm `PoolKind` matches for those three
- external driver / plugin path still works

---

### PR-B3 — Transfer + export checkout via session

**Intent:** transfer/export obtain driver handles through session APIs (budgeted checkout already from A).

---

### PR-B4 — Shrink public `PoolKind`

**Intent:** `PoolKind` `pub(crate)` or re-export only for tests; Tauri/web never import it.

**Gate:**

```text
rg 'PoolKind' apps/ src-tauri/ crates/dbx-web/   # should be empty or adapters only
```

---

### PR-B5 — Optional capability traits (PG/MySQL first)

**Intent:** Extract `SqlExecute` / `SchemaBrowse` traits for first-class drivers; remaining variants stay enum.

Only if B1–B4 prove the seam; do not invent traits with one adapter.

## Compatibility rules

1. Keep Phase A budgets and stage log vocabulary.
2. `query_timeout_secs = 0` still means unbounded SQL; infra still bounded.
3. MySQL session-scoped pools: no cleanup-after-every-query.
4. Plugin / external driver invoke paths stay timeout-wrapped.

## Verification

```bash
# Safe (WSL):
CARGO_BUILD_JOBS=1 cargo test -p dbx-core --lib \
  --no-default-features --features mq-admin -j 1 \
  database_session connection_lifecycle -- --test-threads=1

# Full when memory allows:
CARGO_BUILD_JOBS=1 cargo test -p dbx-core --lib -j 1 -- --test-threads=1
```

## Definition of done (Phase B)

1. Product hot paths (query execute, schema tree, transfer) do not `match PoolKind` outside `database_session` / `connection`.
2. Deletion test passes for the session module.
3. Phase A lifecycle + tests remain green.
4. This plan’s status updated; PIP-0001 notes Phase B progress.

## Immediate next step

**PR-B2:** move schema hot paths (`list_databases` / `list_schemas` / `list_tables`) into `database_session`.
