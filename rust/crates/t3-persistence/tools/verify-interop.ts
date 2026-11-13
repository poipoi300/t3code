// Run from the repository root; requires Node and the repository's JS dependencies.
import { DatabaseSync } from "node:sqlite";
import * as Assert from "node:assert/strict";
import * as FS from "node:fs/promises";
import * as OS from "node:os";
import * as Path from "node:path";
import * as Module from "node:module";
import { fileURLToPath, pathToFileURL } from "node:url";
import { spawnSync } from "node:child_process";
import { Effect } from "../../../tools/export/effect.ts";
import * as NodeSqlite from "../../../../packages/shared/src/nodeSqliteClient.ts";
import { runMigrations } from "../../../../apps/server/src/persistence/Migrations.ts";
import { serializeAssistantCitation } from "../../../../packages/shared/src/assistantCitations.ts";
import {
  EnvironmentId,
  MessageId,
  ThreadId,
} from "../../../../packages/contracts/src/baseSchemas.ts";
import type { Effect as EffectProgram } from "../../../../apps/server/node_modules/effect/dist/Effect.js";
import type { SqlClient as SqlService } from "../../../../apps/server/node_modules/effect/dist/sql/SqlClient.js";
const require = Module.createRequire(
  new URL("../../../../apps/server/package.json", import.meta.url),
);
const SqlClient = await import(pathToFileURL(require.resolve("effect/sql/SqlClient")).href);
const directory = await FS.mkdtemp(Path.join(OS.tmpdir(), "t3-persistence-interop-"));
const cargo = (path: string, head?: number) => {
  const result = spawnSync(
    "cargo",
    [
      "run",
      "--quiet",
      "-p",
      "t3-persistence",
      "--example",
      "migrate",
      "--",
      path,
      ...(head === undefined ? [] : [String(head)]),
    ],
    {
      cwd: fileURLToPath(new URL("../../../", import.meta.url)),
      encoding: "utf8",
      env: {
        ...process.env,
        CARGO_TARGET_DIR: fileURLToPath(
          new URL("../../../target/agents/t3-persistence", import.meta.url),
        ),
      },
    },
  );
  if (result.status !== 0) throw new Error(result.stderr || String(result.error));
  process.stdout.write(result.stdout);
};
const withDatabase = <A, E>(filename: string, body: () => EffectProgram<A, E, SqlService>) =>
  Effect.runPromise(body().pipe(Effect.provide(NodeSqlite.layer({ filename })), Effect.scoped));
const schema = (database: DatabaseSync) =>
  database
    .prepare(`SELECT type, name, tbl_name, sql FROM sqlite_schema
  WHERE sql IS NOT NULL AND name NOT LIKE 'sqlite_%'
  AND name NOT IN (SELECT name FROM pragma_table_list WHERE type = 'shadow') ORDER BY type, name`)
    .all();
const verifySchema = async (path: string, exported = false) => {
  const expected = new DatabaseSync(":memory:");
  const actual = new DatabaseSync(path, { readOnly: true });
  try {
    expected.exec(
      await FS.readFile(new URL("../../../fixtures/database/schema.sql", import.meta.url), "utf8"),
    );
    const actualSchema = schema(actual);
    if (exported) {
      // The TypeScript fixture exporter trims each object's outer whitespace.
      for (const object of actualSchema) object.sql = String(object.sql).trim();
    }
    Assert.deepEqual(
      actualSchema,
      schema(expected),
      "stored SQL must match TypeScript byte for byte per object",
    );
  } finally {
    actual.close();
    expected.close();
  }
};
const verifyClosedWal = async (path: string) => {
  const wal = await FS.stat(`${path}-wal`).catch((error: NodeJS.ErrnoException) => {
    if (error.code === "ENOENT") return undefined;
    throw error;
  });
  Assert.ok(wal === undefined || wal.size === 0, "Rust close left a nonempty WAL");
};
try {
  const path = Path.join(directory, "typescript.sqlite");
  await withDatabase(path, () =>
    Effect.gen(function* () {
      const sql = yield* SqlClient.SqlClient;
      yield* runMigrations({ toMigrationInclusive: 80 });
      const citation = serializeAssistantCitation({
        version: 1,
        environmentId: EnvironmentId.make("local"),
        threadId: ThreadId.make("thread"),
        messageId: MessageId.make("quoted"),
        text: "Readable citation",
        start: 0,
        end: 17,
        prefix: "",
        suffix: "",
      });
      yield* sql`INSERT INTO orchestration_v2_projection_messages(message_id,thread_id,role,streaming,created_at,updated_at,payload_json) VALUES ('message','thread','user',0,'2026-10-01','2026-10-01',${JSON.stringify({ text: citation })})`;
      yield* sql`INSERT INTO orchestration_v2_projection_turn_items(turn_item_id,thread_id,ordinal,type,status,updated_at,payload_json) VALUES ('item','thread',1,'assistant_message','completed','2026-10-01','{}')`;
    }),
  );
  cargo(path, 84);
  await withDatabase(path, () =>
    Effect.gen(function* () {
      const sql = yield* SqlClient.SqlClient;
      const applied = yield* runMigrations();
      Assert.deepEqual(
        applied.map(([id]) => id),
        [85, 86, 87, 88, 89],
      );
      Assert.equal(
        (yield* sql`SELECT search_text FROM orchestration_v2_projection_messages`)[0].search_text,
        "Readable citation",
      );
      Assert.equal(
        (yield* sql`SELECT item_count FROM orchestration_v2_projection_turn_item_counts`)[0]
          .item_count,
        1,
      );
      console.log(
        "TypeScript 80 -> Rust 84 -> TypeScript 89: schema, citation, counts and ledger passed",
      );
    }),
  );
  await verifySchema(path, true);
  const fresh = Path.join(directory, "rust.sqlite");
  cargo(fresh);
  await verifyClosedWal(fresh);
  await verifySchema(fresh);
  await withDatabase(fresh, () =>
    Effect.gen(function* () {
      Assert.deepEqual(yield* runMigrations(), []);
      console.log("Rust fresh 89 -> TypeScript: accepted with no pending migrations");
    }),
  );
} finally {
  await FS.rm(directory, { recursive: true, force: true });
}
