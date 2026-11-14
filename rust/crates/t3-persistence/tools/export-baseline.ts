// Run with Node from the repository root. All databases live in an OS temp directory.
import * as Assert from "node:assert/strict";
import { DatabaseSync } from "node:sqlite";
import * as FS from "node:fs/promises";
import * as OS from "node:os";
import * as Path from "node:path";
import * as Module from "node:module";
import { pathToFileURL } from "node:url";
import { Effect } from "../../../tools/export/effect.ts";
import * as NodeSqlite from "../../../../packages/shared/src/nodeSqliteClient.ts";
import { migrationManifest, runMigrations } from "../../../../apps/server/src/persistence/Migrations.ts";
const require = Module.createRequire(
  new URL("../../../../apps/server/package.json", import.meta.url),
);
const SqlClient = await import(pathToFileURL(require.resolve("effect/sql/SqlClient")).href);
const directory = await FS.mkdtemp(Path.join(OS.tmpdir(), "t3-persistence-baseline-"));
try {
  for (const [head] of migrationManifest.filter(([id]) => id >= 69)) {
    const sql = await Effect.runPromise(
      Effect.gen(function* () {
        const db = yield* SqlClient.SqlClient;
        yield* runMigrations({ toMigrationInclusive: head });
        const objects = yield* db`SELECT type, name, sql FROM sqlite_master
        WHERE sql IS NOT NULL AND name NOT LIKE 'sqlite_%'
        AND name NOT IN (SELECT name FROM pragma_table_list WHERE type = 'shadow')
        ORDER BY rowid`;
        if (process.argv.includes("--check")) {
          const expected = yield* db`SELECT name FROM sqlite_schema WHERE sql IS NOT NULL
          AND name NOT LIKE 'sqlite_%' AND name NOT IN (SELECT name FROM pragma_table_list WHERE type = 'shadow') ORDER BY rowid`;
          const saved = yield* Effect.promise(() =>
            FS.readFile(new URL(`../sql/head-${head}.sql`, import.meta.url), "utf8"),
          );
          const replay = new DatabaseSync(":memory:");
          try {
            replay.exec(saved);
            const actual = replay
              .prepare(`SELECT name FROM sqlite_schema WHERE sql IS NOT NULL
            AND name NOT LIKE 'sqlite_%' AND name NOT IN (SELECT name FROM pragma_table_list WHERE type = 'shadow') ORDER BY rowid`)
              .all();
            Assert.deepEqual(
              actual.map((row) => row.name),
              expected.map((row) => row.name),
              `head ${head} schema creation order`,
            );
          } finally {
            replay.close();
          }
        }
        let result = objects.map((row) => row.sql.trim().replace(/;$/, "") + ";\n").join("\n");
        for (const table of objects.filter(
          (row) =>
            row.type === "table" &&
            row.name !== "effect_sql_migrations" &&
            !row.sql.includes("VIRTUAL TABLE"),
        )) {
          const columns = yield* db.unsafe(`PRAGMA table_info("${table.name}")`);
          const rows = yield* db.unsafe(
            `SELECT ${columns.map((column) => `quote("${column.name}") AS "${column.name}"`).join(",")} FROM "${table.name}"`,
          );
          for (const row of rows)
            result += `\nINSERT INTO "${table.name}" VALUES (${columns.map((column) => (table.name === "orchestration_v2_projection_metadata" && column.name === "updated_at" ? "strftime('%Y-%m-%dT%H:%M:%fZ', 'now')" : row[column.name])).join(",")});\n`;
        }
        return result;
      }).pipe(
        Effect.provide(NodeSqlite.layer({ filename: Path.join(directory, `head-${head}.sqlite`) })),
        Effect.scoped,
      ),
    );
    if (process.argv.includes("--check")) continue;
    await FS.mkdir(new URL("../sql/", import.meta.url), { recursive: true });
    await FS.writeFile(new URL(`../sql/head-${head}.sql`, import.meta.url), sql);
  }
} finally {
  await FS.rm(directory, { recursive: true, force: true });
}
