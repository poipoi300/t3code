// @effect-diagnostics nodeBuiltinImport:off globalConsole:off - standalone fixture generator with a machine-readable summary.
import * as NodeFSP from "node:fs/promises";
import * as NodeURL from "node:url";
import * as NodePath from "node:path";
import { discover, exportSchemas, json, order, reason } from "./contracts.ts";
import { assertSamplesDecode, samplesFor } from "./samples.ts";
import { exportDatabase } from "./database.ts";
import { differentialVectors } from "./differential.ts";
import { STORED_BOUNDARIES, storedParent, storedSamples } from "./stored.ts";
import { version, Schema } from "./effect.ts";

const root = NodeURL.fileURLToPath(new URL("../../../", import.meta.url));
const destination = NodePath.join(root, "rust/fixtures");
const check = process.argv.includes("--check");
const { schemas, exports, methods, variants } = await discover(root);
const { document, unsupported } = exportSchemas(schemas);
const files = new Map<string, string>();
const sampleIndex: { schema: string; file: string; count: number }[] = [];
const sampleChecks: { schema: string; file: string; values: readonly unknown[] }[] = [];
const generated = new Map<string, Awaited<ReturnType<typeof samplesFor>>>();
const tagged = new Map<string, Map<string, unknown>>();
const tagKey = (key: string, value: unknown) => `${key}:${JSON.stringify(value)}`;
for (const [name, schema] of [...schemas].sort(([a], [b]) => order(a, b))) {
  const result = await samplesFor(schema).catch((error: unknown) => ({
    values: [],
    nonScalar: [],
    failures: [`sample derivation: ${reason(error)}`],
    missing: [],
  }));
  generated.set(name, result);
  for (const value of result.values) {
    if (typeof value !== "object" || value === null) continue;
    for (const key of ["type", "kind", "_tag"]) {
      if (!(key in value)) continue;
      const id = tagKey(key, Reflect.get(value, key));
      const candidates = tagged.get(id) ?? new Map<string, unknown>();
      candidates.set(JSON.stringify(value), value);
      tagged.set(id, candidates);
    }
  }
}
for (const [name, result] of generated) {
  const { values, nonScalar, failures } = result;
  const schema = schemas.get(name);
  if (!schema) throw new Error(`Missing schema ${name}`);
  const recover = () => {
    const codec = Schema.toCodecJson(
      schema,
    ) as unknown as import("../../../apps/server/node_modules/effect/dist/Schema.js").Codec<
      unknown,
      unknown
    >;
    const encode = Schema.encodeUnknownSync(codec);
    const decode = Schema.decodeUnknownSync(codec);
    const missing = result.missing.filter(({ key, value }) => {
      for (const candidate of tagged.get(tagKey(key, value))?.values() ?? []) {
        try {
          const wire: unknown = JSON.parse(
            JSON.stringify(encode(decode(structuredClone(candidate)))),
          );
          if (typeof wire === "object" && wire !== null && Reflect.get(wire, key) === value) {
            values.push(wire);
            return false;
          }
        } catch {
          continue;
        }
      }
      return true;
    });
    return missing;
  };
  const missing = result.missing.length ? recover() : [];
  if (missing.length)
    failures.push(
      `No emitted witness for tagged variants (the codec may normalize decode-only aliases): ${missing.map(({ key, value }) => `${key}=${String(value)}`).join(", ")}`,
    );
  for (const failure of failures)
    unsupported.push({ schema: name, stage: "samples", reason: failure });
  const file = `samples/${name}.jsonl`;
  sampleChecks.push({ schema: name, file, values });
  sampleIndex.push({ schema: name, file, count: values.length });
  files.set(
    file,
    values.map((value) => JSON.stringify(value)).join("\n") + (values.length ? "\n" : ""),
  );
  if (nonScalar.length) {
    sampleChecks.push({ schema: name, file: `non-scalar/${name}.jsonl`, values: nonScalar });
    unsupported.push({
      schema: name,
      stage: "unicode",
      reason: `${nonScalar.length} wire samples contain lone UTF-16 surrogates; serde_json cannot represent them. Preserved in non-scalar/${name}.jsonl`,
    });
    files.set(
      `non-scalar/${name}.jsonl`,
      nonScalar.map((value) => JSON.stringify(value)).join("\n") + "\n",
    );
  }
}
for (const [parent, members] of Object.entries(variants)) {
  for (const { tag, discriminator } of members) {
    const name = `${parent}.${tag}`;
    const samples = generated.get(name);
    if (!samples) throw new Error(`Missing variant samples ${name}`);
    for (const [directory, values] of [
      ["samples", samples.values],
      ["non-scalar", samples.nonScalar],
    ] as const) {
      const file = `${directory}/${name}.jsonl`;
      sampleChecks.push({ schema: parent, file, values });
      if (
        values.some(
          (value) =>
            typeof value !== "object" ||
            value === null ||
            Reflect.get(value, discriminator) !== tag,
        )
      )
        throw new Error(`Wrong discriminator in ${file}: expected ${discriminator}=${tag}`);
    }
  }
}
assertSamplesDecode(schemas, sampleChecks);
files.set("differential.jsonl", differentialVectors(schemas, generated));
const storedIndex: { schema: string; parent: string; boundary: string; file: string; count: number }[] = [];
for (const [name, result] of [...generated].sort(([a], [b]) => order(a, b))) {
  const parent = storedParent(name);
  if (!parent) continue;
  // Variant samples decode through the parent union, as the store does.
  const schema = schemas.get(parent);
  if (!schema) throw new Error(`Missing stored schema ${parent}`);
  const { values, failures } = storedSamples(name, schema, result.values);
  for (const failure of failures) unsupported.push({ schema: name, stage: "stored", reason: failure });
  const file = `stored/${name}.jsonl`;
  storedIndex.push({ schema: name, parent, boundary: STORED_BOUNDARIES[parent] ?? "", file, count: values.length });
  files.set(file, values.map((value) => JSON.stringify(value)).join("\n") + (values.length ? "\n" : ""));
}
const database = await exportDatabase();
const variantCount = (name: string) => {
  const entries = variants[name];
  if (!entries) throw new Error(`Missing union ${name}`);
  return entries.length;
};
const counts = {
  schemas: schemas.size,
  unsupported: new Set(unsupported.map((entry) => entry.schema)).size,
  unsupportedJsonSchema: new Set(
    unsupported.filter((entry) => entry.stage === "json-schema").map((entry) => entry.schema),
  ).size,
  unsupportedSampling: new Set(
    unsupported.filter((entry) => entry.stage === "samples").map((entry) => entry.schema),
  ).size,
  nonScalarSampleFiles: [...files.keys()].filter((name) => name.startsWith("non-scalar/")).length,
  namedExports: new Set(Object.values(exports)).size,
  rpcMethods: methods.length,
  commands: variantCount("OrchestrationV2Command") + variantCount("ProjectMutation"),
  events: variantCount("OrchestrationV2DomainEvent") + variantCount("ApplicationProjectEvent"),
  sampleFiles: sampleIndex.length,
  samples: sampleIndex.reduce((sum, entry) => sum + entry.count, 0),
  tables: database.tables,
};
files.set("contracts.schema.json", json(document));
files.set("rpc.json", json(methods));
files.set("orchestration.json", json(variants));
files.set(
  "manifest.json",
  json({
    effectVersion: version,
    seed: 0x7433,
    counts,
    exports,
    samples: sampleIndex,
    stored: storedIndex,
    unsupported,
  }),
);




async function existing(directory: string, prefix = ""): Promise<string[]> {
  let entries;
  try {
    entries = await NodeFSP.readdir(directory, { withFileTypes: true });
  } catch (error) {
    if ((error as NodeJS.ErrnoException).code === "ENOENT") return [];
    throw error;
  }
  const paths: string[] = [];
  for (const entry of entries) {
    const name = prefix + entry.name;
    // RPC transcripts belong to the separate rpcTranscripts.ts exporter.
    if (entry.isDirectory() && (name === "rpc-transcripts" || name === "database")) continue;
    paths.push(
      ...(entry.isDirectory()
        ? await existing(NodePath.join(directory, entry.name), name + "/")
        : [name]),
    );
  }
  return paths;
}
const stale: string[] = [];
for (const [name, content] of files) {
  const path = NodePath.join(destination, name);
  if (check) {
    if ((await NodeFSP.readFile(path, "utf8").catch(() => undefined)) !== content) stale.push(name);
  } else {
    await NodeFSP.mkdir(NodePath.join(path, ".."), { recursive: true });
    await NodeFSP.writeFile(path, content);
  }
}
for (const name of await existing(destination)) {
  if (!files.has(name)) {
    if (check) stale.push(name);
    else await NodeFSP.rm(NodePath.join(destination, name));
  }
}
console.log(JSON.stringify(counts));
if (stale.length)
  throw new Error(`Stale Rust fixtures (${stale.length}): ${stale.slice(0, 20).join(", ")}`);
