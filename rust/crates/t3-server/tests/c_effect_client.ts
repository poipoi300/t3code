// @effect-diagnostics nodeBuiltinImport:off globalConsole:off
// Drives a running Rust t3-server with the real Effect RPC client stack that the product's
// client runtime uses (packages/client-runtime/src/rpc/session.ts): WsRpcGroup from
// @t3tools/contracts, RpcClient.makeProtocolSocket over a WebSocket with the frame-part
// joiner. Prints one JSON report line; exits non-zero when any expectation fails.
// Usage: node c_effect_client.ts <ws url including orchestrationProtocol=2 and wsTicket>
import * as Cause from "../../../../apps/server/node_modules/effect/dist/Cause.js";
import * as Effect from "../../../../apps/server/node_modules/effect/dist/Effect.js";
import * as Layer from "../../../../apps/server/node_modules/effect/dist/Layer.js";
import * as Schedule from "../../../../apps/server/node_modules/effect/dist/Schedule.js";
import * as Stream from "../../../../apps/server/node_modules/effect/dist/Stream.js";
import * as RpcClient from "../../../../apps/server/node_modules/effect/dist/rpc/RpcClient.js";
import * as RpcSerialization from "../../../../apps/server/node_modules/effect/dist/rpc/RpcSerialization.js";
import * as Socket from "../../../../apps/server/node_modules/effect/dist/socket/Socket.js";
import { WS_METHODS, WsRpcGroup } from "../../../../packages/contracts/src/index.ts";
import { joinSocketFrameParts } from "../../../../packages/shared/src/socketFrameParts.ts";

const url = process.argv[2];
if (!url) throw new Error("missing websocket url");

const expect = (condition: unknown, message: string): void => {
  if (!condition) throw new Error(message);
};

const describeExit = (exit: any) => {
  if (exit._tag === "Success") return { _tag: "Success" as const };
  return {
    _tag: "Failure" as const,
    reasons: (exit.cause.reasons as ReadonlyArray<any>).map((reason) => ({
      _tag: reason._tag as string,
      value: reason._tag === "Die" ? reason.defect : reason.error,
    })),
  };
};

const program = Effect.gen(function* () {
  const layerSocket = Layer.effect(
    Socket.Socket,
    Effect.map(Effect.service(Socket.Socket), joinSocketFrameParts),
  ).pipe(
    Layer.provide(Socket.layerWebSocket(url)),
    Layer.provide(Socket.layerWebSocketConstructorGlobal),
  );
  const layerProtocol = Layer.effect(
    RpcClient.Protocol,
    RpcClient.makeProtocolSocket({
      pingTimeout: "15 seconds",
      retryTransientErrors: false,
      retryPolicy: Schedule.recurs(0),
    }),
  ).pipe(Layer.provide(Layer.mergeAll(layerSocket, RpcSerialization.layerJson)));
  const context = yield* Layer.build(layerProtocol);
  const client = yield* RpcClient.make(WsRpcGroup).pipe(Effect.provide(context));
  const report: Record<string, unknown> = {};

  const probe = () => client[WS_METHODS.serverProbe]({});
  yield* probe();
  report.firstProbe = "ok";

  // A unary method the Rust server does not implement: its Fail union has no NotImplemented
  // member, so the server answers on the Die channel. The client must surface that as a
  // defect on this call only.
  const unary = yield* Effect.exit(client[WS_METHODS.serverRunStorageCleanup]({}));
  report.unsupportedUnary = describeExit(unary);
  expect(unary._tag === "Failure", "unsupported unary call must fail");
  const unaryReasons = (report.unsupportedUnary as any).reasons;
  expect(
    unaryReasons.length === 1 && unaryReasons[0]._tag === "Die",
    `unsupported unary must be a single defect: ${JSON.stringify(unaryReasons)}`,
  );
  expect(
    unaryReasons[0].value === "Unknown request tag: server.runStorageCleanup",
    `defect must be Effect's unknown-request-tag answer: ${JSON.stringify(unaryReasons[0])}`,
  );

  // The session survives it.
  yield* probe();
  report.probeAfterUnaryDefect = "ok";

  const stream = yield* Effect.exit(
    Stream.runCollect(client[WS_METHODS.serverGetStorageCleanupReport]({})),
  );
  report.unsupportedStream = describeExit(stream);
  expect(stream._tag === "Failure", "unsupported stream must fail");
  const streamReasons = (report.unsupportedStream as any).reasons;
  expect(
    streamReasons.length === 1 &&
      streamReasons[0]._tag === "Die" &&
      streamReasons[0].value === "Unknown request tag: server.getStorageCleanupReport",
    `unsupported stream must be the unknown-request-tag defect: ${JSON.stringify(streamReasons)}`,
  );

  yield* probe();
  report.probeAfterStreamDefect = "ok";

  // The implemented subscription streams through the same session.
  const first = yield* Stream.runHead(client[WS_METHODS.subscribeServerConfig]({}));
  expect(first._tag === "Some" && (first.value as any).type === "snapshot", "config snapshot");
  report.configSnapshot = "ok";

  const config = yield* client[WS_METHODS.serverGetConfig]({});
  expect(config.environment.orchestrationProtocolVersion === 2, "config environment version");
  report.getConfig = "ok";

  yield* probe();
  report.lastProbe = "ok";
  return report;
});

Effect.runPromise(Effect.scoped(program)).then(
  (report) => {
    console.log(JSON.stringify({ ok: true, ...report }));
    process.exit(0);
  },
  (error) => {
    console.log(JSON.stringify({ ok: false, error: String(error?.stack ?? error) }));
    process.exit(1);
  },
);
