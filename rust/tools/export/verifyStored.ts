// @effect-diagnostics nodeBuiltinImport:off globalConsole:off - standalone verifier with a machine-readable summary.
// Runs the TypeScript server's real stored-dialect decoder over Rust's output.
//
// From rust/ (any CARGO_TARGET_DIR):
//   T3_CONTRACT_STORED_OUT=<tmp>/stored-rust.jsonl cargo test -p t3-contracts --test golden stored_dialect
// From the repository root:
//   node rust/tools/export/verifyStored.ts <tmp>/stored-rust.jsonl
//
// Every row must decode with the plain codec the store uses
// (OrchestrationEventStore.ts:181, ProjectionStore.ts, cloud/config.ts, ...),
// and equal TypeScript's own re-encoding as JSON. Rows whose bytes differ only
// in key order are counted, not failed: serde writes an internally tagged
// union's tag first, TypeScript in declaration order. Stores that compare bytes
// (t3-orchestration's event encoder) restore TypeScript's order themselves.
// The opposite direction (TypeScript's stored text decoded by Rust) is the
// `stored_dialect` test itself, over fixtures/stored/.
import * as NodeFSP from "node:fs/promises";
import { discover } from "./contracts.ts";
import { Schema } from "./effect.ts";
import * as NodeURL from "node:url";

type Codec = import("../../../apps/server/node_modules/effect/dist/Schema.js").Codec<unknown, unknown>;

const input = process.argv[2];
if (!input) throw new Error("usage: node rust/tools/export/verifyStored.ts <rust-stored.jsonl>");
const root = NodeURL.fileURLToPath(new URL("../../../", import.meta.url));
const { schemas } = await discover(root);
const rows = (await NodeFSP.readFile(input, "utf8"))
  .split("\n")
  .filter((line) => line.length > 0)
  .map((line) => JSON.parse(line) as { schema: string; sample: string; line: number; rust: string });
if (rows.length === 0) throw new Error(`${input} has no rows`);
const codecs = new Map<string, { decode: (u: unknown) => unknown; encode: (u: unknown) => unknown }>();
const failures: string[] = [];
let keyOrderOnly = 0;
const canonical = (value: unknown): unknown =>
  Array.isArray(value)
    ? value.map(canonical)
    : typeof value === "object" && value !== null
      ? Object.fromEntries(
          Object.entries(value)
            .sort(([a], [b]) => (a < b ? -1 : a > b ? 1 : 0))
            .map(([key, inner]) => [key, canonical(inner)]),
        )
      : value;
for (const row of rows) {
  let codec = codecs.get(row.schema);
  if (!codec) {
    const schema = schemas.get(row.schema) as unknown as Codec | undefined;
    if (!schema) throw new Error(`Unknown schema ${row.schema}`);
    codec = { decode: Schema.decodeUnknownSync(schema), encode: Schema.encodeUnknownSync(schema) };
    codecs.set(row.schema, codec);
  }
  const where = `${row.sample}:${row.line}`;
  try {
    const decoded = codec.decode(JSON.parse(row.rust));
    const typescript = JSON.stringify(codec.encode(decoded));
    if (typescript === row.rust) continue;
    const sameValue =
      JSON.stringify(canonical(JSON.parse(typescript))) === JSON.stringify(canonical(JSON.parse(row.rust)));
    if (sameValue) keyOrderOnly += 1;
    else failures.push(`${where}: value differs\n  rust ${row.rust}\n  ts   ${typescript}`);
  } catch (error) {
    failures.push(`${where}: rejected: ${error instanceof Error ? error.message.split("\n")[0] : String(error)}`);
  }
}
console.log(
  JSON.stringify({ rows: rows.length, schemas: codecs.size, keyOrderOnly, failures: failures.length }),
);
if (failures.length) {
  console.error(failures.slice(0, 20).join("\n"));
  const report = process.env.T3_VERIFY_STORED_REPORT;
  if (report) await NodeFSP.writeFile(report, failures.join("\n"));
  process.exitCode = 1;
}
