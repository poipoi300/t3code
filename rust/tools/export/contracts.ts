// @effect-diagnostics nodeBuiltinImport:off - exports build artifacts without an application runtime.
import * as NodeFSP from "node:fs/promises";
import * as NodeURL from "node:url";
import * as NodePath from "node:path";
import { Schema, SchemaAST, RpcSchema, version, type ContractSchema, type Ast } from "./effect.ts";

export const reason = (error: unknown) =>
  error instanceof Error ? error.message : (JSON.stringify(error) ?? String(error));
export const json = (value: unknown) => JSON.stringify(value, null, 2) + "\n";
export const order = (a: string, b: string) => (a < b ? -1 : a > b ? 1 : 0);
export const schemaRef = (name: string) => ({
  $ref: `#/$defs/${name.replaceAll("~", "~0").replaceAll("/", "~1")}`,
});

export async function discover(root: string) {
  const schemas = new Map<string, ContractSchema>();
  const bySchema = new Map<ContractSchema, string>();
  const exports: Record<string, string> = {};
  const groups = new Map<string, { requests: ReadonlyMap<string, RpcDefinition> }>();
  const add = (name: string, schema: ContractSchema, preserveName = false) => {
    const known = preserveName ? undefined : bySchema.get(schema);
    if (known) return known;
    if (schemas.get(name) === schema) return name;
    if (schemas.has(name)) throw new Error(`Duplicate schema name ${name}`);
    schemas.set(name, schema);
    if (!bySchema.has(schema)) bySchema.set(schema, name);
    return name;
  };
  async function visit(directory: string) {
    for (const entry of (await NodeFSP.readdir(directory, { withFileTypes: true })).sort((a, b) =>
      order(a.name, b.name),
    )) {
      if (entry.isDirectory()) {
        await visit(NodePath.join(directory, entry.name));
        continue;
      }
      if (!entry.name.endsWith(".ts") || /\.(test|d)\.ts$/.test(entry.name)) continue;
      const path = NodePath.join(directory, entry.name);
      const module = (await import(NodeURL.pathToFileURL(path).href)) as Record<string, unknown>;
      for (const [name, value] of Object.entries(module).sort(([a], [b]) => order(a, b))) {
        if (Schema.isSchema(value)) {
          exports[
            `${path.slice(NodePath.join(root, "packages/contracts/src").length + 1).replaceAll("\\", "/")}#${name}`
          ] = add(name, value, true);
        } else if (
          ((typeof value === "object" && value !== null) || typeof value === "function") &&
          "requests" in value &&
          value.requests instanceof Map
        ) {
          groups.set(name, value as { requests: ReadonlyMap<string, RpcDefinition> });
        }
      }
    }
  }
  await visit(NodePath.join(root, "packages/contracts/src"));
  const acp = await import("../../../packages/effect-acp/src/rpc.ts");
  for (const [name, value] of Object.entries(acp)) {
    if (
      ((typeof value === "object" && value !== null) || typeof value === "function") &&
      "requests" in value &&
      value.requests instanceof Map
    )
      groups.set(name, value as { requests: ReadonlyMap<string, RpcDefinition> });
  }
  const { CLIENT_GUARDED_RPC_SCOPES } =
    await import("../../../packages/contracts/src/clientRpcPermissions.ts");
  const scopes: Readonly<Record<string, unknown>> = CLIENT_GUARDED_RPC_SCOPES;
  const methods = new Map<string, object>();
  for (const [group, definition] of groups) {
    for (const [tag, rpc] of definition.requests) {
      const streamSchema = RpcSchema.isStreamSchema(rpc.successSchema)
        ? rpc.successSchema
        : undefined;
      const success = streamSchema?.success ?? rpc.successSchema;
      const declaredError = streamSchema?.error ?? rpc.errorSchema;
      const error = Schema.Union([
        declaredError,
        ...[...rpc.middlewares].map((middleware) => middleware.error),
      ]);
      const ref = (part: string, schema: ContractSchema) => ({
        ...schemaRef(add(`rpc.${group}.${tag}.${part}`, schema)),
      });
      methods.set(`${group}:${tag}`, {
        tag,
        group,
        payload: ref("payload", rpc.payloadSchema),
        success: ref("success", success),
        error: ref("error", error),
        streams: streamSchema !== undefined,
        requiredScope: group === "WsRpcGroup" ? (scopes[tag] ?? null) : null,
      });
    }
  }
  const unions = [
    "OrchestrationV2Command",
    "OrchestrationV2DomainEvent",
    "OrchestrationV2DomainEventJson",
    "ProjectMutation",
    "ApplicationProjectEvent",
  ];
  const variants: Record<
    string,
    { tag: string; discriminator: string; payload: { $ref: string } }[]
  > = {};
  for (const name of unions) {
    const schema = schemas.get(name);
    if (!schema) throw new Error(`Missing required union ${name}`);
    const members = schema.ast;
    if (members._tag !== "Union") throw new Error(`${name} is no longer a union`);
    variants[name] = members.types.flatMap((ast) => {
      if (ast._tag !== "Objects") throw new Error(`Non-object variant in ${name}`);
      const discriminator = ast.propertySignatures.find((field) =>
        ["type", "_tag", "kind"].includes(String(field.name)),
      );
      if (!discriminator) throw new Error(`Untagged variant in ${name}`);
      const tags =
        discriminator.type._tag === "Union" ? discriminator.type.types : [discriminator.type];
      return tags.map((literal) => {
        if (literal._tag !== "Literal") throw new Error(`Non-literal tag in ${name}`);
        const tag = String(literal.literal);
        const variant = Schema.make<ContractSchema>(
          new SchemaAST.Objects(
            ast.propertySignatures.map((field) =>
              field === discriminator
                ? new SchemaAST.PropertySignature(field.name, literal)
                : field,
            ),
            ast.indexSignatures,
            ast.annotations,
            ast.checks,
            ast.encoding,
            ast.context,
            ast.encodingChecks,
          ),
        );
        return {
          tag,
          discriminator: String(discriminator.name),
          payload: schemaRef(add(`${name}.${tag}`, variant)),
        };
      });
    });
  }
  return { schemas, exports, methods: [...methods.values()], variants };
}

interface RpcDefinition {
  payloadSchema: ContractSchema;
  successSchema: ContractSchema;
  errorSchema: ContractSchema;
  middlewares: ReadonlySet<{ error: ContractSchema }>;
}

export function exportSchemas(schemas: Map<string, ContractSchema>) {
  const names = new Map<Ast, string>();
  const unsupported: { schema: string; stage: string; reason: string }[] = [];
  const supported = new Map<string, ContractSchema>();
  const rejected: Record<string, unknown> = {};
  for (const [name, schema] of schemas) {
    try {
      const encoded = Schema.toEncoded(Schema.toCodecJson(schema)).ast;
      if (!names.has(encoded)) names.set(encoded, name);
      const seen = new Set<Ast>();
      const visit = (ast: Ast, path: string) => {
        if (seen.has(ast)) return;
        seen.add(ast);
        if (ast._tag === "Declaration")
          unsupported.push({
            schema: name,
            stage: "json-schema",
            reason: `${path}: opaque declaration has no structural JSON codec; Effect emits an unconstrained schema`,
          });
        for (const check of ast.checks ?? []) {
          if (check._tag === "Filter" && !check.annotations?.toJsonSchema)
            unsupported.push({
              schema: name,
              stage: "json-schema",
              reason: `${path}: custom check has no JSON Schema representation (${check.annotations?.expected ?? "unnamed filter"})`,
            });
        }
        if (ast._tag === "Objects") {
          for (const field of ast.propertySignatures)
            visit(field.type, `${path}.${String(field.name)}`);
          for (const index of ast.indexSignatures) {
            visit(index.parameter, `${path}.[key]`);
            visit(index.type, `${path}.[value]`);
          }
        } else if (ast._tag === "Union")
          ast.types.forEach((member, index) => visit(member, `${path}.union[${index}]`));
        else if (ast._tag === "Arrays") {
          ast.elements.forEach((element, index) => visit(element, `${path}[${index}]`));
          ast.rest.forEach((element) => visit(element, `${path}[]`));
        } else if (ast._tag === "Suspend") visit(ast.thunk(), path);
      };
      visit(encoded, "$");
      Schema.toJsonSchemaDocument(schema);
      supported.set(name, schema);
    } catch (error) {
      unsupported.push({ schema: name, stage: "json-schema", reason: reason(error) });
      rejected[name] = { "x-unsupported": reason(error) };
    }
  }
  const doc = Schema.toJsonSchemaDocument(Schema.Struct(Object.fromEntries(supported)), {
    referencePolicy: ({ ast, identifier }) => {
      const name = names.get(ast);
      return name ? `types.${name}` : identifier ? `internal.${identifier}` : undefined;
    },
  });
  const properties = doc.schema.properties;
  if (typeof properties !== "object" || properties === null || Array.isArray(properties))
    throw new Error("Effect did not export the named schema registry as an object");
  const definitions: Record<string, unknown> = { ...doc.definitions, ...properties, ...rejected };
  return {
    document: {
      $schema: "https://json-schema.org/draft/2020-12/schema",
      "x-effect-version": version,
      $defs: definitions,
    },
    unsupported,
  };
}
