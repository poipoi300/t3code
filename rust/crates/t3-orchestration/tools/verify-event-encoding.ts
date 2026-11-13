import * as Assert from "node:assert/strict";
import * as FS from "node:fs/promises";
import * as Module from "node:module";
import { pathToFileURL } from "node:url";
import { Arbitrary, Effect, Layer, Schema, SchemaAST } from "../../../tools/export/effect.ts";
import {
  ApplicationProjectEvent,
  CommandId,
  OrchestrationV2DomainEventJson,
  OrchestrationV2TurnItemJson,
} from "../../../../packages/contracts/src/index.ts";
import * as Store from "../../../../apps/server/src/persistence/OrchestrationEventStore.ts";
import * as Sqlite from "../../../../apps/server/src/persistence/Sqlite.ts";
import * as Outbox from "../../../../apps/server/src/orchestration-v2/EffectOutbox.ts";
import * as Receipts from "../../../../apps/server/src/orchestration-v2/CommandReceiptStore.ts";
import * as Projects from "../../../../apps/server/src/orchestration-v2/ProjectStore.ts";
import * as Positions from "../../../../apps/server/src/orchestration-v2/TurnItemPositionStore.ts";
const require = Module.createRequire(
  new URL("../../../../apps/server/package.json", import.meta.url),
);
const SqlClient = await import(pathToFileURL(require.resolve("effect/sql/SqlClient")).href);
const Stream = await import(pathToFileURL(require.resolve("effect/Stream")).href);
const DateTime = await import(pathToFileURL(require.resolve("effect/DateTime")).href);
const Option = await import(pathToFileURL(require.resolve("effect/Option")).href);
const TestClock = await import(pathToFileURL(require.resolve("effect/testing/TestClock")).href);
const Cause = await import(pathToFileURL(require.resolve("effect/Cause")).href);

// Rust to TypeScript: rows written by the Rust store (dumped by the Rust test
// `rows_written_by_rust_for_the_typescript_decoder` with T3_STORE_ROWS_OUT=<file>) are read back
// one by one through the real TypeScript store, and its re-encoding must equal the Rust bytes.
const verifyRustRows = process.argv.indexOf("--verify-rust-rows");
if (verifyRustRows >= 0) {
  const file = process.argv[verifyRustRows + 1];
  if (file === undefined) throw new Error("usage: --verify-rust-rows <rows.jsonl>");
  const rows = (await FS.readFile(file, "utf8"))
    .split("\n")
    .filter((line) => line.length > 0)
    .map((line) => JSON.parse(line));
  if (rows.length === 0) throw new Error(`${file} has no rows`);
  const columns = Object.keys(rows[0]);
  const failures: string[] = [];
  let anchored = 0;
  await Effect.runPromise(
    Effect.gen(function* () {
      const store = yield* Store.OrchestrationEventStore;
      const sql = yield* SqlClient.SqlClient;
      for (const row of rows) {
        yield* sql.unsafe(
          `INSERT INTO orchestration_events (${columns.join(", ")}) VALUES (${columns.map(() => "?").join(", ")})`,
          columns.map((column) => row[column]),
        );
      }
      for (const row of rows) {
        const where = `${row.event_id} (${row.event_type})`;
        const exit = yield* Effect.exit(
          store
            .readAgentEvents({ afterSequence: row.sequence - 1, throughSequence: row.sequence })
            .pipe(Stream.runCollect),
        );
        if (exit._tag === "Failure") {
          failures.push(`${where}: TypeScript cannot read it: ${Cause.pretty(exit.cause).split("\n")[0]}`);
          continue;
        }
        const [stored] = exit.value;
        const encoded: any = Schema.encodeSync(OrchestrationV2DomainEventJson)(stored.event);
        const metadata = Object.fromEntries(
          ["runId", "nodeId", "driver", "providerInstanceId", "rawEventId"]
            .filter((key) => encoded[key] !== undefined)
            .map((key) => [key, encoded[key]]),
        );
        if (encoded.occurredAt !== row.occurred_at) failures.push(`${where}: occurred_at differs`);
        if (JSON.stringify(metadata) !== row.metadata_json) failures.push(`${where}: metadata_json differs`);
        if (row.payload_json.includes('"promptSequence"')) anchored++;
        else if (JSON.stringify(encoded.payload) !== row.payload_json) {
          failures.push(`${where}: payload_json differs from TypeScript's encoding\n  rust: ${row.payload_json.slice(0, 400)}\n  ts:   ${JSON.stringify(encoded.payload).slice(0, 400)}`);
        }
      }
    }).pipe(Effect.provide(Store.layer.pipe(Layer.provideMerge(Sqlite.layerMemory))), Effect.scoped),
  );
  console.log(
    `Rust rows: ${rows.length} read by the TypeScript store; ${rows.length - anchored} compared byte for byte (${anchored} hold an anchored prompt); ${failures.length} failures.`,
  );
  for (const failure of failures.slice(0, 20)) console.log(failure);
  process.exit(failures.length === 0 ? 0 : 1);
}
const input = JSON.parse(
  await FS.readFile(new URL("../tests/fixtures/event-input.json", import.meta.url), "utf8"),
);
const expected = JSON.parse(
  await FS.readFile(new URL("../tests/fixtures/ts-event-rows.json", import.meta.url), "utf8"),
);
const events = input.map(Schema.decodeUnknownSync(OrchestrationV2DomainEventJson));
await Effect.runPromise(
  Effect.gen(function* () {
    const store = yield* Store.OrchestrationEventStore;
    const sql = yield* SqlClient.SqlClient;
    yield* store.appendAgentEvents({ commandId: CommandId.make("command:compat"), events });
    const rows = yield* sql`SELECT * FROM orchestration_events ORDER BY sequence`;
    Assert.deepEqual(
      rows.map((row) => ({ ...row })),
      expected,
    );
    console.log(
      "Real TypeScript store: all columns and exact JSON bytes match the derived 3-row oracle.",
    );
  }).pipe(Effect.provide(Store.layer.pipe(Layer.provideMerge(Sqlite.layerMemory))), Effect.scoped),
);

const fixtureUrl = new URL("../tests/fixtures/event-compatibility.json", import.meta.url);
const refresh = process.argv.includes("--refresh");
const samples = new URL("../../../fixtures/samples/", import.meta.url);
const readSamples = async (name: string) =>
  (await FS.readFile(new URL(name, samples), "utf8"))
    .trim()
    .split("\n")
    .map((line) => JSON.parse(line));
const scalar = <T>(value: T): T =>
  (typeof value === "string"
    ? value.toWellFormed()
    : Array.isArray(value)
      ? value.map(scalar)
      : value !== null && typeof value === "object"
        ? Object.fromEntries(
            Object.entries(value).map(([key, child]) => [key.toWellFormed(), scalar(child)]),
          )
        : value) as T;
const makeInputs = async () => {
  const values = [];
  for (const file of (await FS.readdir(samples)).sort()) {
    if (file.startsWith("OrchestrationV2DomainEventJson.") && file.endsWith(".jsonl")) {
      values.push(...(await readSamples(file)));
    }
  }
  const occurredAt = "2026-09-03T00:00:00.000Z";
  for (const item of await readSamples("OrchestrationV2TurnItemJson.jsonl")) {
    values.push({ type: "turn-item.updated", occurredAt, payload: item });
  }
  for (const member of OrchestrationV2TurnItemJson.ast.types) {
    const items = await Effect.runPromise(
      Arbitrary.sampleEffect(Arbitrary.schema(Schema.make(member)), {
        count: 2,
        size: 0,
        seed: 29747,
        maxDiscards: 100,
      }),
    );
    for (const item of items) {
      const payload = Schema.encodeUnknownSync(OrchestrationV2TurnItemJson)(item);
      values.push(JSON.parse(JSON.stringify({ type: "turn-item.updated", occurredAt, payload })));
    }
  }
  const subagent = values.find((event) => event.type === "subagent.updated");
  const item = values.find(
    (event) => event.type === "turn-item.updated" && event.payload.type === "subagent",
  );
  // Matching IDs intentionally exercise the two independent insertion-ordered hint keys.
  for (const prompt of ["Audit 🦀\n".repeat(200), "Audit 🦀\n".repeat(200), "changed", "changed"]) {
    for (const template of [subagent, item]) {
      const event = structuredClone(template);
      event.payload.id = "anchor:shared-id";
      event.payload.prompt = prompt;
      values.push(event);
    }
  }
  const literals = [
    "-0.0",
    "1.0",
    "1e-7",
    "1e-6",
    "1e20",
    "1e21",
    "1e23",
    "1.0000000000000001e18",
    "9007199254740993",
    "18446744073709551615",
    "5e-324",
    "1.7976931348623157e308",
    "333333333.33333329",
    "2.2250738585072014e-308",
  ];
  const view = new DataView(new ArrayBuffer(8));
  let bits = 0x7433n;
  for (let index = 0; index < 512; index++) {
    bits = BigInt.asUintN(64, bits * 6364136223846793005n + 1442695040888963407n);
    view.setBigUint64(0, bits);
    const number = view.getFloat64(0);
    if (Number.isFinite(number)) literals.push(number.toExponential());
  }
  const numericEvent = structuredClone(
    values.findLast(
      (event) => event.type === "turn-item.updated" && event.payload.type === "dynamic_tool",
    ),
  );
  numericEvent.payload.id = "numeric:tool";
  numericEvent.payload.input = {
    "10": "ten",
    "2": "two",
    "01": "leading",
    "0": "zero",
    "4294967294": "index",
    "4294967295": "ordinary",
    numbers: literals.map((literal) => JSON.parse(literal)),
    nested: { "20": 20, "3": 3 },
    escapes: '\u0000\b\f\n\r\t"\\/ é 🦀\u2028\u2029',
  };
  values.push(numericEvent);
  // Explicit nulls on optional keys are rejected by the store's decoder. Each such candidate is
  // kept as an expected rejection and its null-free copy as an accepted input.
  const rejected: { reason: string; input: unknown }[] = [];
  const decodes = (input: unknown) => {
    try {
      Schema.decodeUnknownSync(OrchestrationV2DomainEventJson)(input);
      return true;
    } catch {
      return false;
    }
  };
  const candidates = values.map((value, index) => {
    const event = scalar(value);
    const raw = { ...event, id: `compat:${index}`, threadId: `compat:thread:${index % 3}` };
    const input = structuredClone(raw);
    for (const key of ["runId", "nodeId", "driver", "providerInstanceId", "rawEventId"]) {
      if (input[key] === null) delete input[key];
    }
    if (input.type === "turn-item.updated") {
      for (const key of ["toolNonExecutionKind", "toolSurface", "toolIcon", "toolSource"]) {
        if (input.payload[key] === null) delete input.payload[key];
      }
      if (input.payload.nativeItemRef?.fingerprint === null)
        delete input.payload.nativeItemRef.fingerprint;
    }
    if (JSON.stringify(input) !== JSON.stringify(raw) && !decodes(raw)) {
      rejected.push({ reason: "explicit null for an optional key", input: raw });
    }
    return input;
  });
  const accepted = candidates.filter((input) => {
    if (decodes(input)) return true;
    rejected.push({ reason: "rejected by the event decoder", input });
    return false;
  });
  const inputs = accepted.map((input) =>
    Schema.encodeSync(OrchestrationV2DomainEventJson)(
      Schema.decodeUnknownSync(OrchestrationV2DomainEventJson)(input),
    ),
  );
  console.log(
    `Oracle inputs: ${inputs.length} accepted, ${rejected.length} rejected by the TypeScript decoder (${rejected.filter((r) => r.reason.startsWith("explicit")).length} only for explicit nulls).`,
  );
  // Two rejections per event and turn-item tag keep the fixture small.
  const sampled = new Map<string, number>();
  const rejectedSample = rejected.filter(({ input }: any) => {
    const key = `${input.type}:${input.payload?.type ?? ""}:${input.payload === undefined}`;
    sampled.set(key, (sampled.get(key) ?? 0) + 1);
    return sampled.get(key)! <= 2;
  });
  Assert.ok(inputs.some((event) => event.payload.id === "numeric:tool"));
  Assert.deepEqual(
    [...new Set(inputs.map((event) => event.type))].sort(),
    [...new Set(candidates.map((event) => event.type))].sort(),
  );
  Assert.equal(
    new Set(
      inputs
        .filter((event) => event.type === "turn-item.updated")
        .map((event) => event.payload.type),
    ).size,
    25,
  );
  return {
    inputs,
    rejected: rejectedSample,
    numbers: literals.map((input) => ({ input, encoded: JSON.stringify(JSON.parse(input)) })),
  };
};
const plainJson = (value: unknown) => JSON.parse(JSON.stringify(value));
const fixture = refresh ? await makeInputs() : JSON.parse(await FS.readFile(fixtureUrl, "utf8"));
for (const { input, encoded } of fixture.numbers) {
  Assert.equal(JSON.stringify(JSON.parse(input)), encoded);
}
for (const { input } of fixture.rejected) {
  Assert.throws(() => Schema.decodeUnknownSync(OrchestrationV2DomainEventJson)(input));
}
const actual = await Effect.runPromise(
  Effect.gen(function* () {
    const store = yield* Store.OrchestrationEventStore;
    const sql = yield* SqlClient.SqlClient;
    const decoded = fixture.inputs.map((input, index) => {
      try {
        return Schema.decodeUnknownSync(OrchestrationV2DomainEventJson)(input);
      } catch (error) {
        throw new Error(`Input ${index} (${input.type}): ${error.message}`);
      }
    });
    // Several append calls must share prompt hints just as independent server commits do.
    for (let offset = 0; offset < decoded.length; offset += 17) {
      yield* store.appendAgentEvents({
        commandId: CommandId.make("command:compat"),
        events: decoded.slice(offset, offset + 17),
      });
    }
    const rows = yield* sql`SELECT * FROM orchestration_events ORDER BY sequence`;
    const replayed = yield* store.readAgentEvents().pipe(Stream.runCollect);
    Assert.deepEqual(
      JSON.parse(JSON.stringify(replayed.map((row) => row.event))),
      JSON.parse(JSON.stringify(decoded)),
    );
    // The Rust read of each row is compared with its input, so the TypeScript read must equal it.
    Assert.deepEqual(
      plainJson(replayed.map((row) => Schema.encodeSync(OrchestrationV2DomainEventJson)(row.event))),
      plainJson(fixture.inputs),
    );
    return rows.map((row) => ({ ...row }));
  }).pipe(Effect.provide(Store.layer.pipe(Layer.provideMerge(Sqlite.layerMemory))), Effect.scoped),
);
if (refresh) {
  await FS.writeFile(fixtureUrl, JSON.stringify({ ...fixture, rows: actual }, null, 2) + "\n");
} else {
  Assert.deepEqual(actual, fixture.rows);
}
console.log(
  `Real TypeScript store: ${actual.length} exact rows; ${new Set(fixture.inputs.map((event) => event.type)).size} event tags; ${new Set(fixture.inputs.filter((event) => event.type === "turn-item.updated").map((event) => event.payload.type)).size} turn-item tags; ${fixture.numbers.length} JavaScript number encodings.`,
);

const projectUrl = new URL("../tests/fixtures/project-event-compatibility.json", import.meta.url);
const at = "2026-09-03T00:00:00.000Z";
const projectBase = {
  sequence: 999,
  aggregateKind: "project",
  aggregateId: "project",
  occurredAt: at,
  commandId: null,
  causationEventId: null,
  correlationId: null,
  metadata: {},
};
const projectInputs: unknown[] = [
  { commandId: null, metadata: {} },
  { commandId: "client:project", metadata: {} },
  { commandId: "provider:project", metadata: {} },
  { commandId: "server:project", metadata: { adapterKey: "codex" } },
  { commandId: "client:provider-turn", metadata: { providerTurnId: "turn" } },
  { commandId: "client:provider-item", metadata: { providerItemId: "item" } },
  { commandId: null, metadata: { adapterKey: "codex" } },
  {
    commandId: "client:metadata",
    metadata: {
      deferredTurn: false,
      providerTurnId: "turn",
      providerItemId: "item",
      adapterKey: "codex",
      requestId: "request",
      ingestedAt: at,
      origin: { surface: "desktop", appVersion: "1.0" },
    },
  },
].map((fields, index) => ({
  ...projectBase,
  eventId: `project:${index}`,
  ...fields,
  causationEventId: index ? "cause" : null,
  correlationId: index ? "correlation" : null,
  type: "project.created",
  payload: {
    projectId: "project",
    title: "Project 🦀",
    workspaceRoot: "C:/temporary/project",
    defaultModelSelection: index ? { instanceId: "codex", model: "model" } : null,
    ...(index === 1
      ? { projectIcon: null }
      : index === 2
        ? { projectIcon: { kind: "emoji", emoji: "🦀" } }
        : {}),
    scripts: [],
    createdAt: at,
    updatedAt: at,
  },
}));
projectInputs.push({
  ...projectBase,
  eventId: "project:meta",
  type: "project.meta-updated",
  payload: {
    projectId: "project",
    defaultModelSelection: null,
    projectIcon: null,
    pinnedAt: null,
    archivedAt: null,
    updatedAt: at,
  },
});
projectInputs.push({
  ...projectBase,
  eventId: "project:deleted",
  type: "project.deleted",
  payload: { projectId: "project", deletedAt: at },
});
const projectFixture = refresh
  ? { inputs: projectInputs.map(Schema.decodeUnknownSync(ApplicationProjectEvent)) }
  : JSON.parse(await FS.readFile(projectUrl, "utf8"));
const projectRows = await Effect.runPromise(
  Effect.gen(function* () {
    const store = yield* Store.OrchestrationEventStore;
    const sql = yield* SqlClient.SqlClient;
    for (const input of projectFixture.inputs) {
      const { sequence: _, ...event } = input;
      yield* store.appendProjectEvent(event);
    }
    return (yield* sql`SELECT * FROM orchestration_events ORDER BY sequence`).map((row) => ({
      ...row,
    }));
  }).pipe(Effect.provide(Store.layer.pipe(Layer.provideMerge(Sqlite.layerMemory))), Effect.scoped),
);
if (refresh)
  await FS.writeFile(
    projectUrl,
    JSON.stringify({ ...projectFixture, rows: projectRows }, null, 2) + "\n",
  );
else Assert.deepEqual(projectRows, projectFixture.rows);
console.log(
  `Real TypeScript store: ${projectRows.length} exact project rows across all three event payloads and actor inference branches.`,
);

// ---------------------------------------------------------------------------------------------
// Store oracle: every table the Rust store layer writes, driven through the real TypeScript
// stores with a fixed clock. `tests/fixtures/store-oracle.json` holds the operations (shared by
// both sides), the TypeScript results and table snapshots; the Rust tests replay the operations.
// ---------------------------------------------------------------------------------------------

const T0 = "2026-09-03T00:00:00.000Z";
const atOffset = (offsetMs: number) => new Date(Date.parse(T0) + offsetMs).toISOString();
const storeLayer = Layer.mergeAll(
  Store.layer,
  Outbox.layer,
  Receipts.layer,
  Projects.layer,
  Positions.layer,
).pipe(Layer.provideMerge(Sqlite.layerMemory));
const runStores = <A>(program: any): Promise<A> =>
  Effect.runPromise(
    program.pipe(Effect.provide(storeLayer), Effect.provide(TestClock.layer()), Effect.scoped),
  );
const setNow = (iso: string) => TestClock.setTime(Date.parse(iso));
const snapshot = (table: string) =>
  Effect.gen(function* () {
    const sql = yield* SqlClient.SqlClient;
    return (yield* sql.unsafe(`SELECT * FROM ${table} ORDER BY rowid`)).map((row: any) => ({
      ...row,
    }));
  });
const plain = (value: unknown) => JSON.parse(JSON.stringify(value));

// Effect's DateTimeUtcFromString round trip, the canonical form of every stored event date.
// Forms V8 reads as local time (`2026-1-03`, `002026-09-03`) are left out: their result depends
// on the machine's time zone.
const dateInputs = [
  "2026-09-03T00:00:00.000Z", "2026-09-03T00:00:00Z", "2026-09-03T00:00Z",
  "2026-09-03T00:00:00.1Z", "2026-09-03T00:00:00.123456789Z", "2026-09-03T00:00:00.9999Z",
  "2026-09-03T01:00:00+02:00", "2026-09-03T01:00:00-0230", "2026-09-03T00:00:00.000-00:00",
  "2026-09-03T12:30:15.5+05:30", "2026-09-03T00:00:00+23:59", "2026-09-03T00:00:00+24:00",
  "2026-09-03", "2026-09", "2026", "2026Z", "2026-09-03Z", "2026-09-03T00:00:00",
  "2026-09-03T00:00", "2026-09-03T00:00:00.5", "2026-09-03 00:00:00Z", "2026-09-03 00:00Z",
  "2026-09-03t00:00:00z", "2026-09-03T00:00:00.000z", "+010000-01-01T00:00:00.000Z",
  "+010000", "-000001-01-01T00:00:00Z", "-000001-12-31", "0000-01-01T00:00:00Z",
  "+002026-09-03T00:00:00Z", "2026-09-03T24:00:00Z", "2026-09-03T24:00Z",
  "2026-09-03T24:00:01Z", "2026-09-03T24:00:00.001Z", "2026-02-30T00:00:00Z", "2026-04-31",
  "2026-02-29", "2024-02-29", "2026-01-32", "2026-00-01", "2026-01-00", "2026-13-01T00:00:00Z",
  "2026-09-03T23:59:60Z", "2026-09-03T00:60:00Z", "2026-09-03T0:00:00Z", "2026-09-03T1:2Z",
  "2026-09-03T00Z", "2026-09-03T", "T00:00Z", "20260903T000000Z", "2026-09-03T00:00:00+00",
  "2026-09-03T00:00:00+0", "2026-09-03T00:00:00+05:3", "2026-09-03T00:00:00.000+01",
  "2026-09-03T00:00:00.Z", "2026-09-03T00:00:00,5Z", "2026-09-03T00:00:00.000 Z",
  "2026-09-03T00:00:00Z ", "  2026-09-03T00:00:00Z", "2026-09-03T00:00:00GMT",
  "2026-09-03T00:00:00.000Z[UTC]", "2026-09-03T00:00:00.000ZZ", "-000000-01-01T00:00:00Z",
  "+275760-09-13T00:00:00.000Z", "+275760-09-13T00:00:00.001Z", "275760-09-13T00:00:00Z",
  "-271821-04-20T00:00:00.000Z", "-271821-04-19T23:59:59.999Z", "1970-01-01T00:00:00.000-00:01",
  "2026-09-03T23:59:59.9999999999Z", "Thu, 03 Sep 2026 00:00:00 GMT", "2026-9-3T00:00:00Z",
  "2026-09-3", "not a date", "",
];
const dates = dateInputs.map((input) => {
  try {
    const decoded = Schema.decodeUnknownSync(Schema.DateTimeUtcFromString)(input);
    return { input, canonical: Schema.encodeSync(Schema.DateTimeUtcFromString)(decoded) };
  } catch {
    return { input, canonical: null };
  }
});

// Raw event inputs that the TypeScript store re-encodes: other date spellings, other key order,
// full metadata and a null command id. Each is decoded and appended by the real store.
const reverseKeys = (value: any): any =>
  Array.isArray(value)
    ? value.map(reverseKeys)
    : value !== null && typeof value === "object"
      ? Object.fromEntries(Object.entries(value).reverse().map(([k, v]) => [k, reverseKeys(v)]))
      : value;
const canonicalDate = /^\d{4}-\d\d-\d\dT\d\d:\d\d:\d\d\.\d{3}Z$/;
const respellDates = (value: any, spell: (date: string) => string): any =>
  typeof value === "string" && canonicalDate.test(value)
    ? spell(value)
    : Array.isArray(value)
      ? value.map((child) => respellDates(child, spell))
      : value !== null && typeof value === "object"
        ? Object.fromEntries(Object.entries(value).map(([k, v]) => [k, respellDates(v, spell)]))
        : value;
const templates = [
  "thread.created",
  "provider-session.detached",
  "subagent.updated",
  "plan.updated",
  "turn-item.updated:user_message",
  "turn-item.updated:dynamic_tool",
  "turn-item.updated:subagent",
].map((tag) => {
  const [type, item] = tag.split(":");
  const found = fixture.inputs.find(
    (event: any) => event.type === type && (item === undefined || event.payload.type === item),
  );
  Assert.ok(found, `no template for ${tag}`);
  return found;
});
const occurredAtSpellings = [
  "2026-09-03T00:00:00Z",
  "2026-09-03T02:00:00.123456+02:00",
  "2026-09-03",
  "2026-09-03T00:00:00",
  "+010000-01-01T00:00:00Z",
  "2026-02-30T00:00:00Z",
  "2026-09-03 00:00Z",
];
const rawEvents: { event: any; commandId: string | null }[] = [];
templates.forEach((template: any, t: number) => {
  const base = (variant: string) => ({ ...structuredClone(template), id: `raw:${t}:${variant}` });
  occurredAtSpellings.forEach((occurredAt, i) =>
    rawEvents.push({ event: { ...base(`at${i}`), occurredAt }, commandId: "command:raw" }),
  );
  rawEvents.push({
    event: respellDates(base("dates"), (date) =>
      date.replace(".000Z", "Z").replace(/\.(\d)\d\dZ$/, ".$1Z"),
    ),
    commandId: "command:raw",
  });
  rawEvents.push({ event: reverseKeys(base("order")), commandId: "command:raw" });
  rawEvents.push({
    event: {
      ...base("metadata"),
      runId: "run:raw",
      nodeId: "node:raw",
      driver: "codex",
      providerInstanceId: "codex",
      rawEventId: "raw-event:raw",
    },
    commandId: "command:raw",
  });
  rawEvents.push({ event: base("no-command"), commandId: null });
});
const dynamicTool: any = templates[5];
const userMessage: any = templates[4];
const withPayload = (event: any, payload: any) => ({ ...structuredClone(event), payload });
const rawRejected = [
  { reason: "invalid occurredAt", event: { ...structuredClone(userMessage), occurredAt: "not a date" } },
  { reason: "invalid payload date", event: withPayload(userMessage, { ...userMessage.payload, updatedAt: "yesterday" }) },
  { reason: "explicit null toolSurface", event: withPayload(dynamicTool, { ...dynamicTool.payload, toolSurface: null }) },
  { reason: "explicit null optional metadata", event: { ...structuredClone(userMessage), runId: null } },
  { reason: "compact numeric turn item id", event: withPayload(userMessage, { ...userMessage.payload, id: 7 }) },
  { reason: "unknown event type", event: { ...structuredClone(userMessage), type: "future.event" } },
  { reason: "unknown turn item type", event: withPayload(userMessage, { ...userMessage.payload, type: "future_item" }) },
  {
    reason: "missing required payload key",
    event: withPayload(
      userMessage,
      Object.fromEntries(Object.entries(userMessage.payload).filter(([k]) => k !== "updatedAt")),
    ),
  },
].map(({ reason, event }) => ({ reason, event: { ...event, id: `rejected:${reason}` } }));
for (const { reason, event } of rawRejected) {
  Assert.throws(() => Schema.decodeUnknownSync(OrchestrationV2DomainEventJson)(event), reason);
}
const rawResult = await runStores<any>(
  Effect.gen(function* () {
    const store = yield* Store.OrchestrationEventStore;
    for (const { event, commandId } of rawEvents) {
      yield* store.appendAgentEvents({
        ...(commandId === null ? {} : { commandId: CommandId.make(commandId) }),
        events: [Schema.decodeUnknownSync(OrchestrationV2DomainEventJson)(event)],
      });
    }
    const replayed = yield* store.readAgentEvents().pipe(Stream.runCollect);
    return {
      rows: yield* snapshot("orchestration_events"),
      reads: replayed.map((stored: any) => ({
        sequence: stored.sequence,
        commandId: stored.commandId,
        event: Schema.encodeSync(OrchestrationV2DomainEventJson)(stored.event),
      })),
    };
  }),
);

// Outbox: one operation log, replayed by the Rust test with the same timestamps.
const effectRequests = [
  { type: "provider-runtime.continue", sourceRunId: "run:source" },
  { type: "provider-session.detach", providerSessionId: "session:1", detail: "idle", revokeMcpCredential: true },
  {
    type: "provider-session.apply-model-selection",
    providerSessionId: "session:1",
    providerThreadId: "provider-thread:1",
    modelSelection: { instanceId: "codex", model: "gpt-5.4", options: [{ id: "reasoningEffort", value: "high" }] },
  },
  { type: "provider-turn.start", runId: "run:1" },
  {
    type: "provider-turn.interrupt",
    providerSessionId: "session:1",
    providerThreadId: "provider-thread:1",
    providerTurnId: "turn:1",
    subagent: { id: "node:1", nativeTaskId: "task:1" },
  },
  {
    type: "provider-turn.steer",
    providerSessionId: "session:1",
    providerThreadId: "provider-thread:1",
    providerTurnId: "turn:1",
    messageId: "message:1",
    modelSelection: { instanceId: "claudeAgent", model: "claude" },
    steerFollowUpAfterRunOrdinal: 4,
  },
  {
    type: "provider-turn.restart",
    providerSessionId: "session:1",
    providerThreadId: "provider-thread:1",
    providerTurnId: "turn:1",
    interruptedAttemptId: "attempt:1",
    runId: "run:2",
    sessionTransition: { type: "replace", replacementProviderSessionId: "session:2" },
  },
  {
    type: "runtime-request.respond",
    providerSessionId: "session:1",
    requestId: "request:1",
    decision: "acceptForSession",
    feedback: "ok",
    // Keys already sorted: Rust's ProviderUserInputAnswers is a BTreeMap (see the ignored test).
    answers: { answer: "yes", count: 2 },
  },
  { type: "provider-thread.rollback", restoreFiles: false, providerThreadId: "provider-thread:1", checkpointId: "checkpoint:1", scopeId: "scope:1" },
  { type: "checkpoint.capture", runId: "run:1", scopeId: "scope:1" },
  { type: "scheduled-tasks.cleanup" },
  { type: "terminal.cleanup" },
  { type: "preview.cleanup" },
  { type: "attachment.cleanup", attachmentIds: ["attachment:1", "attachment:2"] },
  { type: "thread-title.generate", kind: { type: "initial", messageId: "message:1" } },
  { type: "delegated-tasks.stop", reason: "stopped" },
  {
    type: "provider-turn.restart",
    providerSessionId: "session:1",
    providerThreadId: "provider-thread:1",
    providerTurnId: "turn:2",
    interruptedAttemptId: "attempt:2",
    runId: "run:3",
    sessionTransition: { type: "detach" },
  },
  { type: "thread-title.generate", kind: { type: "regenerate" } },
  { type: "provider-session.detach", providerSessionId: "session:3" },
];
const pending = effectRequests.map((request, index) => ({
  id: `effect:${String(index).padStart(2, "0")}`,
  commandId: `command:${index % 3}`,
  threadId: `thread:${index % 5}`,
  request,
  ...(index === 3 ? { availableAt: atOffset(86_400_000) } : {}),
}));
const outboxOps: any[] = [
  { op: "enqueue", at: T0, effects: pending },
  { op: "enqueue", at: atOffset(1), effects: [{ ...pending[0], request: { type: "terminal.cleanup" } }] },
  { op: "snapshot" },
  { op: "claim", at: atOffset(1000), workerId: "worker:a" },
  { op: "claim", at: atOffset(1000), workerId: "worker:b", maxRunningTitleGenerations: 0 },
  { op: "claim", at: atOffset(1000), workerId: "worker:c", excludeRestartContinuations: true },
  { op: "claim", at: atOffset(1000), workerId: "worker:d", maxRunningTitleGenerations: 1 },
  { op: "nextClaimableAt" },
  { op: "nextClaimableAt", maxRunningTitleGenerations: 0 },
  { op: "succeed", at: atOffset(2000), claim: 0 },
  { op: "succeed", at: atOffset(2000), claim: 0 },
  { op: "fail", at: atOffset(2000), claim: 1, error: "boom" },
  { op: "retry", at: atOffset(2000), claim: 2, error: "again", availableAt: atOffset(7000) },
  {
    op: "cancel",
    at: atOffset(3000),
    threadId: "thread:3",
    effectTypes: ["provider-turn.start", "attachment.cleanup", "terminal.cleanup"],
    reason: "stop",
  },
  {
    op: "insert",
    sql: "INSERT INTO orchestration_v2_effect_outbox (effect_id, command_id, thread_id, effect_type, payload_json, status, attempt_count, available_at, created_at, updated_at) VALUES ('effect:future', 'command:future', 'thread:future', 'future.effect', '{\"type\":\"future.effect\"}', 'pending', 0, '2026-09-02T00:00:00.000Z', '2026-09-02T00:00:00.000Z', '2026-09-02T00:00:00.000Z')",
  },
  { op: "claimDecodable", at: atOffset(4000), workerId: "worker:e" },
  { op: "claimDecodable", at: atOffset(4000), workerId: "worker:e" },
  { op: "claimDecodable", at: atOffset(8000), workerId: "worker:f" },
  { op: "snapshot" },
  { op: "get", effectIds: pending.map((effect) => effect.id) },
  { op: "listByCommand", commandId: "command:1" },
  { op: "prune", at: atOffset(8 * 86_400_000), cutoff: atOffset(86_400_000) },
  { op: "snapshot" },
];
const lossOps: any[] = [
  ...effectRequests.slice(0, 16).flatMap((request, index) =>
    ["pending", "running", "succeeded", "failed", "cancelled"].map((status) => ({
      op: "enqueue",
      at: T0,
      effects: [
        {
          id: `${index}:${status}`,
          commandId: "command:loss",
          threadId: `${index}:${status}`,
          request,
          availableAt: atOffset(86_400_000),
        },
      ],
    })),
  ),
  {
    op: "insert",
    sql: `UPDATE orchestration_v2_effect_outbox SET status = substr(effect_id, instr(effect_id, ':') + 1), lease_owner = CASE WHEN effect_id LIKE '%:running' THEN 'crashed' ELSE NULL END, attempt_count = CASE WHEN effect_id LIKE '%:running' THEN 1 ELSE 0 END, completed_at = CASE WHEN effect_id LIKE '%:succeeded' OR effect_id LIKE '%:failed' OR effect_id LIKE '%:cancelled' THEN '${T0}' ELSE NULL END`,
  },
  { op: "reconcile", at: atOffset(5000) },
  { op: "snapshot" },
];
const runOutbox = (ops: any[]) =>
  runStores<any[]>(
    Effect.gen(function* () {
      const outbox = yield* Outbox.EffectOutboxV2;
      const sql = yield* SqlClient.SqlClient;
      const claims: any[] = [];
      const results: any[] = [];
      const effect = (value: any) => (value === undefined ? null : plain(value));
      for (const op of ops) {
        if (op.at !== undefined) yield* setNow(op.at);
        switch (op.op) {
          case "enqueue":
            yield* outbox.enqueue(
              op.effects.map((e: any) => ({
                ...e,
                request: Schema.decodeUnknownSync(Outbox.OrchestrationEffectRequestV2)(e.request),
                ...(e.availableAt === undefined
                  ? {}
                  : { availableAt: DateTime.makeUnsafe(e.availableAt) }),
              })),
            );
            results.push(null);
            break;
          case "snapshot":
            results.push(yield* snapshot("orchestration_v2_effect_outbox"));
            break;
          case "insert":
            yield* sql.unsafe(op.sql);
            results.push(null);
            break;
          case "claim": {
            const { op: _, at: __, ...input } = op;
            const claimed = Option.getOrUndefined(yield* outbox.claimNext(input));
            claims.push({ id: claimed?.id, workerId: op.workerId });
            results.push(effect(claimed));
            break;
          }
          case "claimDecodable": {
            let undecodable = 0;
            while (true) {
              const exit = yield* Effect.exit(outbox.claimNext({ workerId: op.workerId }));
              if (exit._tag === "Failure") {
                undecodable++;
                continue;
              }
              results.push({ undecodable, effect: effect(Option.getOrUndefined(exit.value)) });
              break;
            }
            break;
          }
          case "nextClaimableAt": {
            const next = yield* outbox.nextClaimableAt(
              op.maxRunningTitleGenerations === undefined
                ? {}
                : { maxRunningTitleGenerations: op.maxRunningTitleGenerations },
            );
            results.push(Option.isNone(next) ? null : DateTime.formatIso(next.value));
            break;
          }
          case "succeed":
          case "fail":
          case "retry": {
            const claim = claims[op.claim];
            const input = {
              effectId: claim.id,
              workerId: claim.workerId,
              ...(op.error === undefined ? {} : { error: op.error }),
            };
            results.push(
              op.op === "succeed"
                ? yield* outbox.succeed(input)
                : op.op === "fail"
                  ? yield* outbox.fail(input)
                  : yield* outbox.retry({
                      ...input,
                      delayMs: Date.parse(op.availableAt) - Date.parse(op.at),
                    }),
            );
            break;
          }
          case "cancel":
            results.push(
              yield* outbox.cancelUnsettled({
                threadId: op.threadId,
                effectTypes: op.effectTypes,
                reason: op.reason,
              }),
            );
            break;
          case "get":
            results.push(
              yield* Effect.forEach(op.effectIds, (id: string) =>
                outbox.get(id).pipe(Effect.map((found) => effect(Option.getOrUndefined(found)))),
              ),
            );
            break;
          case "listByCommand":
            results.push(plain(yield* outbox.listByCommandId(op.commandId)));
            break;
          case "prune":
            Assert.equal(op.cutoff, new Date(Date.parse(op.at) - 7 * 86_400_000).toISOString());
            results.push(yield* outbox.pruneSettled);
            break;
          case "reconcile":
            results.push(plain(yield* outbox.reconcileAfterProcessLoss));
            break;
          default:
            throw new Error(`unknown outbox op ${op.op}`);
        }
      }
      return results;
    }),
  );
const outboxResults = await runOutbox(outboxOps);
const lossResults = await runOutbox(lossOps);

// Receipts through CommandReceiptStoreV2: thread and project receipts, conflicts and reads.
const receiptOps: any[] = [
  {
    op: "insertIfAbsent",
    receipt: { commandId: "command:thread", threadId: "thread:1", commandType: "message.dispatch", acceptedAt: "2026-09-03T00:00:01.250Z", resultSequence: 7, status: "accepted", error: null },
  },
  {
    op: "insertIfAbsent",
    receipt: { commandId: "command:thread", threadId: "thread:2", commandType: "other", acceptedAt: T0, resultSequence: 9, status: "rejected", error: "duplicate" },
  },
  {
    op: "insertIfAbsent",
    receipt: { commandId: "command:project", projectId: "project:1", commandType: "project.create", acceptedAt: T0, resultSequence: 0, status: "rejected", error: "no 🦀" },
  },
  {
    op: "upsert",
    receipt: { commandId: "command:project", projectId: "project:2", commandType: "project.meta-update", acceptedAt: "+010000-01-01T00:00:00.000Z", resultSequence: 3, status: "accepted", error: null },
  },
  {
    op: "upsert",
    receipt: { commandId: "command:upserted", threadId: "thread:3", commandType: "thread.archive", acceptedAt: atOffset(500), resultSequence: 1, status: "accepted", error: null },
  },
  { op: "get", commandId: "command:thread" },
  { op: "get", commandId: "command:project" },
  { op: "getProject", commandId: "command:project" },
  { op: "getProject", commandId: "command:thread" },
  { op: "get", commandId: "command:missing" },
  { op: "snapshot" },
];
const receiptResults = await runStores<any[]>(
  Effect.gen(function* () {
    const receipts = yield* Receipts.CommandReceiptStoreV2;
    const results: any[] = [];
    const read = (found: any) =>
      Option.isNone(found)
        ? null
        : { ...found.value, acceptedAt: DateTime.formatIso(found.value.acceptedAt) };
    for (const op of receiptOps) {
      const receipt =
        op.receipt === undefined
          ? undefined
          : { ...op.receipt, acceptedAt: DateTime.makeUnsafe(op.receipt.acceptedAt) };
      switch (op.op) {
        case "insertIfAbsent":
          results.push(yield* receipts.insertIfAbsent(receipt));
          break;
        case "upsert":
          yield* receipts.upsert(receipt);
          results.push(null);
          break;
        case "get":
          results.push(read(yield* receipts.getByCommandId(op.commandId)));
          break;
        case "getProject":
          results.push(read(yield* receipts.getProjectByCommandId(op.commandId)));
          break;
        case "snapshot":
          results.push(yield* snapshot("orchestration_command_receipts"));
          break;
      }
    }
    return results;
  }),
);

// Project rows: every payload field shape, set then cleared, a custom title kept across a
// re-create, an update of a missing project, and deletion.
const projectEvent = (index: number, type: string, payload: any) => ({
  sequence: index,
  eventId: `project-row:${index}`,
  aggregateKind: "project",
  aggregateId: payload.projectId,
  occurredAt: T0,
  commandId: null,
  causationEventId: null,
  correlationId: null,
  metadata: {},
  type,
  payload,
});
const projectRowEvents = [
  projectEvent(0, "project.created", {
    projectId: "project:a",
    title: "Alpha 🦀",
    workspaceRoot: "C:/work/alpha",
    defaultModelSelection: { instanceId: "codex", model: "gpt-5.4", options: [{ id: "reasoningEffort", value: "high" }, { id: "fast", value: true }] },
    defaultThreadEnvMode: "worktree",
    faviconPath: "public/favicon.png",
    projectIcon: { kind: "lucide", name: "rocket", color: "violet" },
    scripts: [
      { id: "test", name: "Test", command: "vp test", icon: "test", runOnWorktreeCreate: false },
      { id: "setup", name: "Setup", command: "vp i", icon: "configure", runOnWorktreeCreate: true },
    ],
    createdAt: T0,
    updatedAt: T0,
  }),
  projectEvent(1, "project.created", {
    projectId: "project:b",
    title: "Beta",
    workspaceRoot: "C:/work/beta",
    defaultModelSelection: null,
    projectIcon: { kind: "monogram", text: "B", color: "rose" },
    scripts: [],
    createdAt: atOffset(1),
    updatedAt: atOffset(1),
  }),
  projectEvent(2, "project.created", {
    projectId: "project:c",
    title: "Gamma",
    workspaceRoot: "C:/work/gamma",
    defaultModelSelection: { instanceId: "claudeAgent", model: "claude" },
    projectIcon: { kind: "emoji", emoji: "🧪" },
    scripts: [],
    createdAt: atOffset(2),
    updatedAt: atOffset(2),
  }),
  projectEvent(3, "project.meta-updated", {
    projectId: "project:b",
    title: "Beta renamed",
    pinnedAt: atOffset(10),
    archivedAt: atOffset(11),
    autoPull: true,
    faviconPath: "icon.svg",
    defaultThreadEnvMode: "local",
    updatedAt: atOffset(12),
  }),
  projectEvent(4, "project.meta-updated", {
    projectId: "project:b",
    pinnedAt: null,
    archivedAt: null,
    faviconPath: null,
    projectIcon: null,
    defaultModelSelection: null,
    defaultThreadEnvMode: null,
    scripts: [{ id: "lint", name: "Lint", command: "vp lint", icon: "lint", runOnWorktreeCreate: false }],
    updatedAt: atOffset(13),
  }),
  projectEvent(5, "project.created", {
    projectId: "project:b",
    title: "Beta recreated",
    workspaceRoot: "C:/work/beta2",
    defaultModelSelection: null,
    scripts: [],
    createdAt: atOffset(1),
    updatedAt: atOffset(14),
  }),
  projectEvent(6, "project.meta-updated", { projectId: "project:missing", title: "Nobody", updatedAt: atOffset(15) }),
  projectEvent(7, "project.deleted", { projectId: "project:c", deletedAt: atOffset(16) }),
  projectEvent(8, "project.meta-updated", {
    projectId: "project:a",
    workspaceRoot: "C:/work/alpha2",
    defaultModelSelection: { instanceId: "codex", model: "gpt-5.5" },
    updatedAt: atOffset(17),
  }),
];
const projectResult = await runStores<any>(
  Effect.gen(function* () {
    const projects = yield* Projects.ProjectStoreV2;
    for (const event of projectRowEvents) {
      yield* projects.apply(Schema.decodeUnknownSync(ApplicationProjectEvent)(event));
    }
    return {
      rows: yield* snapshot("projection_projects"),
      shells: plain(yield* projects.listShells()),
      deleted: plain(
        Option.getOrUndefined(yield* projects.get("project:c", { includeDeleted: true })) ?? null,
      ),
      byRoot: plain(
        Option.getOrUndefined(yield* projects.findActiveByWorkspaceRoot("C:/work/beta2")) ?? null,
      ),
    };
  }),
);

// Turn-item positions: reuse, supplied and persisted run ordinals, run-start placement.
const positionSetup =
  "INSERT INTO orchestration_v2_projection_runs (run_id, thread_id, ordinal, provider, status, requested_at, payload_json) VALUES ('run:persisted', 'thread:p', 3, 'codex', 'running', '2026-09-03T00:00:00.000Z', '{}')";
const positionOps = [
  { threadId: "thread:p", turnItemId: "item:1", runId: null },
  { threadId: "thread:p", turnItemId: "item:2", runId: null },
  { threadId: "thread:p", turnItemId: "item:1", runId: "run:other", runOrdinal: 9 },
  { threadId: "thread:p", turnItemId: "item:3", runId: "run:persisted" },
  { threadId: "thread:p", turnItemId: "item:4", runId: "run:persisted", runOrdinal: 5 },
  { threadId: "thread:p", turnItemId: "item:5", runId: "run:supplied", runOrdinal: 2 },
  { threadId: "thread:p", turnItemId: "item:6", runId: "run:supplied", runOrdinal: 2, placement: "run_start" },
  { threadId: "thread:p", turnItemId: "item:7", runId: "run:supplied", runOrdinal: 2, placement: "run_start" },
  { threadId: "thread:p", turnItemId: "item:8", runId: "run:fresh", runOrdinal: 7, placement: "run_start" },
  { threadId: "thread:p", turnItemId: "item:9", runId: "run:fresh", runOrdinal: 7 },
  { threadId: "thread:q", turnItemId: "item:1", runId: null },
];
const positionResult = await runStores<any>(
  Effect.gen(function* () {
    const positions = yield* Positions.TurnItemPositionStoreV2;
    const sql = yield* SqlClient.SqlClient;
    yield* sql.unsafe(positionSetup);
    const ordinals: number[] = [];
    for (const op of positionOps) ordinals.push(yield* positions.allocate(op as any));
    return { ordinals, rows: yield* snapshot("orchestration_v2_turn_item_positions") };
  }),
);

// The persisted event shape the Rust store applies before writing and after reading.
const LEAVES = ["date", "string", "number", "boolean", "null", "any"] as const;
const persistedShape = (root: any) => {
  const nodes: unknown[] = [...LEAVES];
  const ids = new Map<any, number>();
  const leaf = (name: (typeof LEAVES)[number]) => LEAVES.indexOf(name);
  const isDate = (a: any) =>
    a._tag === "Declaration" && a.annotations?.representation?.id === "effect/schema/DateTimeUtc";
  const literals = (a: any): unknown[] | undefined => {
    const e = SchemaAST.toEncoded(a);
    if (e._tag === "Literal") return [e.literal];
    if (e._tag === "Union" && e.types.every((t: any) => t._tag === "Literal"))
      return e.types.map((t: any) => t.literal);
    return undefined;
  };
  // Rust holds the encoded JSON, so each encoding link is followed to its encoded side; a date
  // declaration is the one kept, because its canonical string is what the store must write.
  const visit = (input: any): number => {
    const a = input._tag === "Suspend" ? input.thunk() : input;
    if (isDate(a)) return leaf("date");
    if (a.encoding !== undefined) return visit(a.encoding[a.encoding.length - 1].to);
    switch (a._tag) {
      case "String":
      case "TemplateLiteral":
        return leaf("string");
      case "Number":
        return leaf("number");
      case "Boolean":
        return leaf("boolean");
      case "Null":
        return leaf("null");
      case "Literal":
        return typeof a.literal === "string"
          ? leaf("string")
          : typeof a.literal === "number"
            ? leaf("number")
            : typeof a.literal === "boolean"
              ? leaf("boolean")
              : leaf("any");
      case "Unknown":
      case "Any":
        return leaf("any");
      case "Undefined":
        return -1;
    }
    const known = ids.get(a);
    if (known !== undefined) return known;
    const id = nodes.push(null) - 1;
    ids.set(a, id);
    switch (a._tag) {
      case "Objects":
        nodes[id] = {
          o: a.propertySignatures.map((p: any) => [String(p.name), visit(p.type)]),
          i: a.indexSignatures.length ? visit(a.indexSignatures[0].type) : null,
          q: a.propertySignatures
            .filter((p: any) => p.type.context?.isOptional !== true)
            .map((p: any) => String(p.name)),
          l: Object.fromEntries(
            a.propertySignatures.flatMap((p: any) => {
              const values = literals(p.type);
              return values === undefined ? [] : [[String(p.name), values]];
            }),
          ),
        };
        break;
      case "Arrays":
        nodes[id] = { a: a.elements.map(visit), r: a.rest.length ? visit(a.rest[0]) : null };
        break;
      case "Union": {
        const members = a.types.map((t: any) => (t._tag === "Suspend" ? t.thunk() : t));
        const objects = members.filter((t: any) => t._tag === "Objects");
        // The discriminator is a property whose literal sets are disjoint across members.
        const tag =
          objects.length < 2
            ? undefined
            : objects[0].propertySignatures
                .map((p: any) => p.name)
                .find((name: any) => {
                  const seen = new Set<unknown>();
                  return objects.every((o: any) => {
                    const p = o.propertySignatures.find((q: any) => q.name === name);
                    const values = p === undefined ? undefined : literals(p.type);
                    if (values === undefined || values.some((v) => seen.has(v))) return false;
                    values.forEach((v) => seen.add(v));
                    return true;
                  });
                });
        const cases: Record<string, number> = {};
        const other: number[] = [];
        for (const member of members) {
          if (tag !== undefined && member._tag === "Objects") {
            const p = member.propertySignatures.find((q: any) => q.name === tag);
            for (const literal of literals(p.type)!) cases[String(literal)] = visit(member);
          } else {
            const child = visit(member);
            if (child >= 0) other.push(child);
          }
        }
        nodes[id] = { u: tag ?? null, c: cases, x: other };
        break;
      }
      default:
        throw new Error(`persisted shape: unsupported schema node ${a._tag}`);
    }
    return id;
  };
  return { root: visit(root), leaves: LEAVES, nodes };
};
const shapeText = JSON.stringify(persistedShape(OrchestrationV2DomainEventJson.ast)) + "\n";
const shapeUrl = new URL("../src/stores/persisted-event-shape.json", import.meta.url);

// FNV-1a over the TypeScript sources the oracle depends on (CR stripped), so the Rust test can
// tell when the checked-in output is stale.
const fnv = (text: string) => {
  let hash = 0xcbf29ce484222325n;
  for (const byte of new TextEncoder().encode(text.replaceAll("\r", ""))) {
    hash = BigInt.asUintN(64, (hash ^ BigInt(byte)) * 0x100000001b3n);
  }
  return hash.toString(16).padStart(16, "0");
};
const sourceFiles = [
  "rust/crates/t3-orchestration/tools/verify-event-encoding.ts",
  "apps/server/src/persistence/OrchestrationEventStore.ts",
  "apps/server/src/persistence/OrchestrationCommandReceipts.ts",
  "apps/server/src/orchestration-v2/EffectOutbox.ts",
  "apps/server/src/orchestration-v2/CommandReceiptStore.ts",
  "apps/server/src/orchestration-v2/ProjectStore.ts",
  "apps/server/src/orchestration-v2/TurnItemPositionStore.ts",
  "packages/contracts/src/orchestrationV2.ts",
  "packages/contracts/src/applicationEvent.ts",
  "packages/contracts/src/modelSelection.ts",
  "packages/contracts/src/project.ts",
  "packages/contracts/src/baseSchemas.ts",
];
const repositoryRoot = new URL("../../../../", import.meta.url);
let sources = "";
for (const file of sourceFiles) {
  sources += `${file}\n${await FS.readFile(new URL(file, repositoryRoot), "utf8")}\n`;
}

const storeOracle = {
  sourceFiles,
  sourceHash: fnv(sources),
  shapeHash: fnv(shapeText),
  dates,
  events: { inputs: rawEvents, ...rawResult, rejected: rawRejected },
  outbox: { ops: outboxOps, results: outboxResults },
  processLoss: {
    processBound: [...Outbox.PROCESS_BOUND_EFFECT_TYPES],
    replaySafe: [...Outbox.REPLAY_SAFE_EFFECT_TYPES_AFTER_PROCESS_LOSS],
    ops: lossOps,
    results: lossResults,
  },
  receipts: { ops: receiptOps, results: receiptResults },
  projects: { events: projectRowEvents, ...projectResult },
  positions: { setup: positionSetup, ops: positionOps, ...positionResult },
};
const storeOracleUrl = new URL("../tests/fixtures/store-oracle.json", import.meta.url);
if (refresh) {
  await FS.writeFile(storeOracleUrl, JSON.stringify(storeOracle, null, 2) + "\n");
  await FS.writeFile(shapeUrl, shapeText);
} else {
  Assert.deepEqual(JSON.parse(await FS.readFile(storeOracleUrl, "utf8")), storeOracle);
  Assert.equal((await FS.readFile(shapeUrl, "utf8")).replaceAll("\r", ""), shapeText);
}
console.log(
  `Real TypeScript stores: ${rawEvents.length} raw event rows and ${rawRejected.length} rejections, ${dates.length} date spellings, ${outboxOps.length + lossOps.length} outbox operations over ${effectRequests.length} effect payloads, ${receiptOps.length} receipt operations, ${projectRowEvents.length} project events, ${positionOps.length} position allocations; persisted shape of ${JSON.parse(shapeText).nodes.length} nodes.`,
);
