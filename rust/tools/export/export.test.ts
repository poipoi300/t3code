import * as NodeAssert from "node:assert/strict";
import * as NodeTest from "node:test";
import * as NodeURL from "node:url";
import { Schema, type ContractSchema } from "./effect.ts";
import { discover, exportSchemas, schemaRef } from "./contracts.ts";
import { assertSamplesDecode, samplesFor } from "./samples.ts";

NodeTest.test(
  "sample self-check reports every rejection across files using the real codecs",
  () => {
    const schemas = new Map<string, ContractSchema>([
      ["Name", Schema.Trim.check(Schema.isNonEmpty())],
      ["Count", Schema.Int],
    ]);
    NodeAssert.throws(
      () =>
        assertSamplesDecode(schemas, [
          { schema: "Name", file: "samples/Name.jsonl", values: ["valid", "", " "] },
          { schema: "Count", file: "non-scalar/Count.jsonl", values: [0, 0.5] },
        ]),
      (error: unknown) => {
        NodeAssert.ok(error instanceof Error);
        NodeAssert.match(error.message, /Rejected generated samples \(3\)/);
        for (const location of [
          "samples/Name.jsonl:2",
          "samples/Name.jsonl:3",
          "non-scalar/Count.jsonl:2",
        ])
          NodeAssert.ok(error.message.includes(location));
        return true;
      },
    );
    NodeAssert.doesNotThrow(() =>
      assertSamplesDecode(schemas, [
        { schema: "Name", file: "samples/Name.jsonl", values: ["valid"] },
        { schema: "Count", file: "samples/Count.jsonl", values: [0, 1] },
      ]),
    );
  },
);

NodeTest.test(
  "synthetic variants retain the parent codec's refinements and transformations",
  async () => {
    const { schemas, variants } = await discover(
      NodeURL.fileURLToPath(new URL("../../../", import.meta.url)),
    );
    for (const parent of [
      "ProjectMutation",
      "ApplicationProjectEvent",
      "OrchestrationV2Command",
      "OrchestrationV2DomainEventJson",
    ]) {
      const schema = schemas.get(parent);
      NodeAssert.ok(schema);
      const decode = Schema.decodeUnknownSync(Schema.toCodecJson(schema) as never);
      const entries = variants[parent];
      NodeAssert.ok(entries);
      for (const { tag, discriminator } of entries.slice(0, 2)) {
        const name = `${parent}.${tag}`;
        const member = schemas.get(name);
        NodeAssert.ok(member);
        const samples = await samplesFor(member);
        NodeAssert.ok(samples.values.length, name);
        for (const value of [...samples.values, ...samples.nonScalar]) {
          NodeAssert.doesNotThrow(() => decode(structuredClone(value)), name);
          NodeAssert.equal(Reflect.get(value as object, discriminator), tag);
        }
      }
    }
  },
);

NodeTest.test("exports encoded aliases and shared references, and records opaque codecs", () => {
  const id = Schema.String.pipe(Schema.brand("Id"));
  const schemas = new Map<string, ContractSchema>([
    ["Id", id],
    ["Alias", id],
    ["Record", Schema.Struct({ id, date: Schema.DateTimeUtc })],
    ["Opaque", Schema.declare((value: unknown): value is object => typeof value === "object")],
    [
      "ThrowingCodec",
      Schema.declare((value: unknown): value is string => typeof value === "string", {
        toCodecJson: () => {
          throw new Error("custom codec cannot export JSON");
        },
      }),
    ],
  ]);
  const { document, unsupported } = exportSchemas(schemas);
  NodeAssert.ok(document.$defs.Id);
  NodeAssert.ok(document.$defs.Alias);
  NodeAssert.match(JSON.stringify(document.$defs.Record), /\$ref/);
  NodeAssert.ok(unsupported.some((entry) => entry.schema === "Opaque"));
  NodeAssert.ok(
    unsupported.some(
      (entry) =>
        entry.schema === "ThrowingCodec" && entry.reason === "custom codec cannot export JSON",
    ),
  );
  NodeAssert.ok(document.$defs.ThrowingCodec);
  NodeAssert.equal(schemaRef("rpc.session/new~payload").$ref, "#/$defs/rpc.session~1new~0payload");
});

NodeTest.test(
  "samples preserve optional, null, dates, empty arrays, integers, and every tag",
  async () => {
    const item = Schema.Struct({
      optional: Schema.optionalKey(Schema.NullOr(Schema.String)),
      date: Schema.DateTimeUtc,
      count: Schema.Int,
      items: Schema.Array(Schema.String),
    });
    const schema = Schema.Union([
      Schema.Struct({ type: Schema.Literal("first"), ...item.fields }),
      Schema.Struct({ type: Schema.Literals(["second", "third"]), ...item.fields }),
    ]);
    const first = await samplesFor(schema);
    await samplesFor(Schema.Array(item));
    const second = await samplesFor(schema);
    NodeAssert.deepEqual(first, second);
    const values = first.values as {
      type: string;
      optional?: string | null;
      date: string;
      count: number;
      items: string[];
    }[];
    for (const tag of ["first", "second", "third"])
      NodeAssert.ok(
        values.some((value) => value.type === tag),
        tag,
      );
    NodeAssert.ok(values.some((value) => !("optional" in value)));
    NodeAssert.ok(values.some((value) => value.optional === null));
    NodeAssert.ok(values.some((value) => value.items.length === 0));
    NodeAssert.ok(values.some((value) => value.count === Number.MAX_SAFE_INTEGER));
    NodeAssert.ok(values.every((value) => Number.isFinite(Date.parse(value.date))));
  },
);
