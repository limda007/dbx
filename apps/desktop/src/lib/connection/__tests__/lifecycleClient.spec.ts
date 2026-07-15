import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
  CANCEL_QUERY_TIMEOUT_MS,
  CONNECTION_HEALTH_CHECK_TTL_MS,
  ENSURE_CONNECTED_HEALTH_TIMEOUT_MS,
  cancelQueryTimeoutMessage,
  connectionAttemptTimeoutMs,
  connectionHealthCheckTimeoutMs,
  connectionHealthTimeoutMessage,
  connectionLifecycleDiagnostics,
  withCancelQueryTimeout,
  withEnsureConnectedHealthTimeout,
  withLifecycleTimeout,
} from "@/lib/connection/lifecycleClient";

describe("lifecycleClient timeout policy", () => {
  beforeEach(() => {
    vi.useFakeTimers();
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  it("exposes PIP table constants", () => {
    expect(CONNECTION_HEALTH_CHECK_TTL_MS).toBe(2_000);
    expect(ENSURE_CONNECTED_HEALTH_TIMEOUT_MS).toBe(5_000);
    expect(CANCEL_QUERY_TIMEOUT_MS).toBe(10_000);
  });

  it("scales standalone health timeout as max(connect_timeout * 2, 5s)", () => {
    expect(connectionHealthCheckTimeoutMs({ connect_timeout_secs: 10 })).toBe(20_000);
    expect(connectionHealthCheckTimeoutMs({ connect_timeout_secs: 1 })).toBe(5_000);
    expect(connectionHealthCheckTimeoutMs(undefined)).toBe(20_000);
  });

  it("re-exports connect attempt timeout from connectionAttemptTimeout", () => {
    expect(connectionAttemptTimeoutMs({ connect_timeout_secs: 10, transport_layers: [] })).toBeGreaterThan(10_000);
  });

  it("times out hung ensureConnected health checks", async () => {
    const hung = new Promise<void>(() => undefined);
    const result = withEnsureConnectedHealthTimeout(hung).catch((error) => error);
    await vi.advanceTimersByTimeAsync(ENSURE_CONNECTED_HEALTH_TIMEOUT_MS + 1);
    const error = await result;
    expect(error).toBeInstanceOf(Error);
    expect(error.message).toBe(connectionHealthTimeoutMessage(ENSURE_CONNECTED_HEALTH_TIMEOUT_MS));
  });

  it("times out hung cancel requests", async () => {
    const hung = new Promise<boolean>(() => undefined);
    const result = withCancelQueryTimeout(hung).catch((error) => error);
    await vi.advanceTimersByTimeAsync(CANCEL_QUERY_TIMEOUT_MS + 1);
    const error = await result;
    expect(error).toBeInstanceOf(Error);
    expect(error.message).toBe(cancelQueryTimeoutMessage());
  });

  it("resolves before timeout when the promise settles", async () => {
    const value = await withLifecycleTimeout(Promise.resolve("ok"), 5_000, "timed out");
    expect(value).toBe("ok");
  });

  it("builds a compact diagnostics snippet", () => {
    expect(
      connectionLifecycleDiagnostics({
        connectionId: "pg-1",
        dbType: "postgres",
        connected: false,
        connecting: true,
        lastError: "connection refused",
      }),
    ).toEqual({
      connectionId: "pg-1",
      dbType: "postgres",
      connected: false,
      connecting: true,
      lastError: "connection refused",
    });
  });
});
