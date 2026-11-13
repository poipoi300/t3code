import { Arbitrary, Effect, Schema, type Ast, type ContractSchema } from "./effect.ts";
import { reason } from "./contracts.ts";

const seed = 0x7433;
const interleave = (groups: unknown[][], limit: number) => {
  const values: unknown[] = [];
  for (
    let index = 0;
    groups.some((group) => index < group.length) && values.length < limit;
    index++
  ) {
    for (const group of groups)
      if (index < group.length && values.length < limit) values.push(group[index]);
  }
  return values;
};
const jsonValue = (value: unknown): unknown => JSON.parse(JSON.stringify(value));
const scalar = (value: unknown): boolean =>
  typeof value === "string"
    ? value.isWellFormed()
    : Array.isArray(value)
      ? value.every(scalar)
      : typeof value === "object" && value !== null
        ? Object.entries(value).every(([key, child]) => key.isWellFormed() && scalar(child))
        : true;
const toScalar = (value: unknown): unknown =>
  typeof value === "string"
    ? value.toWellFormed()
    : Array.isArray(value)
      ? value.map(toScalar)
      : typeof value === "object" && value !== null
        ? Object.fromEntries(
            Object.entries(value).map(([key, value]) => [key.toWellFormed(), toScalar(value)]),
          )
        : value;

export async function samplesFor(schema: ContractSchema) {
  const cache = new Map<Ast, Promise<unknown[]>>();
  const codec = Schema.toCodecJson(schema);
  const synchronous =
    codec as unknown as import("../../../apps/server/node_modules/effect/dist/Schema.js").Codec<
      unknown,
      unknown
    >;
  const encode = Schema.encodeUnknownSync(synchronous);
  const decode = Schema.decodeUnknownSync(synchronous);
  const samples = new Map<string, unknown>();
  const nonScalar = new Map<string, unknown>();
  const failures = new Set<string>();
  const add = (value: unknown) => {
    try {
      // A sample is what TypeScript emits after its own decode, which can
      // normalise (decoding defaults, derived flags), so Rust must round-trip it.
      // Compact id-table envelopes stay as encoded: decode expands the table and
      // only the server's encode (with `idTable: true`) builds it again.
      const encoded = jsonValue(encode(value));
      const compact =
        typeof encoded === "object" && encoded !== null && Array.isArray(Reflect.get(encoded, "ids"));
      let wire = compact ? encoded : jsonValue(encode(decode(structuredClone(encoded))));
      decode(structuredClone(wire));
      if (!scalar(wire)) {
        nonScalar.set(JSON.stringify(wire), wire);
        wire = jsonValue(encode(decode(toScalar(wire))));
        if (!scalar(wire)) return false;
      }
      samples.set(JSON.stringify(wire), wire);
      return true;
    } catch {
      return false;
    }
  };
  for (const size of [0, 4]) {
    try {
      const values = await Effect.runPromise(
        Arbitrary.sampleEffect(Arbitrary.schema(schema), {
          count: 4,
          size,
          seed,
          maxDiscards: 100,
        }),
      );
      for (const value of values) add(value);
    } catch (error) {
      failures.add(`arbitrary: ${reason(error)}`);
    }
  }
  const encoded = Schema.toEncoded(codec);
  const witnesses = (ast: Ast, depth = 0): Promise<unknown[]> => {
    const known = cache.get(ast);
    if (known) return known;
    const task = (async () => {
      const node = Schema.make(ast);
      const valid = Schema.is(node);
      let values: unknown[] = [];
      try {
        values = [
          ...(await Effect.runPromise(
            Arbitrary.sampleEffect(Arbitrary.schema(node), {
              count: 1,
              size: 0,
              seed,
              maxDiscards: 100,
            }),
          )),
        ];
      } catch {
        /* Structural witnesses below also handle arbitrary derivation gaps. */
      }
      switch (ast._tag) {
        case "Union":
          values.push(
            ...interleave(
              await Promise.all(ast.types.map((member) => witnesses(member, depth + 1))),
              Math.max(64, ast.types.length),
            ),
          );
          break;
        case "Objects": {
          const fields = await Promise.all(
            ast.propertySignatures.map(async (field) => ({
              field,
              values: await witnesses(field.type, depth + 1),
            })),
          );
          const full = Object.fromEntries(
            fields
              .filter(({ values }) => values.length > 0)
              .map(({ field, values }) => [field.name, values[0]]),
          );
          values.push(full);
          const required = { ...full };
          for (const { field } of fields)
            if (field.type.context?.isOptional) delete required[String(field.name)];
          values.push(required);
          const variations: unknown[][] = [];
          for (const { field, values: children } of fields) {
            const omitted = { ...full };
            delete omitted[String(field.name)];
            variations.push([
              omitted,
              ...children.map((child) => ({ ...full, [field.name]: child })),
            ]);
          }
          values.push(...interleave(variations, 32));
          break;
        }
        case "Arrays":
          values.push([]);
          if (ast.elements.length === 0 && ast.rest[0]) {
            const children = await witnesses(ast.rest[0], depth + 1);
            values.push(children);
            for (const child of children.slice(0, 16)) values.push([child]);
          }
          break;
        case "String":
          values.push(
            "",
            "a",
            "é",
            "image/png",
            "favicon.png",
            "https://example.test",
            "2024-02-29T12:34:56.789Z",
            "1970-01-01T00:00:00.000Z",
          );
          break;
        case "Number":
          values.push(0, -1, 1, Number.MAX_SAFE_INTEGER, Number.MIN_SAFE_INTEGER);
          break;
        case "Boolean":
          values.push(false, true);
          break;
        case "Literal":
          values.push(ast.literal);
          break;
        case "Null":
          values.push(null);
          break;
      }
      return [
        ...new Map(
          values
            .filter(valid)
            .filter(scalar)
            .map((value) => [JSON.stringify(value), value]),
        ).values(),
      ];
    })();
    cache.set(ast, task);
    return task;
  };
  for (const value of await witnesses(encoded.ast)) {
    try {
      add(decode(structuredClone(value)));
    } catch {
      /* Encoded-side checks cannot represent every decoded-side refinement. */
    }
  }
  try {
    for (const value of await witnesses(Schema.toType(schema).ast)) add(value);
  } catch (error) {
    failures.add(`decoded witnesses: ${reason(error)}`);
  }
  const tags = (ast: Ast): { key: string; value: unknown; ast: Ast }[] => {
    if (ast._tag === "Union") return ast.types.flatMap(tags);
    if (ast._tag !== "Objects") return [];
    const field = ast.propertySignatures.find((field) =>
      ["type", "kind", "_tag"].includes(String(field.name)),
    );
    if (!field) return [];
    const literals = field.type._tag === "Union" ? field.type.types : [field.type];
    return literals.flatMap((literal) =>
      literal._tag === "Literal" ? [{ key: String(field.name), value: literal.literal, ast }] : [],
    );
  };
  const observed = ({ key, value }: { key: string; value: unknown }) =>
    [...samples.values()].some(
      (sample) =>
        typeof sample === "object" &&
        sample !== null &&
        key in sample &&
        Reflect.get(sample, key) === value,
    );
  const variants = tags(encoded.ast);
  for (const variant of variants.filter((variant) => !observed(variant))) {
    for (const wire of samples.values()) {
      if (typeof wire !== "object" || wire === null || Array.isArray(wire)) continue;
      try {
        add(decode({ ...structuredClone(wire), [variant.key]: variant.value }));
      } catch {
        continue;
      }
      if (observed(variant)) break;
    }
    if (!observed(variant) && variant.ast._tag === "Objects") {
      const specific = Schema.Struct(
        Object.fromEntries(
          variant.ast.propertySignatures.map((field) => [
            field.name,
            field.name === variant.key
              ? Schema.Literal(variant.value as string)
              : Schema.make(field.type),
          ]),
        ),
      );
      for (const value of await witnesses(specific.ast)) {
        add(value);
        try {
          add(decode(structuredClone(value)));
        } catch {
          continue;
        }
      }
    }
  }
  if (samples.size === 0 && encoded.ast._tag !== "Never")
    failures.add("No encodable samples; Rust coverage must not count this schema as tested");
  const missing = variants.filter((variant) => !observed(variant));
  return {
    values: [...samples.values()],
    nonScalar: [...nonScalar.values()],
    failures: [...failures].sort(),
    missing: missing.map(({ key, value }) => ({ key, value })),
  };
}

export function assertSamplesDecode(
  schemas: ReadonlyMap<string, ContractSchema>,
  files: Iterable<{ schema: string; file: string; values: readonly unknown[] }>,
) {
  const errors: string[] = [];
  const decoders = new Map<string, (value: unknown) => unknown>();
  for (const { schema: name, file, values } of files) {
    if (values.length === 0) continue;
    let decode = decoders.get(name);
    if (!decode) {
      const schema = schemas.get(name);
      if (!schema) throw new Error(`Missing sample schema ${name}`);
      const codec = Schema.toCodecJson(
        schema,
      ) as unknown as import("../../../apps/server/node_modules/effect/dist/Schema.js").Codec<
        unknown,
        unknown
      >;
      decode = Schema.decodeUnknownSync(codec);
      decoders.set(name, decode);
    }
    for (const [index, value] of values.entries()) {
      try {
        decode(structuredClone(value));
      } catch (error) {
        errors.push(`${file}:${index + 1} (${name}): ${reason(error)}`);
      }
    }
  }
  if (errors.length)
    throw new Error(`Rejected generated samples (${errors.length}):\n${errors.join("\n")}`);
}
