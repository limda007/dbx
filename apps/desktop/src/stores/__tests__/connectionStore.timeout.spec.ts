import { createPinia, setActivePinia } from "pinia";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { ConnectionConfig, TreeNode } from "@/types/database";

function installLocalStorage() {
  const data = new Map<string, string>();
  vi.stubGlobal("localStorage", {
    getItem: vi.fn((key: string) => data.get(key) ?? null),
    setItem: vi.fn((key: string, value: string) => data.set(key, value)),
    removeItem: vi.fn((key: string) => data.delete(key)),
  });
}

function postgresConnection(overrides: Partial<ConnectionConfig> = {}): ConnectionConfig {
  return {
    id: "pg-1",
    name: "Postgres",
    db_type: "postgres",
    host: "127.0.0.1",
    port: 5432,
    username: "postgres",
    password: "",
    database: "app",
    read_only: false,
    ...overrides,
  } as ConnectionConfig;
}

describe("connectionStore timeout recovery", () => {
  beforeEach(() => {
    vi.useFakeTimers();
    vi.resetModules();
    vi.unstubAllGlobals();
    installLocalStorage();
    setActivePinia(createPinia());
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  it("times out connected health checks and falls back to reconnect", async () => {
    const checkConnectionHealth = vi.fn(() => new Promise(() => undefined));
    const connectDb = vi.fn().mockResolvedValue(undefined);

    vi.doMock("@/lib/backend/tauriRuntime", () => ({ isTauriRuntime: () => false }));
    vi.doMock("@/lib/backend/api", () => ({
      checkConnectionHealth,
      connectDb,
      deleteSchemaCachePrefix: vi.fn().mockResolvedValue(undefined),
      saveConnections: vi.fn().mockResolvedValue(undefined),
      saveSidebarLayout: vi.fn().mockResolvedValue(undefined),
    }));

    const { useConnectionStore } = await import("@/stores/connectionStore");
    const store = useConnectionStore();
    const connection = postgresConnection({ connect_timeout_secs: 10 });
    store.connections = [connection];
    store.connectedIds.add(connection.id);

    const ensure = store.ensureConnected(connection.id);
    await vi.advanceTimersByTimeAsync(5001);
    await ensure;

    expect(checkConnectionHealth).toHaveBeenCalledWith(connection.id);
    expect(connectDb).toHaveBeenCalledWith(connection, expect.any(Number));
    expect(store.connectedIds.has(connection.id)).toBe(true);
  }, 10_000);

  it("normalizes missing keepalive interval to 30 seconds", async () => {
    vi.doMock("@/lib/backend/tauriRuntime", () => ({ isTauriRuntime: () => false }));
    vi.doMock("@/lib/backend/api", () => ({
      deleteSchemaCachePrefix: vi.fn().mockResolvedValue(undefined),
      saveConnections: vi.fn().mockResolvedValue(undefined),
      saveSidebarLayout: vi.fn().mockResolvedValue(undefined),
    }));

    const { useConnectionStore } = await import("@/stores/connectionStore");
    const store = useConnectionStore();
    await store.addConnection(postgresConnection({ keepalive_interval_secs: undefined }));

    expect(store.connections[0]?.keepalive_interval_secs).toBe(30);
  });

  it("bounds every table-detail metadata request and clears its loading state", async () => {
    const getColumns = vi.fn(() => new Promise(() => undefined));
    const listIndexes = vi.fn(() => new Promise(() => undefined));
    const listForeignKeys = vi.fn(() => new Promise(() => undefined));
    const listTriggers = vi.fn(() => new Promise(() => undefined));

    vi.doMock("@/lib/backend/tauriRuntime", () => ({ isTauriRuntime: () => false }));
    vi.doMock("@/lib/backend/api", () => ({
      deleteSchemaCachePrefix: vi.fn().mockResolvedValue(undefined),
      getColumns,
      listForeignKeys,
      listInstalledAgents: vi.fn().mockResolvedValue([]),
      listIndexes,
      listTriggers,
      saveConnections: vi.fn().mockResolvedValue(undefined),
      saveSidebarLayout: vi.fn().mockResolvedValue(undefined),
    }));

    const { useConnectionStore } = await import("@/stores/connectionStore");
    const store = useConnectionStore();
    const connection = postgresConnection({ query_timeout_secs: 1 });
    store.connections = [connection];

    const scenarios: Array<{
      label: string;
      nodeId: string;
      load: () => Promise<void>;
    }> = [
      {
        label: "columns",
        nodeId: `${connection.id}:app:public:users:__columns`,
        load: () => store.loadColumns(connection.id, "app", "users", "public", `${connection.id}:app:public:users:__columns`),
      },
      {
        label: "indexes",
        nodeId: `${connection.id}:app:public:users:__indexes`,
        load: () => store.loadIndexes(connection.id, "app", "users", "public", `${connection.id}:app:public:users:__indexes`),
      },
      {
        label: "foreign keys",
        nodeId: `${connection.id}:app:public:users:__fkeys`,
        load: () => store.loadForeignKeys(connection.id, "app", "users", "public", `${connection.id}:app:public:users:__fkeys`),
      },
      {
        label: "triggers",
        nodeId: `${connection.id}:app:public:users:__triggers`,
        load: () => store.loadTriggers(connection.id, "app", "users", "public", `${connection.id}:app:public:users:__triggers`),
      },
    ];

    for (const scenario of scenarios) {
      const node: TreeNode = {
        id: scenario.nodeId,
        label: scenario.label,
        type: "group-columns",
        connectionId: connection.id,
        database: "app",
        schema: "public",
        tableName: "users",
        isLoading: false,
        children: [],
      };
      store.treeNodes = [node];
      store.connectedIds.add(connection.id);

      const pending = scenario.load().catch((error) => error);
      await vi.advanceTimersByTimeAsync(15_001);
      const error = await pending;

      expect(error).toBeInstanceOf(Error);
      expect((error as Error).message).toContain(`loading ${scenario.label} after 15s`);
      expect(node.isLoading).toBe(false);
      expect(store.connectedIds.has(connection.id)).toBe(false);
      expect(store.connectionErrors[connection.id]).toContain(`loading ${scenario.label}`);
    }

    expect(getColumns).toHaveBeenCalledTimes(1);
    expect(listIndexes).toHaveBeenCalledTimes(1);
    expect(listForeignKeys).toHaveBeenCalledTimes(1);
    expect(listTriggers).toHaveBeenCalledTimes(1);
  }, 20_000);

  it("bounds schema and shared object-group metadata cache misses", async () => {
    const checkConnectionHealth = vi.fn().mockResolvedValue(undefined);
    const listSchemaInfos = vi.fn(() => new Promise(() => undefined));
    const listTables = vi.fn(() => new Promise(() => undefined));

    vi.doMock("@/lib/backend/tauriRuntime", () => ({ isTauriRuntime: () => false }));
    vi.doMock("@/lib/backend/api", () => ({
      checkConnectionHealth,
      deleteSchemaCachePrefix: vi.fn().mockResolvedValue(undefined),
      listInstalledAgents: vi.fn().mockResolvedValue([]),
      listSchemaInfos,
      listTables,
      saveConnections: vi.fn().mockResolvedValue(undefined),
      saveSidebarLayout: vi.fn().mockResolvedValue(undefined),
    }));

    const { useConnectionStore } = await import("@/stores/connectionStore");
    const store = useConnectionStore();
    const connection = postgresConnection({ query_timeout_secs: 1 });
    store.connections = [connection];

    const databaseNode: TreeNode = {
      id: `${connection.id}:app`,
      label: "app",
      type: "database",
      connectionId: connection.id,
      database: "app",
      isLoading: false,
      children: [],
    };
    store.treeNodes = [databaseNode];
    store.connectedIds.add(connection.id);
    const schemaLoad = store.loadSchemas(connection.id, "app", { force: true }).catch((error) => error);
    await vi.advanceTimersByTimeAsync(15_001);
    const schemaError = await schemaLoad;

    expect(schemaError).toBeInstanceOf(Error);
    expect((schemaError as Error).message).toContain("loading schemas after 15s");
    expect(databaseNode.isLoading).toBe(false);

    const tableGroup: TreeNode = {
      id: `${connection.id}:app:public:__tables`,
      label: "tree.tables",
      type: "group-tables",
      connectionId: connection.id,
      database: "app",
      schema: "public",
      isLoading: false,
      children: [],
    };
    store.treeNodes = [tableGroup];
    store.connectedIds.add(connection.id);
    const tableLoad = store.loadObjectGroupChildren(tableGroup, { force: true }).catch((error) => error);
    await vi.advanceTimersByTimeAsync(15_001);
    const tableError = await tableLoad;

    expect(tableError).toBeInstanceOf(Error);
    expect((tableError as Error).message).toContain("loading tables after 15s");
    expect(tableGroup.isLoading).toBe(false);
    expect(listSchemaInfos).toHaveBeenCalledWith(connection.id, "app");
    expect(listTables).toHaveBeenCalledWith(connection.id, "app", "public", undefined, 1001, 0, ["TABLE"]);
  }, 20_000);

  it("bounds SQL Server database and linked-server tree metadata", async () => {
    const checkConnectionHealth = vi.fn().mockResolvedValue(undefined);
    const listSchemas = vi.fn(() => new Promise(() => undefined));
    const listSqlServerLinkedServers = vi.fn(() => new Promise(() => undefined));
    const listSqlServerLinkedServerCatalogs = vi.fn(() => new Promise(() => undefined));
    const listSqlServerLinkedServerSchemas = vi.fn(() => new Promise(() => undefined));

    vi.doMock("@/lib/backend/tauriRuntime", () => ({ isTauriRuntime: () => false }));
    vi.doMock("@/lib/backend/api", () => ({
      checkConnectionHealth,
      deleteSchemaCachePrefix: vi.fn().mockResolvedValue(undefined),
      listInstalledAgents: vi.fn().mockResolvedValue([]),
      listSchemas,
      listSqlServerLinkedServerCatalogs,
      listSqlServerLinkedServerSchemas,
      listSqlServerLinkedServers,
      saveConnections: vi.fn().mockResolvedValue(undefined),
      saveSidebarLayout: vi.fn().mockResolvedValue(undefined),
    }));

    const { useConnectionStore } = await import("@/stores/connectionStore");
    const store = useConnectionStore();
    const connection = postgresConnection({ db_type: "sqlserver", database: "app", query_timeout_secs: 1 });
    store.connections = [connection];

    const scenarios: Array<{ label: string; node: TreeNode; load: () => Promise<void> }> = [
      {
        label: "schemas",
        node: { id: `${connection.id}:app`, label: "app", type: "database", connectionId: connection.id, database: "app", isLoading: false, children: [] },
        load: () => store.loadSqlServerDatabaseObjects(connection.id, "app", { force: true }),
      },
      {
        label: "linked servers",
        node: { id: `${connection.id}:__linked_servers`, label: "tree.linkedServers", type: "linked-server-root", connectionId: connection.id, database: "app", isLoading: false, children: [] },
        load: () => store.loadSqlServerLinkedServers(connection.id, { force: true }),
      },
      {
        label: "linked server catalogs",
        node: { id: `${connection.id}:__linked_servers:server-1`, label: "server-1", type: "linked-server", connectionId: connection.id, database: "app", linkedServer: "server-1", isLoading: false, children: [] },
        load: () => store.loadSqlServerLinkedServerCatalogs(store.treeNodes[0]!, { force: true }),
      },
      {
        label: "linked server schemas",
        node: { id: `${connection.id}:__linked_servers:server-1:catalog-1`, label: "catalog-1", type: "linked-server-catalog", connectionId: connection.id, database: "app", linkedServer: "server-1", linkedCatalog: "catalog-1", isLoading: false, children: [] },
        load: () => store.loadSqlServerLinkedServerSchemas(store.treeNodes[0]!, { force: true }),
      },
    ];

    for (const scenario of scenarios) {
      store.treeNodes = [scenario.node];
      store.connectedIds.add(connection.id);

      const pending = scenario.load().catch((error) => error);
      await vi.advanceTimersByTimeAsync(15_001);
      const error = await pending;

      expect(error).toBeInstanceOf(Error);
      expect((error as Error).message).toContain(`loading ${scenario.label} after 15s`);
      expect(scenario.node.isLoading).toBe(false);
      expect(store.connectedIds.has(connection.id)).toBe(false);
    }

    expect(listSchemas).toHaveBeenCalledWith(connection.id, "app");
    expect(listSqlServerLinkedServers).toHaveBeenCalledWith(connection.id);
    expect(listSqlServerLinkedServerCatalogs).toHaveBeenCalledWith(connection.id, "server-1");
    expect(listSqlServerLinkedServerSchemas).toHaveBeenCalledWith(connection.id, "server-1", "catalog-1");
  }, 20_000);

  it("clears connection node loading when health check timeout forces reconnect failure", async () => {
    const checkConnectionHealth = vi.fn(() => new Promise(() => undefined));
    const connectDb = vi.fn().mockRejectedValue(new Error("reconnect failed"));

    vi.doMock("@/lib/backend/tauriRuntime", () => ({ isTauriRuntime: () => false }));
    vi.doMock("@/lib/backend/api", () => ({
      checkConnectionHealth,
      connectDb,
      deleteSchemaCachePrefix: vi.fn().mockResolvedValue(undefined),
      saveConnections: vi.fn().mockResolvedValue(undefined),
      saveSidebarLayout: vi.fn().mockResolvedValue(undefined),
    }));

    const { useConnectionStore } = await import("@/stores/connectionStore");
    const store = useConnectionStore();
    const connection = postgresConnection({ connect_timeout_secs: 10 });
    const node: TreeNode = {
      id: connection.id,
      label: connection.name,
      type: "connection",
      connectionId: connection.id,
      isLoading: true,
      children: [],
    };
    store.connections = [connection];
    store.connectedIds.add(connection.id);
    store.treeNodes = [node];

    const ensure = store.ensureConnected(connection.id).catch((error) => error);
    await vi.advanceTimersByTimeAsync(5001);
    const error = await ensure;

    expect(error).toBeInstanceOf(Error);
    expect(node.isLoading).toBe(false);
  }, 10_000);

  it("cancels an in-flight connection without leaving connected or loading state", async () => {
    const connectDb = vi.fn(() => new Promise(() => undefined));
    const disconnectDb = vi.fn().mockResolvedValue(undefined);

    vi.doMock("@/lib/backend/tauriRuntime", () => ({ isTauriRuntime: () => false }));
    vi.doMock("@/lib/backend/api", () => ({
      connectDb,
      deleteSchemaCachePrefix: vi.fn().mockResolvedValue(undefined),
      disconnectDb,
      listInstalledAgents: vi.fn().mockResolvedValue([]),
      saveConnections: vi.fn().mockResolvedValue(undefined),
      saveSidebarLayout: vi.fn().mockResolvedValue(undefined),
    }));

    const { CONNECTION_ATTEMPT_CANCELLED_MESSAGE, useConnectionStore } = await import("@/stores/connectionStore");
    const store = useConnectionStore();
    const connection = postgresConnection({ connect_timeout_secs: 1 });
    const node: TreeNode = {
      id: connection.id,
      label: connection.name,
      type: "connection",
      connectionId: connection.id,
      isLoading: false,
      children: [],
    };
    store.connections = [connection];
    store.treeNodes = [node];

    const ensure = store.ensureConnected(connection.id).catch((error) => error);
    await vi.advanceTimersByTimeAsync(1);

    expect(store.connectingIds.has(connection.id)).toBe(true);
    expect(node.isLoading).toBe(true);

    await expect(store.cancelConnecting(connection.id)).resolves.toBe(true);
    expect(disconnectDb).toHaveBeenCalledWith(connection.id, expect.any(Number));
    expect(store.connectingIds.has(connection.id)).toBe(false);
    expect(store.connectedIds.has(connection.id)).toBe(false);
    expect(store.connectionErrors[connection.id]).toBeUndefined();
    expect(node.isLoading).toBe(false);

    await vi.advanceTimersByTimeAsync(3001);
    const error = await ensure;

    expect(error).toBeInstanceOf(Error);
    expect(error.message).toContain(CONNECTION_ATTEMPT_CANCELLED_MESSAGE);
    expect(store.connectingIds.has(connection.id)).toBe(false);
    expect(store.connectedIds.has(connection.id)).toBe(false);
    expect(store.connectionErrors[connection.id]).toBeUndefined();
    expect(node.isLoading).toBe(false);
  }, 10_000);

  it("allows reconnecting the same connection while a scoped cancel is pending", async () => {
    let resolveDisconnect!: () => void;
    const pendingConnect = new Promise<string>(() => undefined);
    let connectCallCount = 0;
    const connectDb = vi.fn(() => {
      connectCallCount += 1;
      return connectCallCount === 1 ? pendingConnect : Promise.resolve("pg-1");
    });
    const disconnectDb = vi.fn(
      () =>
        new Promise<void>((resolve) => {
          resolveDisconnect = resolve;
        }),
    );

    vi.doMock("@/lib/backend/tauriRuntime", () => ({ isTauriRuntime: () => false }));
    vi.doMock("@/lib/backend/api", () => ({
      connectDb,
      deleteSchemaCachePrefix: vi.fn().mockResolvedValue(undefined),
      disconnectDb,
      listInstalledAgents: vi.fn().mockResolvedValue([]),
      saveConnections: vi.fn().mockResolvedValue(undefined),
      saveSidebarLayout: vi.fn().mockResolvedValue(undefined),
    }));

    const { useConnectionStore } = await import("@/stores/connectionStore");
    const store = useConnectionStore();
    const connection = postgresConnection({ connect_timeout_secs: 1 });
    store.connections = [connection];
    store.treeNodes = [
      {
        id: connection.id,
        label: connection.name,
        type: "connection",
        connectionId: connection.id,
        isLoading: false,
        children: [],
      },
    ];

    const firstEnsure = store.ensureConnected(connection.id).catch((error) => error);
    await vi.advanceTimersByTimeAsync(1);
    expect(connectDb).toHaveBeenCalledTimes(1);
    const firstAttempt = connectDb.mock.calls[0]?.[1];

    const cancel = store.cancelConnecting(connection.id);
    await vi.advanceTimersByTimeAsync(1);
    expect(disconnectDb).toHaveBeenCalledWith(connection.id, firstAttempt);

    const reconnect = store.ensureConnected(connection.id);
    await vi.advanceTimersByTimeAsync(1);
    expect(connectDb).toHaveBeenCalledTimes(2);
    expect(connectDb.mock.calls[1]?.[1]).not.toBe(firstAttempt);

    resolveDisconnect();
    await cancel;
    await reconnect;

    expect(connectDb).toHaveBeenCalledTimes(2);
    expect(store.connectedIds.has(connection.id)).toBe(true);
    expect(store.connectionErrors[connection.id]).toBeUndefined();

    await vi.advanceTimersByTimeAsync(3001);
    await firstEnsure;
  }, 10_000);

  it("starts a fresh root metadata load after canceling a pending connection", async () => {
    let connectCallCount = 0;
    const connectDb = vi.fn(() => {
      connectCallCount += 1;
      return connectCallCount === 1 ? new Promise<string>(() => undefined) : Promise.resolve("pg-1");
    });
    const disconnectDb = vi.fn().mockResolvedValue(undefined);
    const listDatabases = vi.fn().mockResolvedValue([{ name: "app" }]);

    vi.doMock("@/lib/backend/tauriRuntime", () => ({ isTauriRuntime: () => false }));
    vi.doMock("@/lib/backend/api", () => ({
      connectDb,
      deleteSchemaCachePrefix: vi.fn().mockResolvedValue(undefined),
      disconnectDb,
      listDatabases,
      loadSchemaCache: vi.fn().mockResolvedValue(null),
      saveConnections: vi.fn().mockResolvedValue(undefined),
      saveSchemaCache: vi.fn().mockResolvedValue(undefined),
      saveSidebarLayout: vi.fn().mockResolvedValue(undefined),
    }));

    const { useConnectionStore } = await import("@/stores/connectionStore");
    const store = useConnectionStore();
    const connection = postgresConnection({ connect_timeout_secs: 10 });
    store.connections = [connection];
    store.treeNodes = [
      {
        id: connection.id,
        label: connection.name,
        type: "connection",
        connectionId: connection.id,
        isLoading: false,
        children: [],
      },
    ];

    void store.loadDatabases(connection.id).catch(() => undefined);
    await vi.advanceTimersByTimeAsync(1);
    expect(connectDb).toHaveBeenCalledTimes(1);
    const firstAttempt = connectDb.mock.calls[0]?.[1];

    await expect(store.cancelConnecting(connection.id)).resolves.toBe(true);
    expect(disconnectDb).toHaveBeenCalledWith(connection.id, firstAttempt);
    expect(store.treeNodes[0]?.isLoading).toBe(false);

    await store.loadDatabases(connection.id);

    expect(connectDb).toHaveBeenCalledTimes(2);
    expect(connectDb.mock.calls[1]?.[1]).not.toBe(firstAttempt);
    expect(listDatabases).toHaveBeenCalledTimes(1);
    expect(store.connectedIds.has(connection.id)).toBe(true);
    expect(store.treeNodes[0]?.isExpanded).toBe(true);
  }, 10_000);

  it("allows reconnecting the same connection while a scoped disconnect is pending", async () => {
    let resolveDisconnect!: () => void;
    const connectDb = vi.fn().mockResolvedValue("pg-1");
    const disconnectDb = vi.fn(
      () =>
        new Promise<void>((resolve) => {
          resolveDisconnect = resolve;
        }),
    );

    vi.doMock("@/lib/backend/tauriRuntime", () => ({ isTauriRuntime: () => false }));
    vi.doMock("@/lib/backend/api", () => ({
      connectDb,
      deleteSchemaCachePrefix: vi.fn().mockResolvedValue(undefined),
      disconnectDb,
      saveConnections: vi.fn().mockResolvedValue(undefined),
      saveSidebarLayout: vi.fn().mockResolvedValue(undefined),
    }));

    const { useConnectionStore } = await import("@/stores/connectionStore");
    const store = useConnectionStore();
    const connection = postgresConnection({ connect_timeout_secs: 10 });
    store.connections = [connection];
    store.treeNodes = [
      {
        id: connection.id,
        label: connection.name,
        type: "connection",
        connectionId: connection.id,
        isLoading: true,
        isExpanded: true,
        children: [],
      },
    ];

    await store.connect(connection);
    expect(store.connectedIds.has(connection.id)).toBe(true);
    const firstAttempt = connectDb.mock.calls[0]?.[1];

    const disconnect = store.disconnect(connection.id);
    expect(disconnectDb).toHaveBeenCalledWith(connection.id, firstAttempt);
    expect(store.connectedIds.has(connection.id)).toBe(false);
    expect(store.treeNodes[0]?.isLoading).toBe(false);
    expect(store.treeNodes[0]?.isExpanded).toBe(false);

    await store.connect(connection);
    expect(connectDb).toHaveBeenCalledTimes(2);
    expect(connectDb.mock.calls[1]?.[1]).not.toBe(firstAttempt);
    expect(store.connectedIds.has(connection.id)).toBe(true);

    resolveDisconnect();
    await disconnect;

    expect(store.connectedIds.has(connection.id)).toBe(true);
    expect(store.connectionErrors[connection.id]).toBeUndefined();
  }, 10_000);

  it("keeps a newer reconnect error when an older scoped disconnect finishes later", async () => {
    let resolveDisconnect!: () => void;
    let connectCallCount = 0;
    const connectDb = vi.fn(() => {
      connectCallCount += 1;
      return connectCallCount === 1 ? Promise.resolve("pg-1") : Promise.reject(new Error("reconnect failed"));
    });
    const disconnectDb = vi.fn(
      () =>
        new Promise<void>((resolve) => {
          resolveDisconnect = resolve;
        }),
    );

    vi.doMock("@/lib/backend/tauriRuntime", () => ({ isTauriRuntime: () => false }));
    vi.doMock("@/lib/backend/api", () => ({
      connectDb,
      deleteSchemaCachePrefix: vi.fn().mockResolvedValue(undefined),
      disconnectDb,
      listInstalledAgents: vi.fn().mockResolvedValue([]),
      saveConnections: vi.fn().mockResolvedValue(undefined),
      saveSidebarLayout: vi.fn().mockResolvedValue(undefined),
    }));

    const { useConnectionStore } = await import("@/stores/connectionStore");
    const store = useConnectionStore();
    const connection = postgresConnection({ connect_timeout_secs: 10 });
    store.connections = [connection];

    await store.connect(connection);
    const firstAttempt = connectDb.mock.calls[0]?.[1];

    const disconnect = store.disconnect(connection.id);
    expect(disconnectDb).toHaveBeenCalledWith(connection.id, firstAttempt);

    await expect(store.connect(connection)).rejects.toThrow("reconnect failed");
    expect(store.connectionErrors[connection.id]).toBe("reconnect failed");

    resolveDisconnect();
    await disconnect;

    expect(store.connectedIds.has(connection.id)).toBe(false);
    expect(store.connectionErrors[connection.id]).toBe("reconnect failed");
  }, 10_000);

  it("forceClearPoolsAndReconnect disconnects then reconnects and clears loading", async () => {
    const disconnectDb = vi.fn().mockResolvedValue(undefined);
    const connectDb = vi.fn().mockResolvedValue("pg-1");
    const checkConnectionHealth = vi.fn().mockResolvedValue(undefined);

    vi.doMock("@/lib/backend/tauriRuntime", () => ({ isTauriRuntime: () => false }));
    vi.doMock("@/lib/backend/api", () => ({
      checkConnectionHealth,
      connectDb,
      deleteSchemaCachePrefix: vi.fn().mockResolvedValue(undefined),
      disconnectDb,
      listInstalledAgents: vi.fn().mockResolvedValue([]),
      saveConnections: vi.fn().mockResolvedValue(undefined),
      saveSidebarLayout: vi.fn().mockResolvedValue(undefined),
    }));

    const { useConnectionStore } = await import("@/stores/connectionStore");
    const store = useConnectionStore();
    const connection = postgresConnection({ connect_timeout_secs: 10 });
    const node: TreeNode = {
      id: connection.id,
      label: connection.name,
      type: "connection",
      connectionId: connection.id,
      isLoading: true,
      children: [],
    };
    store.connections = [connection];
    store.connectedIds.add(connection.id);
    store.connectionErrors[connection.id] = "stale pool";
    store.treeNodes = [node];

    await store.forceClearPoolsAndReconnect(connection.id);

    // No prior successful attempt → clientAttempt is undefined (full pool clear).
    expect(disconnectDb).toHaveBeenCalledWith(connection.id, undefined);
    expect(connectDb).toHaveBeenCalledWith(connection, expect.any(Number));
    expect(store.connectedIds.has(connection.id)).toBe(true);
    expect(store.connectionErrors[connection.id]).toBeUndefined();
    expect(node.isLoading).toBe(false);
    expect(store.getConnectionLifecycleDiagnostics(connection.id)).toMatchObject({
      connectionId: connection.id,
      dbType: "postgres",
      connected: true,
      connecting: false,
    });
  }, 10_000);

  it("merges runtime diagnostics with the latest failed health check", async () => {
    const checkConnectionHealth = vi.fn().mockRejectedValue(new Error("pool is unhealthy"));
    const connectDb = vi.fn().mockResolvedValue("pg-1");
    const connectionRuntimeDiagnostics = vi.fn().mockResolvedValue({
      connectionId: "pg-1",
      activeQueryCount: 2,
      poolKeys: ["pg-1:app", "pg-1:app:session:tab-1"],
    });

    vi.doMock("@/lib/backend/tauriRuntime", () => ({ isTauriRuntime: () => false }));
    vi.doMock("@/lib/backend/api", () => ({
      checkConnectionHealth,
      connectDb,
      connectionRuntimeDiagnostics,
      deleteSchemaCachePrefix: vi.fn().mockResolvedValue(undefined),
      saveConnections: vi.fn().mockResolvedValue(undefined),
      saveSidebarLayout: vi.fn().mockResolvedValue(undefined),
    }));

    const { useConnectionStore } = await import("@/stores/connectionStore");
    const store = useConnectionStore();
    const connection = postgresConnection({ connect_timeout_secs: 10 });
    store.connections = [connection];
    store.connectedIds.add(connection.id);

    await store.ensureConnected(connection.id);
    const diagnostics = await store.loadConnectionLifecycleDiagnostics(connection.id);

    expect(checkConnectionHealth).toHaveBeenCalledWith(connection.id);
    expect(connectDb).toHaveBeenCalledWith(connection, expect.any(Number));
    expect(connectionRuntimeDiagnostics).toHaveBeenCalledWith(connection.id);
    expect(diagnostics).toMatchObject({
      connectionId: connection.id,
      connected: true,
      activeQueryCount: 2,
      poolKeys: ["pg-1:app", "pg-1:app:session:tab-1"],
      lastHealth: { healthy: false, error: "pool is unhealthy" },
    });
  });

  it("drops a stale runtime snapshot after force-clear or a failed refresh", async () => {
    const checkConnectionHealth = vi.fn().mockResolvedValue(undefined);
    const connectDb = vi.fn().mockResolvedValue("pg-1");
    const disconnectDb = vi.fn().mockResolvedValue(undefined);
    const connectionRuntimeDiagnostics = vi
      .fn()
      .mockResolvedValueOnce({ connectionId: "pg-1", activeQueryCount: 1, poolKeys: ["pg-1:app"] })
      .mockRejectedValueOnce(new Error("diagnostics endpoint unavailable"));

    vi.doMock("@/lib/backend/tauriRuntime", () => ({ isTauriRuntime: () => false }));
    vi.doMock("@/lib/backend/api", () => ({
      checkConnectionHealth,
      connectDb,
      connectionRuntimeDiagnostics,
      deleteSchemaCachePrefix: vi.fn().mockResolvedValue(undefined),
      disconnectDb,
      listInstalledAgents: vi.fn().mockResolvedValue([]),
      saveConnections: vi.fn().mockResolvedValue(undefined),
      saveSidebarLayout: vi.fn().mockResolvedValue(undefined),
    }));

    const { useConnectionStore } = await import("@/stores/connectionStore");
    const store = useConnectionStore();
    const connection = postgresConnection({ connect_timeout_secs: 10 });
    store.connections = [connection];
    store.connectedIds.add(connection.id);
    store.treeNodes = [{ id: connection.id, label: connection.name, type: "connection", connectionId: connection.id, isLoading: false, children: [] }];

    expect(await store.loadConnectionLifecycleDiagnostics(connection.id)).toMatchObject({
      activeQueryCount: 1,
      poolKeys: ["pg-1:app"],
    });

    await store.forceClearPoolsAndReconnect(connection.id);
    const diagnostics = await store.loadConnectionLifecycleDiagnostics(connection.id);

    expect(disconnectDb).toHaveBeenCalledWith(connection.id, undefined);
    expect(diagnostics.activeQueryCount).toBeUndefined();
    expect(diagnostics.poolKeys).toBeUndefined();
  });

  it("scopes a normal disconnect to the active connection attempt when one is running", async () => {
    let resolveConnect!: (connectionId: string) => void;
    const connectDb = vi.fn(
      () =>
        new Promise<string>((resolve) => {
          resolveConnect = resolve;
        }),
    );
    const disconnectDb = vi.fn().mockResolvedValue(undefined);

    vi.doMock("@/lib/backend/tauriRuntime", () => ({ isTauriRuntime: () => false }));
    vi.doMock("@/lib/backend/api", () => ({
      connectDb,
      deleteSchemaCachePrefix: vi.fn().mockResolvedValue(undefined),
      disconnectDb,
      saveConnections: vi.fn().mockResolvedValue(undefined),
      saveSidebarLayout: vi.fn().mockResolvedValue(undefined),
    }));

    const { CONNECTION_ATTEMPT_CANCELLED_MESSAGE, useConnectionStore } = await import("@/stores/connectionStore");
    const store = useConnectionStore();
    const connection = postgresConnection({ connect_timeout_secs: 10 });
    store.connections = [connection];

    const connect = store.connect(connection).catch((error) => error);
    await vi.advanceTimersByTimeAsync(1);
    const activeAttempt = connectDb.mock.calls[0]?.[1];

    await store.disconnect(connection.id);
    expect(disconnectDb).toHaveBeenCalledWith(connection.id, activeAttempt);
    expect(store.connectedIds.has(connection.id)).toBe(false);

    resolveConnect(connection.id);
    await vi.advanceTimersByTimeAsync(1);
    const error = await connect;

    expect(error).toBeInstanceOf(Error);
    expect(error.message).toContain(CONNECTION_ATTEMPT_CANCELLED_MESSAGE);
    expect(disconnectDb).toHaveBeenLastCalledWith(connection.id, activeAttempt);
    expect(store.connectedIds.has(connection.id)).toBe(false);
  }, 10_000);

  it("cleans up backend state when a cancelled connection later succeeds", async () => {
    let resolveConnect!: (connectionId: string) => void;
    const connectDb = vi.fn(
      () =>
        new Promise<string>((resolve) => {
          resolveConnect = resolve;
        }),
    );
    const disconnectDb = vi.fn().mockResolvedValue(undefined);

    vi.doMock("@/lib/backend/tauriRuntime", () => ({ isTauriRuntime: () => false }));
    vi.doMock("@/lib/backend/api", () => ({
      connectDb,
      deleteSchemaCachePrefix: vi.fn().mockResolvedValue(undefined),
      disconnectDb,
      saveConnections: vi.fn().mockResolvedValue(undefined),
      saveSidebarLayout: vi.fn().mockResolvedValue(undefined),
    }));

    const { CONNECTION_ATTEMPT_CANCELLED_MESSAGE, useConnectionStore } = await import("@/stores/connectionStore");
    const store = useConnectionStore();
    const connection = postgresConnection({ connect_timeout_secs: 10 });
    store.connections = [connection];

    const ensure = store.ensureConnected(connection.id).catch((error) => error);
    await vi.advanceTimersByTimeAsync(1);
    const attempt = connectDb.mock.calls[0]?.[1];

    await expect(store.cancelConnecting(connection.id)).resolves.toBe(true);
    expect(disconnectDb).toHaveBeenCalledTimes(1);
    expect(disconnectDb).toHaveBeenCalledWith(connection.id, attempt);

    resolveConnect(connection.id);
    await vi.advanceTimersByTimeAsync(1);
    const error = await ensure;

    expect(error).toBeInstanceOf(Error);
    expect(error.message).toContain(CONNECTION_ATTEMPT_CANCELLED_MESSAGE);
    expect(disconnectDb).toHaveBeenCalledTimes(2);
    expect(disconnectDb).toHaveBeenLastCalledWith(connection.id, attempt);
    expect(store.connectedIds.has(connection.id)).toBe(false);
    expect(store.connectionErrors[connection.id]).toBeUndefined();
  }, 10_000);

  it("keeps errors from earlier cancelled attempts hidden after a second cancel", async () => {
    let rejectFirstConnect!: (error: Error) => void;
    let rejectSecondConnect!: (error: Error) => void;
    let connectCallCount = 0;
    const connectDb = vi.fn(() => {
      connectCallCount += 1;
      return new Promise<string>((_, reject) => {
        if (connectCallCount === 1) {
          rejectFirstConnect = reject;
        } else {
          rejectSecondConnect = reject;
        }
      });
    });
    const disconnectDb = vi.fn().mockResolvedValue(undefined);

    vi.doMock("@/lib/backend/tauriRuntime", () => ({ isTauriRuntime: () => false }));
    vi.doMock("@/lib/backend/api", () => ({
      connectDb,
      deleteSchemaCachePrefix: vi.fn().mockResolvedValue(undefined),
      disconnectDb,
      saveConnections: vi.fn().mockResolvedValue(undefined),
      saveSidebarLayout: vi.fn().mockResolvedValue(undefined),
    }));

    const { CONNECTION_ATTEMPT_CANCELLED_MESSAGE, useConnectionStore } = await import("@/stores/connectionStore");
    const store = useConnectionStore();
    const connection = postgresConnection({ connect_timeout_secs: 10 });
    store.connections = [connection];

    const firstEnsure = store.ensureConnected(connection.id).catch((error) => error);
    await vi.advanceTimersByTimeAsync(1);
    expect(connectDb).toHaveBeenCalledTimes(1);
    await expect(store.cancelConnecting(connection.id)).resolves.toBe(true);

    const secondEnsure = store.ensureConnected(connection.id).catch((error) => error);
    await vi.advanceTimersByTimeAsync(1);
    expect(connectDb).toHaveBeenCalledTimes(2);
    await expect(store.cancelConnecting(connection.id)).resolves.toBe(true);

    rejectFirstConnect(new Error("first connection failed after cancel"));
    const firstError = await firstEnsure;
    expect(firstError).toBeInstanceOf(Error);
    expect(firstError.message).toContain(CONNECTION_ATTEMPT_CANCELLED_MESSAGE);
    expect(store.connectionErrors[connection.id]).toBeUndefined();

    rejectSecondConnect(new Error("second connection failed after cancel"));
    const secondError = await secondEnsure;
    expect(secondError).toBeInstanceOf(Error);
    expect(secondError.message).toContain(CONNECTION_ATTEMPT_CANCELLED_MESSAGE);
    expect(store.connectionErrors[connection.id]).toBeUndefined();
  }, 10_000);
});
