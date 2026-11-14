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
Assert.ok(process.env.CARGO_TARGET_DIR, "Set CARGO_TARGET_DIR before running the interop check");
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
      env: process.env,
    },
  );
  if (result.status !== 0) throw new Error(result.stderr || String(result.error));
  process.stdout.write(result.stdout);
  return result.stdout;
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
        [85, 86, 87, 88, 89, 90, 91],
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
        "TypeScript 80 -> Rust 84 -> TypeScript 91: schema, citation, counts and ledger passed",
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
      console.log("Rust fresh 91 -> TypeScript: accepted with no pending migrations");
    }),
  );

  for (const partial of [false, true]) {
    const rust = Path.join(directory, `rust-upgrade-${partial}.sqlite`);
    const typescript = Path.join(directory, `typescript-upgrade-${partial}.sqlite`);
    cargo(rust, 89);
    const seed = new DatabaseSync(rust);
    try {
      seed.exec(`
        INSERT INTO auth_pairing_links(id,credential,method,scopes,subject,label,proof_key_thumbprint,created_at,expires_at,revoked_at)
        VALUES ('link','credential','one-time-token','["access:read"]','device','Original label','key','2026-10-01','2027-10-01','2026-10-02');
        INSERT INTO auth_sessions(session_id,subject,scopes,method,issued_at,expires_at,client_label)
        VALUES ('session','device','["access:read"]','access-token','2026-10-01','2027-10-01','Original client');
        UPDATE effect_sql_migrations SET created_at='2026-10-01 01:02:03';
      `);
      if (partial) {
        seed.exec(`
          ALTER TABLE auth_pairing_links ADD COLUMN requested_client_json TEXT;
          ALTER TABLE auth_pairing_links ADD COLUMN connect_denied INTEGER NOT NULL DEFAULT 0;
          ALTER TABLE auth_sessions ADD COLUMN proof_key_thumbprint TEXT;
          CREATE TABLE auth_connect_devices(proof_key_thumbprint TEXT PRIMARY KEY,client_json TEXT,approved_at TEXT NOT NULL);
          INSERT INTO auth_connect_devices VALUES ('key','{"label":"Trusted"}','2026-10-03');
          UPDATE auth_pairing_links SET requested_client_json='{"label":"First request"}',connect_denied=1;
          UPDATE auth_sessions SET proof_key_thumbprint='key';
        `);
      }
    } finally {
      seed.close();
    }
    await FS.copyFile(rust, typescript);
    Assert.match(cargo(rust, 91), /Rust applied 2 migrations through 91/);
    await withDatabase(typescript, () =>
      Effect.gen(function* () {
        Assert.deepEqual(yield* runMigrations(), [
          [90, "AuthConnectDevices"],
          [91, "AuthConnectDeniedRequests"],
        ]);
      }),
    );
    const readState = (filename: string) => {
      const db = new DatabaseSync(filename, { readOnly: true });
      try {
        return {
          schema: schema(db).map((object) => ({ ...object, sql: String(object.sql).trim() })),
          links: db.prepare(`SELECT id,credential,method,scopes,subject,label,proof_key_thumbprint,created_at,expires_at,consumed_at,revoked_at,requires_connect_approval,requested_client_json,connect_denied FROM auth_pairing_links ORDER BY id`).all(),
          sessions: db.prepare(`SELECT * FROM auth_sessions ORDER BY session_id`).all(),
          devices: db.prepare(`SELECT * FROM auth_connect_devices ORDER BY proof_key_thumbprint`).all(),
          ledger: db.prepare(`SELECT migration_id,name FROM effect_sql_migrations ORDER BY migration_id`).all(),
          oldLedger: db.prepare(`SELECT * FROM effect_sql_migrations WHERE migration_id<=89 ORDER BY migration_id`).all(),
        };
      } finally {
        db.close();
      }
    };
    const expected = readState(typescript);
    Assert.deepEqual(readState(rust), expected, `head 89 data/schema parity (partial=${partial})`);
    Assert.equal(expected.links[0].connect_denied, partial ? 1 : 0);
    Assert.equal(expected.links[0].requires_connect_approval, 0);
    Assert.equal(expected.oldLedger.length, 89);
    Assert.ok(expected.oldLedger.every((row) => row.created_at === "2026-10-01 01:02:03"));
    await withDatabase(rust, () =>
      Effect.gen(function* () {
        Assert.deepEqual(yield* runMigrations(), []);
      }),
    );
    Assert.match(cargo(typescript, 91), /Rust applied 0 migrations through 91/);
    Assert.deepEqual(readState(typescript), expected);
    if (!partial) await verifySchema(rust);
    console.log(`Head 89 -> 91 (partial=${partial}): identical schema/data/ledger; both migrators accept the other's database as current`);
  }

  for (const [head, missing] of [[89, "auth_sessions"], [90, "auth_pairing_links"]] as const) {
    const rust = Path.join(directory, `rust-failure-${head}.sqlite`);
    const typescript = Path.join(directory, `typescript-failure-${head}.sqlite`);
    cargo(rust, head);
    const seed = new DatabaseSync(rust);
    let restore: string;
    try {
      restore = seed.prepare(`SELECT sql FROM sqlite_schema WHERE tbl_name=? AND sql IS NOT NULL ORDER BY type DESC`)
        .all(missing).map((row) => `${row.sql};`).join("\n");
      seed.exec(`DROP TABLE ${missing}`);
    } finally {
      seed.close();
    }
    await FS.copyFile(rust, typescript);
    const snapshot = (filename: string) => {
      const db = new DatabaseSync(filename, { readOnly: true });
      try {
        return { schema: schema(db), ledger: db.prepare(`SELECT * FROM effect_sql_migrations ORDER BY migration_id`).all() };
      } finally {
        db.close();
      }
    };
    const before = snapshot(rust);
    Assert.throws(() => cargo(rust, 91), new RegExp(missing));
    await Assert.rejects(withDatabase(typescript, () => runMigrations()), new RegExp(`Migration "${head + 1}_`));
    Assert.deepEqual(snapshot(rust), before, `Rust migration ${head + 1} rollback`);
    Assert.deepEqual(snapshot(typescript), before, `TypeScript migration ${head + 1} rollback`);
    for (const filename of [rust, typescript]) {
      const db = new DatabaseSync(filename);
      try {
        db.exec(restore);
      } finally {
        db.close();
      }
    }
    Assert.match(cargo(rust, 91), new RegExp(`Rust applied ${91 - head} migrations through 91`));
    await withDatabase(typescript, () =>
      Effect.gen(function* () {
        Assert.equal((yield* runMigrations()).length, 91 - head);
      }),
    );
    await verifySchema(rust);
    await verifySchema(typescript, true);
    console.log(`Migration ${head + 1}: both runners roll back schema/ledger on failure and retry successfully`);
  }
} finally {
  await FS.rm(directory, { recursive: true, force: true });
}
