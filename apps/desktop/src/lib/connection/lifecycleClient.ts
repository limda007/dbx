/**
 * Frontend connection-lifecycle adapter (PIP-0001 stage 2 / Phase A5).
 *
 * Owns UI-side timeout policy and pure helpers. Pinia stores remain the
 * state owners; they call these helpers instead of redefining the PIP table.
 */
import type { ConnectionConfig } from "@/types/database";
import { connectionAttemptTimeoutMs } from "@/lib/connection/connectionAttemptTimeout";

/** Skip backend health re-check when a successful check landed this recently. */
export const CONNECTION_HEALTH_CHECK_TTL_MS = 2_000;

/**
 * ensureConnected health-check fast path (PIP table: fixed 5s).
 * Used when the UI already believes the connection is connected.
 */
export const ENSURE_CONNECTED_HEALTH_TIMEOUT_MS = 5_000;

/** @deprecated Prefer ENSURE_CONNECTED_HEALTH_TIMEOUT_MS — same value, kept for call-site clarity. */
export const CONNECTION_HEALTH_CHECK_TIMEOUT_MS = ENSURE_CONNECTED_HEALTH_TIMEOUT_MS;

/** cancelQuery frontend guard (PIP table: fixed 10s). */
export const CANCEL_QUERY_TIMEOUT_MS = 10_000;

/** Floor for budgeted health checks that scale with connect_timeout_secs. */
export const CONNECTION_HEALTH_CHECK_MIN_TIMEOUT_MS = 5_000;

const DEFAULT_CONNECT_TIMEOUT_SECS = 10;

export { connectionAttemptTimeoutMs };

/**
 * Standalone checkConnectionHealth budget (PIP):
 * max(connect_timeout_secs * 2, 5s).
 */
export function connectionHealthCheckTimeoutMs(config?: Pick<ConnectionConfig, "connect_timeout_secs"> | null): number {
  const connectSecs = typeof config?.connect_timeout_secs === "number" && Number.isFinite(config.connect_timeout_secs) && config.connect_timeout_secs > 0 ? config.connect_timeout_secs : DEFAULT_CONNECT_TIMEOUT_SECS;
  return Math.max(CONNECTION_HEALTH_CHECK_MIN_TIMEOUT_MS, Math.ceil(connectSecs * 2 * 1000));
}

export function connectionHealthTimeoutMessage(timeoutMs: number = ENSURE_CONNECTED_HEALTH_TIMEOUT_MS): string {
  return `Connection health check timed out after ${Math.ceil(timeoutMs / 1000)}s.`;
}

export function cancelQueryTimeoutMessage(timeoutMs: number = CANCEL_QUERY_TIMEOUT_MS): string {
  return `Cancel request timed out after ${Math.ceil(timeoutMs / 1000)}s.`;
}

/** Race a promise against a hard frontend deadline; always clears the timer. */
export async function withLifecycleTimeout<T>(promise: Promise<T>, timeoutMs: number, message: string): Promise<T> {
  if (timeoutMs <= 0) return promise;
  let timer: ReturnType<typeof setTimeout> | undefined;
  try {
    return await Promise.race([
      promise,
      new Promise<never>((_, reject) => {
        timer = setTimeout(() => reject(new Error(message)), timeoutMs);
      }),
    ]);
  } finally {
    if (timer) clearTimeout(timer);
  }
}

export async function withEnsureConnectedHealthTimeout<T>(promise: Promise<T>): Promise<T> {
  return withLifecycleTimeout(promise, ENSURE_CONNECTED_HEALTH_TIMEOUT_MS, connectionHealthTimeoutMessage(ENSURE_CONNECTED_HEALTH_TIMEOUT_MS));
}

export async function withCancelQueryTimeout<T>(promise: Promise<T>): Promise<T> {
  return withLifecycleTimeout(promise, CANCEL_QUERY_TIMEOUT_MS, cancelQueryTimeoutMessage());
}

export interface ConnectionLifecycleDiagnostics {
  connectionId: string;
  dbType?: string;
  connected: boolean;
  connecting: boolean;
  lastError?: string;
}

export function connectionLifecycleDiagnostics(input: { connectionId: string; dbType?: string; connected: boolean; connecting?: boolean; lastError?: string }): ConnectionLifecycleDiagnostics {
  return {
    connectionId: input.connectionId,
    dbType: input.dbType,
    connected: input.connected,
    connecting: input.connecting === true,
    lastError: input.lastError,
  };
}
