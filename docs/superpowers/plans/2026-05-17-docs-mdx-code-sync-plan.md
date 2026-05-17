# DBX Docs MDX Code Sync Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [x]`) syntax for tracking.

**Goal:** Update the DBX static docs MDX pages in English and Simplified Chinese so they match the actual product behavior implemented in this repository.

**Architecture:** Treat repository code as the documentation source of truth, then revise existing Fumadocs MDX content in bilingual pairs. Keep the existing docs framework and components, and validate the result with file scans plus the docs build.

**Tech Stack:** Fumadocs MDX, Next.js docs app, Vue/Tauri frontend, Rust `dbx-core`, Node MCP server.

---

## File Map

- Modify: `docs/content/docs/*.mdx` and `docs/content/docs/*.cn.mdx`
  - Existing bilingual documentation pages.
  - Keep page slugs unchanged.
  - Keep English and Simplified Chinese page structures equivalent.
- Read only: `src/types/database.ts`
  - Database type names and shared frontend models.
- Read only: `src/components/connection/ConnectionDialog.vue`
  - Connection profiles, default ports, file-based types, SSH/proxy behavior, JDBC fields.
- Read only: `src/lib/databaseCapabilitySets.ts`
  - Feature support by database type.
- Read only: `src/lib/api.ts`, `src/lib/tauri.ts`, `src/lib/http.ts`
  - Desktop/web API parity and feature endpoints.
- Read only: `src/lib/ai.ts`, `src/lib/aiSqlExecutionPolicy.ts`, `src/lib/aiSkills.ts`
  - AI Ask/Agent behavior and SQL safety policy.
- Read only: `crates/dbx-core/src/sql.rs`
  - SQL file splitting and batch behavior.
- Read only: `crates/dbx-core/src/table_import.rs`
  - File import formats, preview, mapping, batching, append/truncate modes.
- Read only: `crates/dbx-core/src/transfer.rs`
  - Transfer modes, create table behavior, batching, cancellation.
- Read only: `crates/dbx-core/src/database_export.rs`
  - Export SQL contents, selected table filtering, progress, cancellation.
- Read only: `crates/dbx-core/src/plugins.rs`
  - Plugin/JDBC protocol behavior and timeout.
- Read only: `mcp/src/*.ts`
  - MCP tools, desktop/web mode, SQL safety environment variables.
- Modify: `docs/superpowers/plans/2026-05-17-docs-mdx-code-sync-plan.md`
  - Track task progress as execution proceeds.

---

### Task 1: Build A Documentation Fact Baseline

**Files:**
- Read: `src/types/database.ts`
- Read: `src/components/connection/ConnectionDialog.vue`
- Read: `src/lib/databaseCapabilitySets.ts`
- Read: `src/lib/api.ts`
- Read: `src/lib/ai.ts`
- Read: `src/lib/aiSqlExecutionPolicy.ts`
- Read: `crates/dbx-core/src/sql.rs`
- Read: `crates/dbx-core/src/table_import.rs`
- Read: `crates/dbx-core/src/transfer.rs`
- Read: `crates/dbx-core/src/database_export.rs`
- Read: `crates/dbx-core/src/plugins.rs`
- Read: `mcp/src/index.ts`
- Read: `mcp/src/sql-safety.ts`
- Modify: `docs/superpowers/plans/2026-05-17-docs-mdx-code-sync-plan.md`

- [x] **Step 1: Extract database type and profile facts**

Run:

```bash
sed -n '1,80p' src/types/database.ts
sed -n '113,230p' src/components/connection/ConnectionDialog.vue
sed -n '404,445p' src/components/connection/ConnectionDialog.vue
```

Expected:

- A complete `DatabaseType` union.
- Connection profile labels, default ports, and profile-to-driver mappings.
- A picker list that shows user-visible database choices.

- [x] **Step 2: Extract feature support boundaries**

Run:

```bash
sed -n '1,220p' src/lib/databaseCapabilitySets.ts
```

Expected:

- Sets for schema-aware databases, SQL file unsupported types, diagrams, database search, table import, table structure, create database, field lineage, and transfer support.

- [x] **Step 3: Extract workflow and safety facts**

Run:

```bash
sed -n '1,160p' src/lib/api.ts
sed -n '1,220p' src/lib/ai.ts
sed -n '1,180p' src/lib/aiSqlExecutionPolicy.ts
sed -n '1,220p' crates/dbx-core/src/sql.rs
sed -n '1,420p' crates/dbx-core/src/table_import.rs
sed -n '1,260p' crates/dbx-core/src/transfer.rs
sed -n '1,470p' crates/dbx-core/src/database_export.rs
sed -n '1,380p' mcp/src/index.ts
sed -n '1,180p' mcp/src/sql-safety.ts
```

Expected:

- Concrete facts for AI modes, SQL safety, SQL file splitting, import formats, transfer modes, export contents, MCP tools, and MCP safety defaults.

- [x] **Step 4: Mark this task complete in this plan**

Edit this file and change Task 1 checkboxes to checked after facts are collected.

---

### Task 2: Deeply Revise Core Setup And Database Pages

**Files:**
- Modify: `docs/content/docs/getting-started.mdx`
- Modify: `docs/content/docs/getting-started.cn.mdx`
- Modify: `docs/content/docs/databases.mdx`
- Modify: `docs/content/docs/databases.cn.mdx`
- Modify: `docs/content/docs/plugins.mdx`
- Modify: `docs/content/docs/plugins.cn.mdx`
- Modify: `docs/content/docs/ssh-tunnel.mdx`
- Modify: `docs/content/docs/ssh-tunnel.cn.mdx`
- Modify: `docs/content/docs/config-export.mdx`
- Modify: `docs/content/docs/config-export.cn.mdx`
- Modify: `docs/superpowers/plans/2026-05-17-docs-mdx-code-sync-plan.md`

- [x] **Step 1: Update Getting Started in both locales**

Use these facts:

- Desktop and Docker share the same Vue frontend through different backends.
- Desktop uses Tauri commands; Docker/web uses HTTP routes.
- Connection setup supports database type selection, connection URL parsing for common schemes, SSH, proxy, SSL, color labels, and visible database filtering where applicable.
- File-based database types include SQLite, DuckDB, and Access.
- Connection secrets are stored separately from the ordinary connection JSON.

Content requirements:

- Keep install tabs.
- Add a concise "Desktop vs Docker" table.
- Expand connection creation steps with URL import, SSH/proxy, file-based databases, and test/save flow.
- Add safety notes for secrets and production connection colors.

- [x] **Step 2: Update Database Support in both locales**

Use these facts:

- Full type set comes from `src/types/database.ts`.
- User-visible profile list comes from `ConnectionDialog.vue`.
- Feature capability sets come from `src/lib/databaseCapabilitySets.ts`.
- Some choices are native, some compatibility profiles, some Agent/JDBC-backed.

Content requirements:

- Replace the incomplete "Fully Supported" table with grouped support tables:
  - Built-in/common engines.
  - Compatibility profiles.
  - Agent/JDBC-oriented engines.
  - File-based engines.
- Add a feature support matrix for schema browser, ER/diagram, database search, table import, table structure editor, field lineage, SQL file execution, and data transfer.
- Keep DM/ODBC notes where still relevant.
- Add a note that feature support is intentionally database-specific.

- [x] **Step 3: Update JDBC Plugin docs in both locales**

Use these facts:

- The plugin protocol version is `SUPPORTED_PLUGIN_PROTOCOL_VERSION`.
- Plugin calls have a request timeout.
- JDBC plugin is optional and drivers are not bundled.
- JDBC connections carry `jdbc_driver_class` and `jdbc_driver_paths`.

Content requirements:

- Clarify main app vs optional plugin responsibilities.
- Document driver JAR import, driver class, connection test, and troubleshooting boundaries.
- Add security and compatibility notes.

- [x] **Step 4: Lightly update SSH Tunnel and Config Export in both locales**

Use these facts:

- SSH supports password and key auth, key passphrase, connect timeout, and optional LAN exposure.
- Proxy supports SOCKS5 and HTTP fields in the connection model.
- Config export/import should distinguish ordinary config from secret handling.

Content requirements:

- Add concise boundary notes without turning these pages into internals docs.
- Ensure links to Getting Started and Database Support are present.

- [x] **Step 5: Mark this task complete in this plan**

Edit this file and check all Task 2 boxes after both English and Chinese files are updated.

---

### Task 3: Deeply Revise Core Workflow Pages

**Files:**
- Modify: `docs/content/docs/query-editor.mdx`
- Modify: `docs/content/docs/query-editor.cn.mdx`
- Modify: `docs/content/docs/data-grid.mdx`
- Modify: `docs/content/docs/data-grid.cn.mdx`
- Modify: `docs/content/docs/schema-browser.mdx`
- Modify: `docs/content/docs/schema-browser.cn.mdx`
- Modify: `docs/content/docs/schema-diff.mdx`
- Modify: `docs/content/docs/schema-diff.cn.mdx`
- Modify: `docs/content/docs/table-structure.mdx`
- Modify: `docs/content/docs/table-structure.cn.mdx`
- Modify: `docs/content/docs/field-lineage.mdx`
- Modify: `docs/content/docs/field-lineage.cn.mdx`
- Modify: `docs/superpowers/plans/2026-05-17-docs-mdx-code-sync-plan.md`

- [x] **Step 1: Update Query Editor in both locales**

Use these facts:

- SQL execution APIs include query execution, multi execution, batch/script execution, transaction execution, cancellation, and query session close.
- Statement selection and cursor statement logic are handled in frontend helpers.
- Completion uses metadata and dialect awareness.
- AI can use the current SQL and schema context but should not imply automatic execution in Ask mode.

Content requirements:

- Add "execution target" explanation for selected SQL, current statement, and full editor contents.
- Add cancellation/session notes.
- Link to AI Assistant and Data Grid.
- Keep shortcut table.

- [x] **Step 2: Update Data Grid in both locales**

Use these facts:

- Grid supports virtual scrolling, selection, column width, sorting, pagination, editing, row status, export formats, Markdown table export, and SQL preview for edits.
- Query results may be read-only depending on query shape and available primary keys.

Content requirements:

- Add "when editing is available" and "when result is read-only" sections.
- Add review-before-save behavior.
- Add export and copy formats.

- [x] **Step 3: Update Schema Browser and related schema pages in both locales**

Use these facts:

- Schema browser handles relational trees, Redis DB/key trees, MongoDB databases/collections, object browser, saved SQL library, pinned items, search, visible databases, object source, and refresh targets.
- Diagram and field lineage support are database-specific from capability sets.
- Table structure editor support is database-specific.

Content requirements:

- Add database-specific object models.
- Add capability boundary tables.
- Cross-link schema diff, table structure, field lineage, database search if documented.

- [x] **Step 4: Mark this task complete in this plan**

Edit this file and check all Task 3 boxes after updates are complete.

---

### Task 4: Deeply Revise Data Movement And Automation Pages

**Files:**
- Modify: `docs/content/docs/data-transfer.mdx`
- Modify: `docs/content/docs/data-transfer.cn.mdx`
- Modify: `docs/content/docs/table-import.mdx`
- Modify: `docs/content/docs/table-import.cn.mdx`
- Modify: `docs/content/docs/sql-file.mdx`
- Modify: `docs/content/docs/sql-file.cn.mdx`
- Modify: `docs/content/docs/database-export.mdx`
- Modify: `docs/content/docs/database-export.cn.mdx`
- Modify: `docs/content/docs/ai-assistant.mdx`
- Modify: `docs/content/docs/ai-assistant.cn.mdx`
- Modify: `docs/content/docs/mcp.mdx`
- Modify: `docs/content/docs/mcp.cn.mdx`
- Modify: `docs/superpowers/plans/2026-05-17-docs-mdx-code-sync-plan.md`

- [x] **Step 1: Update Data Transfer in both locales**

Use these facts:

- Transfer modes include append, overwrite, and upsert.
- Transfer can create target tables, map basic column types, batch rows, report progress, and cancel.
- SQL transfer support is database-specific.

Content requirements:

- Add mode table.
- Add source/target database support boundaries.
- Add review and cancellation notes.

- [x] **Step 2: Update Table Import in both locales**

Use these facts:

- Supported files are CSV, TSV, JSON, XLSX, XLSM, XLS.
- Preview limit defaults to 50 rows.
- Import batch size defaults to 500 rows.
- JSON import accepts object, array of objects, or array rows; mixed row shapes are rejected.
- Empty CSV values become `NULL`.
- Modes are append and truncate.

Content requirements:

- Add file format table with parser behavior.
- Add mapping rules and duplicate target column warning.
- Add append/truncate mode safety callout.

- [x] **Step 3: Update SQL File and Database Export in both locales**

Use these facts:

- SQL file execution supports preview, progress, cancellation, continue-on-error, semicolon-aware splitting, PostgreSQL dollar quotes, and SQL Server `GO` batches.
- SQL file is unsupported for Redis, MongoDB, and Elasticsearch.
- Export includes table DDL, data inserts, supported views/procedures/functions, selected table filtering, progress, and cancellation.

Content requirements:

- Add SQL parser behavior table.
- Add supported/unsupported database notes.
- Add backup and import/export round-trip guidance.

- [x] **Step 4: Update AI Assistant in both locales**

Use these facts:

- AI actions include generate, explain, optimize, fix, convert, and sample data.
- Modes include Ask and Agent.
- Schema context includes tables, columns, indexes, and foreign keys, with truncation behavior.
- SQL execution policy auto-executes read statements only when agent intent is clear, confirms writes in uncertain or production-like contexts, and blocks dangerous statements.

Content requirements:

- Add Ask vs Agent mode table.
- Add SQL safety table.
- Add schema context and `@table` mention guidance.

- [x] **Step 5: Update MCP in both locales**

Use these facts:

- Tools include list connections, list tables, describe table, execute query, get schema context, add/remove connection, and desktop-only open table/execute-and-show.
- MCP query execution returns max 100 rows.
- MCP defaults to one statement and read-only SQL.
- `DBX_MCP_ALLOW_WRITES` and `DBX_MCP_ALLOW_DANGEROUS_SQL` change safety behavior.
- `DBX_WEB_URL` enables web mode.

Content requirements:

- Add tool table.
- Add desktop vs web mode table.
- Add safety environment variable section.

- [x] **Step 6: Mark this task complete in this plan**

Edit this file and check all Task 4 boxes after updates are complete.

---

### Task 5: Light Site-Wide Consistency Pass

**Files:**
- Modify: `docs/content/docs/what-is-dbx.mdx`
- Modify: `docs/content/docs/what-is-dbx.cn.mdx`
- Review: `docs/content/docs/changelog.mdx`
- Review: `docs/content/docs/changelog.cn.mdx`
- Modify: any previously touched MDX page with broken cross-links or mismatched headings
- Modify: `docs/superpowers/plans/2026-05-17-docs-mdx-code-sync-plan.md`

- [x] **Step 1: Update What Is DBX in both locales**

Use these facts:

- DBX has desktop and Docker/web deployment modes.
- DBX includes SQL editing, data grid, schema tools, Redis/Mongo dedicated browsers, data movement tools, AI, MCP, plugins, and configuration migration.

Content requirements:

- Keep page concise.
- Ensure feature list matches the revised core pages.
- Keep screenshot reference intact unless broken.

- [x] **Step 2: Review changelog pages without inventing releases**

Run:

```bash
sed -n '1,60p' docs/content/docs/changelog.mdx
sed -n '1,60p' docs/content/docs/changelog.cn.mdx
```

Expected:

- If only intro wording or links need consistency fixes, edit them.
- Do not add unreleased entries.

- [x] **Step 3: Scan all MDX links for obvious locale mistakes**

Run:

```bash
rg -n "\\](/docs/|\\](/en/docs/|\\](/cn/docs/" docs/content/docs -g '*.mdx'
```

Expected:

- English docs use `/en/docs/...` or the established valid route.
- Chinese docs use `/cn/docs/...`.
- No accidentally mixed locale links in newly edited sections.

- [x] **Step 4: Mark this task complete in this plan**

Edit this file and check all Task 5 boxes after updates are complete.

---

### Task 6: Verify And Commit Documentation Changes

**Files:**
- Verify: `docs/content/docs/*.mdx`
- Verify: `docs/package.json`
- Modify: `docs/superpowers/plans/2026-05-17-docs-mdx-code-sync-plan.md`

- [x] **Step 1: Check bilingual page pairs exist**

Run:

```bash
node - <<'NODE'
const fs = require('fs');
const path = require('path');
const dir = 'docs/content/docs';
const files = fs.readdirSync(dir).filter((f) => f.endsWith('.mdx'));
const base = new Set(files.filter((f) => !f.endsWith('.cn.mdx')).map((f) => f.replace(/\.mdx$/, '')));
const cn = new Set(files.filter((f) => f.endsWith('.cn.mdx')).map((f) => f.replace(/\.cn\.mdx$/, '')));
let ok = true;
for (const name of base) {
  if (name === 'changelog') {}
  if (!cn.has(name) && name !== 'meta') {
    console.log(`Missing Chinese page for ${name}`);
    ok = false;
  }
}
for (const name of cn) {
  if (!base.has(name)) {
    console.log(`Missing English page for ${name}`);
    ok = false;
  }
}
process.exit(ok ? 0 : 1);
NODE
```

Expected: exit code 0 and no missing page output.

- [x] **Step 2: Check English/Chinese heading parity for edited page pairs**

Run:

```bash
node - <<'NODE'
const fs = require('fs');
const pairs = [
  'what-is-dbx','getting-started','databases','query-editor','data-grid','schema-browser',
  'schema-diff','data-transfer','table-structure','field-lineage','table-import','sql-file',
  'database-export','ai-assistant','mcp','plugins','config-export','ssh-tunnel'
];
for (const slug of pairs) {
  const en = fs.readFileSync(`docs/content/docs/${slug}.mdx`, 'utf8').split('\n').filter((l) => /^#{2,4} /.test(l));
  const cn = fs.readFileSync(`docs/content/docs/${slug}.cn.mdx`, 'utf8').split('\n').filter((l) => /^#{2,4} /.test(l));
  if (en.length !== cn.length) {
    console.log(`${slug}: heading count differs en=${en.length} cn=${cn.length}`);
    process.exitCode = 1;
  }
}
NODE
```

Expected: no output. If output appears, inspect whether the mismatch is intentional. Fix unintentional mismatches.

- [x] **Step 3: Build the docs site**

Run:

```bash
pnpm --dir docs build
```

Expected: Next/Fumadocs build completes successfully.

- [x] **Step 4: Review git diff**

Run:

```bash
git diff -- docs/content/docs docs/superpowers/plans/2026-05-17-docs-mdx-code-sync-plan.md
git status --short
```

Expected:

- Only documentation content and plan progress changed.
- No generated cache, build output, or unrelated source changes are included.

- [x] **Step 5: Commit docs content changes**

Run:

```bash
git add docs/content/docs docs/superpowers/plans/2026-05-17-docs-mdx-code-sync-plan.md
git commit -m "docs: sync MDX content with implementation"
```

Expected:

- Commit succeeds with a Conventional Commit message.

- [x] **Step 6: Mark this task complete in this plan**

If the plan file is already committed before this checkbox is marked, leave the completion status in the final response instead of making a follow-up commit only for this checkbox.
