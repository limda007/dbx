# How to read connection lifecycle stage logs

**Date:** 2026-07-15  
**Related:** [PIP-0001](../PIP-0001-database-connection-timeout-recovery.md), [Phase A plan](./2026-07-14-phase-a-connection-lifecycle.md) PR-A6

## Log line shape

All lifecycle stages emit through `connection_lifecycle::log_stage`:

```text
[db:<stage>:<outcome>] elapsed_ms=… timeout_ms=… trace_id=… connection_id=… pool_key=… db_type=… client_session_id=… detail=… error=…
```

| Field | Meaning |
| --- | --- |
| `stage` | Named phase (PIP vocabulary) |
| `outcome` | `start` / `accepted` / `done` / `error` / `cancelled` |
| `elapsed_ms` | Wall time spent in that stage so far |
| `timeout_ms` | Budget for the stage (when known) |
| `trace_id` | Usually the query `execution_id` |
| `connection_id` / `pool_key` | Which connection / pool |
| `db_type` | Product label (`postgres`, `opengauss`, `mysql`, …) — not the pool adapter |
| `detail` | Non-error notes (e.g. `cancel:accepted` explanation) — not a failure |
| `error` | Present only for real failures (`outcome=error` or failed ops) |

**Levels:** `error` → warn; `cancelled` → info; `start`/`done` → debug. Enable `RUST_LOG=dbx_core=debug` (or broader) to see the full sequence.

## Stage vocabulary

| Stage | When it appears |
| --- | --- |
| `ensureConnected` | Backend `connect` / register base pool |
| `pool.checkout` | Waiting for a free pool handle (PG + MySQL hot paths) |
| `pool.recycle` | Reserved for an explicit pool-recycle hook. It is not emitted for a generic `pool.get()` because that operation also includes wait/create work. |
| `ping` | Budgeted health probe (`SELECT 1` / MySQL ping) |
| `schema.set` | PostgreSQL `SET search_path` (and related) |
| `query.execute` | SQL execution wrapper around `do_execute` |
| `result.fetch` | Agent or external-driver cursor-page fetch (`result_session_id`); this is distinct from the initial query execution. |
| `cancel` | User cancel / kill path (`RunningQueries::cancel`, PG cancel packet, MySQL `KILL QUERY`) |
| `cleanup` | Pool close under cleanup budget |

## Diagnosing a hung query

1. Find the query’s `trace_id` (same as frontend `execution_id`).
2. Filter logs: `rg 'trace_id=<id>'` or `rg '\[db:'`.
3. Read the **last** stage line with that `trace_id` (or matching `connection_id` if connect hung before an execution id existed).

| Last stage / outcome | Stuck meaning | Typical next step |
| --- | --- | --- |
| `ensureConnected:start` only | Connect / probe never finished | Network, VPN, driver install; frontend also times out connect |
| `pool.checkout:start` or `:error` with checkout timed out | Pool saturated or dead TCP | Force reconnect / clear pool; check server max connections |
| `ping:error` | Health probe failed | Pool discarded; reconnect |
| `schema.set:start` only | `SET search_path` hung | Often stale PG session; cancel + reconnect |
| `query.execute:start` only | Driver executing SQL (or waiting on network) | Cancel; if cancel also hangs, look for `cancel:` lines |
| `cancel:start` only / `:error` | Cancel request stuck | PG cancel TLS path or MySQL kill checkout; cleanup budget should still bound pool close |
| `cleanup:error` | Pool close exceeded budget | Handle dropped; process should recover without restart |

### Example healthy PG query

```text
[db:query.execute:start] elapsed_ms=0 timeout_ms=30000 trace_id=exec-1 connection_id=c1 pool_key=c1:app db_type=postgres
[db:pool.checkout:done] elapsed_ms=2 timeout_ms=10000 trace_id=exec-1 …
[db:schema.set:done] elapsed_ms=1 …
[db:query.execute:done] elapsed_ms=45 …
```

### Example hung at checkout

```text
[db:query.execute:start] elapsed_ms=0 timeout_ms=30000 trace_id=exec-2 …
[db:pool.checkout:error] elapsed_ms=10012 timeout_ms=10000 … error=checkout timed out
[db:query.execute:error] elapsed_ms=10015 … error=…checkout timed out…
```

→ Stuck stage name: **`pool.checkout`**.

### Example user cancel during SQL

Client cancel is **accepted** immediately; server cancel (`KILL QUERY` / PG cancel packet) logs its own `start` → `done`/`error` when that future settles.

```text
[db:query.execute:start] …
[db:cancel:start] trace_id=exec-3 …          # RunningQueries::cancel
[db:cancel:accepted] trace_id=exec-3 …       # token fired; server kill may still be in flight
[db:cancel:start] trace_id=exec-3 …          # MySQL KILL / PG cancel packet
[db:cancel:done] trace_id=exec-3 …           # server cancel finished (or :error if stuck/failed)
[db:query.execute:cancelled] …
```

Do **not** treat the first `cancel:accepted` as “server cancel succeeded”.

## Frontend notes

UI timeouts (ensureConnected health 5s, cancel 10s, connect attempt budget) live in `apps/desktop/src/lib/connection/lifecycleClient.ts`. They complement backend stage logs: if the UI already cleared `isExecuting` but backend still logs `query.execute:start`, the backend task may still be winding down under cancel/cleanup budgets.

When a connection error popover is visible, **Copy diagnostics** fetches a point-in-time backend snapshot and combines it with local state. The copied text includes active-query count and pool keys, but intentionally excludes SQL, execution IDs, credentials, and connection configuration.
