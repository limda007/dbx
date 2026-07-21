import { describe, expect, it } from "vitest";
import { getDatabaseUserAdminProvider, kingbaseShowGrantsSql, mysqlPrivilegeSelectionFromGrants } from "@/lib/database/databaseUserAdmin";

describe("MySQL grant privilege selection", () => {
  const availablePrivileges = ["SELECT", "INSERT", "UPDATE", "EXECUTE"];

  it("expands ALL PRIVILEGES and reads WITH GRANT OPTION for the current scope", () => {
    expect(
      mysqlPrivilegeSelectionFromGrants({
        grants: ["grant all privileges on `*`.* to 'root'@'%' WITH GRANT OPTION"],
        database: "*",
        table: "*",
        availablePrivileges,
      }),
    ).toEqual({ privileges: availablePrivileges, grantOption: true });
  });

  it("merges matching grants case-insensitively", () => {
    expect(
      mysqlPrivilegeSelectionFromGrants({
        grants: ["GRANT SELECT, insert ON `Sales`.`Orders` TO 'app'@'%'", "GRANT UPDATE ON sales.orders TO 'app'@'%' WITH GRANT OPTION"],
        database: "SALES",
        table: "orders",
        availablePrivileges,
      }),
    ).toEqual({ privileges: ["SELECT", "INSERT", "UPDATE"], grantOption: true });
  });

  it("supports escaped backticks and ignores grants for other scopes or routines", () => {
    expect(
      mysqlPrivilegeSelectionFromGrants({
        grants: ["GRANT SELECT ON `tenant``db`.`audit``log` TO 'app'@'%'", "GRANT INSERT ON `tenant``db`.* TO 'app'@'%' WITH GRANT OPTION", "GRANT EXECUTE ON PROCEDURE `tenant``db`.`audit``log` TO 'app'@'%' WITH GRANT OPTION"],
        database: "tenant`db",
        table: "audit`log",
        availablePrivileges,
      }),
    ).toEqual({ privileges: ["SELECT"], grantOption: false });
  });

  it("returns an empty selection when the current scope has no grant", () => {
    expect(
      mysqlPrivilegeSelectionFromGrants({
        grants: ["GRANT SELECT ON `other`.* TO 'app'@'%' WITH GRANT OPTION"],
        database: "current",
        table: "*",
        availablePrivileges,
      }),
    ).toEqual({ privileges: [], grantOption: false });
  });
});

describe("database user admin providers", () => {
  it("syncs loaded grants only for the MySQL provider", () => {
    const mysqlProvider = getDatabaseUserAdminProvider("mysql");
    const postgresProvider = getDatabaseUserAdminProvider("postgres");
    const starrocksProvider = getDatabaseUserAdminProvider("starrocks");

    expect(
      mysqlProvider?.privilegeSelectionFromGrants?.({
        grants: ["GRANT INSERT ON `app`.* TO 'user'@'%'"],
        database: "app",
        table: "*",
        availablePrivileges: mysqlProvider.privilegesForScope?.("mysql") ?? [],
      }),
    ).toEqual({ privileges: ["INSERT"], grantOption: false });
    expect(postgresProvider?.privilegeSelectionFromGrants).toBeUndefined();
    expect(starrocksProvider?.privilegeSelectionFromGrants).toBeUndefined();
    expect(mysqlProvider?.defaultPrivilegesForScope?.("mysql")).toEqual(["SELECT"]);
    expect(postgresProvider?.defaultPrivilegesForScope?.("database")).toEqual(["CONNECT"]);
    expect(starrocksProvider?.defaultPrivilegesForScope?.("table")).toEqual(["SELECT"]);
  });

  it("uses sys_catalog for Kingbase role metadata", () => {
    const provider = getDatabaseUserAdminProvider("kingbase");

    expect(provider).not.toBeNull();
    expect(provider?.dialect).toBe("postgres");
    expect(provider?.listUsersSql()).toContain("FROM sys_catalog.sys_roles r");
    expect(provider?.listUsersSql()).not.toContain("pg_catalog");
  });

  it("builds Kingbase grant SQL without PostgreSQL catalog tables", () => {
    const sql = kingbaseShowGrantsSql({ user: "role'o", host: "LOGIN" });

    expect(sql).toContain("FROM sys_catalog.sys_roles r");
    expect(sql).toContain("FROM sys_catalog.sys_auth_members m");
    expect(sql).toContain("CROSS JOIN sys_catalog.sys_database d");
    expect(sql).toContain("CROSS JOIN sys_catalog.sys_namespace n");
    expect(sql).toContain("WHERE r.rolname = 'role''o'");
    expect(sql).not.toContain("pg_catalog");
    expect(sql).not.toContain("pg_roles");
  });
});
