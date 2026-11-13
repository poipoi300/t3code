// @effect-diagnostics nodeBuiltinImport:off - the database must be created and removed in the OS temporary directory.
import * as NodeFSP from "node:fs/promises";
import * as NodeOS from "node:os";
import * as NodePath from "node:path";
import * as NodeModule from "node:module";
import * as NodeURL from "node:url";
import { Effect, Layer } from "./effect.ts";
import * as Sqlite from "../../../apps/server/src/persistence/Sqlite.ts";
import * as SqlClient from "../../../apps/server/node_modules/effect/dist/sql/SqlClient.js";

const require = NodeModule.createRequire(
  new URL("../../../apps/server/package.json", import.meta.url),
);
const NodeServices: typeof import("../../../apps/server/node_modules/@effect/platform-node/dist/NodeServices.js") =
  await import(NodeURL.pathToFileURL(require.resolve("@effect/platform-node/NodeServices")).href);

export async function exportDatabase() {
  const temporaryRoot = NodePath.resolve(NodeOS.tmpdir());
  const directory = await NodeFSP.mkdtemp(NodePath.join(temporaryRoot, "t3-rust-fixtures-"));
  const relative = NodePath.relative(temporaryRoot, directory);
  if (relative.startsWith("..") || NodePath.isAbsolute(relative))
    throw new Error("Temporary database escaped the OS temporary directory");
  try {
    const sources = await Promise.all([
      NodeFSP.readFile(
        new URL("../../../apps/server/src/persistence/Sqlite.ts", import.meta.url),
        "utf8",
      ),
      NodeFSP.readFile(
        new URL("../../../packages/shared/src/nodeSqliteClient.ts", import.meta.url),
        "utf8",
      ),
    ]);
    const pragmaNames = [
      ...new Set(
        sources.flatMap((source) =>
          [...source.matchAll(/\bPRAGMA\s+(\w+)\s*=/gi)]
            .map((match) => match[1]?.toLowerCase())
            .filter((name): name is string => name !== undefined),
        ),
      ),
    ].sort();
    return await Effect.runPromise(
      Effect.gen(function* () {
        const sql = yield* SqlClient.SqlClient;
        const objects = yield* sql<{ type: string; name: string; sql: string }>`
        SELECT type, name, sql FROM sqlite_master
        WHERE sql IS NOT NULL AND name NOT LIKE 'sqlite_%'
        AND name NOT IN (SELECT name FROM pragma_table_list WHERE type = 'shadow')
        ORDER BY CASE type WHEN 'table' THEN 0 WHEN 'index' THEN 1 WHEN 'trigger' THEN 2 ELSE 3 END, name COLLATE BINARY`;
        const migrations = yield* sql<{
          id: number;
          name: string;
        }>`SELECT migration_id AS id, name FROM effect_sql_migrations ORDER BY migration_id`;
        const pragmas: Record<string, unknown> = {};
        for (const name of pragmaNames) pragmas[name] = (yield* sql.unsafe(`PRAGMA ${name}`))[0];
        return {
          sql: objects.map((object) => `${object.sql.trim().replace(/;$/, "")};\n`).join("\n"),
          migrations,
          pragmas,
          tables: objects.filter((object) => object.type === "table").length,
        };
      }).pipe(
        Effect.provide(
          Layer.provide(
            Sqlite.layerFromPath(NodePath.join(directory, "statev2.sqlite")),
            NodeServices.layer,
          ),
        ),
        Effect.scoped,
      ),
    );
  } finally {
    await NodeFSP.rm(directory, { recursive: true, force: true });
  }
}
