import * as NodeModule from "node:module";
import * as NodeURL from "node:url";

const require = NodeModule.createRequire(
  new URL("../../../apps/server/package.json", import.meta.url),
);

export const Schema: typeof import("../../../apps/server/node_modules/effect/dist/Schema.js") =
  await import(NodeURL.pathToFileURL(require.resolve("effect/Schema")).href);
export const SchemaAST: typeof import("../../../apps/server/node_modules/effect/dist/SchemaAST.js") =
  await import(NodeURL.pathToFileURL(require.resolve("effect/SchemaAST")).href);
export const Arbitrary: typeof import("../../../apps/server/node_modules/effect/dist/Arbitrary.js") =
  await import(NodeURL.pathToFileURL(require.resolve("effect/Arbitrary")).href);
export const Effect: typeof import("../../../apps/server/node_modules/effect/dist/Effect.js") =
  await import(NodeURL.pathToFileURL(require.resolve("effect/Effect")).href);
export const Layer: typeof import("../../../apps/server/node_modules/effect/dist/Layer.js") =
  await import(NodeURL.pathToFileURL(require.resolve("effect/Layer")).href);
export const RpcSchema: typeof import("../../../apps/server/node_modules/effect/dist/rpc/RpcSchema.js") =
  await import(NodeURL.pathToFileURL(require.resolve("effect/rpc/RpcSchema")).href);
export const version = (require("effect/package.json") as { version: string }).version;
export type ContractSchema = import("../../../apps/server/node_modules/effect/dist/Schema.js").Top;
export type Ast = import("../../../apps/server/node_modules/effect/dist/SchemaAST.js").AST;
