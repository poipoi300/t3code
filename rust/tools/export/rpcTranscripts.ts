// @effect-diagnostics nodeBuiltinImport:off globalConsole:off
import * as NodeFSP from "node:fs/promises";
import * as Effect from "../../../apps/server/node_modules/effect/dist/Effect.js";
import * as Deferred from "../../../apps/server/node_modules/effect/dist/Deferred.js";
import * as Queue from "../../../apps/server/node_modules/effect/dist/Queue.js";
import * as Fiber from "../../../apps/server/node_modules/effect/dist/Fiber.js";
import * as Schema from "../../../apps/server/node_modules/effect/dist/Schema.js";
import * as Stream from "../../../apps/server/node_modules/effect/dist/Stream.js";
import * as Schedule from "../../../apps/server/node_modules/effect/dist/Schedule.js";
import * as Socket from "../../../apps/server/node_modules/effect/dist/socket/Socket.js";
import * as HttpServerRequest from "../../../apps/server/node_modules/effect/dist/http/HttpServerRequest.js";
import * as Headers from "../../../apps/server/node_modules/effect/dist/http/Headers.js";
import * as Cause from "../../../apps/server/node_modules/effect/dist/Cause.js";
import * as Rpc from "../../../apps/server/node_modules/effect/dist/rpc/Rpc.js";
import * as RpcGroup from "../../../apps/server/node_modules/effect/dist/rpc/RpcGroup.js";
import * as RpcMessage from "../../../apps/server/node_modules/effect/dist/rpc/RpcMessage.js";
import * as RpcClient from "../../../apps/server/node_modules/effect/dist/rpc/RpcClient.js";
import * as RpcServer from "../../../apps/server/node_modules/effect/dist/rpc/RpcServer.js";
import * as RpcSerialization from "../../../apps/server/node_modules/effect/dist/rpc/RpcSerialization.js";
import * as TestClock from "../../../apps/server/node_modules/effect/dist/testing/TestClock.js";
import { withOutputWindow } from "../../../apps/server/src/terminal/OutputProtocol.ts";
import {
  splitLargeSocketFrames,
  joinSocketFrameParts,
  splitSocketFrame,
} from "../../../packages/shared/src/socketFrameParts.ts";

if (process.argv.includes("--verify")) {
  let input = "";
  for await (const chunk of process.stdin) input += chunk;
  const frames = JSON.parse(input) as Array<{
    direction: "client" | "server";
    message: Record<string, unknown>;
  }>;
  const bytes = new Uint8Array();
  for (const { direction, message } of frames) {
    const [parsed] = RpcSerialization.json.makeUnsafe().decode(JSON.stringify(message)) as Array<
      Record<string, unknown>
    >;
    const envelope = { ...parsed };
    switch (parsed._tag) {
      case "Request":
        if (!Object.hasOwn(parsed, "payload")) throw new Error("Request is missing payload");
        Schema.decodeUnknownSync(RpcSerialization.json.codecFor(Schema.Unknown))(parsed.payload);
        envelope.payload = bytes;
        break;
      case "Exit":
        Schema.decodeUnknownSync(
          RpcSerialization.json.codecFor(
            Schema.Exit(Schema.Unknown, Schema.Unknown, Schema.Defect()),
          ),
        )(parsed.exit);
        envelope.exit = bytes;
        break;
      case "Chunk":
        Schema.decodeUnknownSync(
          RpcSerialization.json.codecFor(Schema.NonEmptyArray(Schema.Unknown)),
        )(parsed.values);
        envelope.values = bytes;
        break;
      case "Defect":
        Schema.decodeUnknownSync(RpcSerialization.json.codecFor(Schema.Defect()))(parsed.defect);
        envelope.defect = bytes;
        break;
    }
    const allowed =
      direction === "client"
        ? ["Request", "Ack", "Interrupt", "Ping", "Eof"]
        : ["Request", "Chunk", "Exit", "Defect", "Pong"];
    if (!allowed.includes(String(parsed._tag)))
      throw new Error(`Invalid ${direction} frame: ${parsed._tag}`);
    Schema.decodeUnknownSync(RpcMessage.EncodedSchema)(envelope);
  }
  console.log(`Effect accepted ${frames.length} Rust-generated frames`);
  process.exit(0);
}

const failure = Schema.Struct({ code: Schema.String });
const group = RpcGroup.make(
  Rpc.make("echo", { payload: { value: Schema.String }, success: Schema.String }),
  Rpc.make("fail", { error: failure }),
  Rpc.make("die"),
  Rpc.make("wait", { payload: { key: Schema.String }, success: Schema.String }),
  Rpc.make("numbers", { success: Schema.Number, error: failure, stream: true }),
  Rpc.make("broken", { success: Schema.Number, error: failure, stream: true }),
  Rpc.make("endless", { success: Schema.Number, stream: true }),
  Rpc.make("subscribeServerConfig", { success: Schema.Number, stream: true }),
  Rpc.make("terminal.attach", { success: Schema.Unknown, stream: true }),
  Rpc.make("serverInterrupt"),
  Rpc.make("multipleCauses", { error: failure }),
);
type Entry =
  | { direction: "client" | "server"; kind: "text"; data: string }
  | { action: string; [key: string]: unknown };
const scenarios = [
  "unary-success",
  "typed-failure",
  "defect",
  "stream",
  "stream-failure",
  "interrupt-unary",
  "interrupt-stream",
  "concurrent",
  "keepalive",
  "window",
  "terminal-window",
  "disconnect",
  "discard",
  "eof",
  "unknown-tag",
  "malformed",
  "inactive-interrupt",
  "server-interrupt",
  "ping-after-eof",
  "notification",
  "string-id",
  "batch",
  "multiple-causes",
  "ping-timeout",
];
const upgradeHeaders = [
  ["x-upgrade", "fixture"],
  ["x-fixture", "upgrade"],
] as const;
const destination = new URL("../../fixtures/rpc-transcripts/", import.meta.url);
const effectVersion = JSON.parse(
  await NodeFSP.readFile(
    new URL("../../../apps/server/node_modules/effect/package.json", import.meta.url),
    "utf8",
  ),
).version;
await NodeFSP.mkdir(destination, { recursive: true });

for (const name of scenarios) {
  const entries: Entry[] = [];
  let observedHeaders: unknown[] = [];
  let dropServerFrames = false;
  const program = Effect.gen(function* () {
    const c2s = yield* Queue.make<string, Socket.SocketError>();
    const s2c = yield* Queue.make<string, Socket.SocketError>();
    const observed = yield* Queue.make<{ direction: string; message: Record<string, unknown> }>();
    const started = yield* Queue.make<string>();
    const gates = new Map<string, Deferred.Deferred<string>>();
    let holdAcks = name === "window" || name === "terminal-window";
    const heldAcks: string[] = [];
    const closed = new Socket.SocketError({
      reason: new Socket.SocketCloseError({ code: 1000, closeReason: "fixture close" }),
    });
    const socket = (direction: "client" | "server", input: typeof c2s, output: typeof c2s) => {
      const write = (data: string | Uint8Array | Socket.CloseEvent) =>
        Effect.gen(function* () {
          if (typeof data !== "string")
            return yield* Effect.die("JSON socket unexpectedly wrote binary/close");
          if (direction === "client" && holdAcks && JSON.parse(data)._tag === "Ack") {
            heldAcks.push(data);
            return;
          }
          entries.push({ direction, kind: "text", data });
          if (direction === "server") {
            // Observe complete RPC messages while recording the actual split socket frames.
            if (data.startsWith("\u001e")) pendingServer += data.slice(1);
            else {
              const whole = pendingServer + data;
              pendingServer = "";
              yield* Queue.offer(observed, { direction, message: JSON.parse(whole) });
            }
          } else {
            try {
              yield* Queue.offer(observed, { direction, message: JSON.parse(data) });
            } catch {
              /* malformed-input scenario */
            }
          }
          if (direction !== "server" || !dropServerFrames) yield* Queue.offer(output, data);
        });
      return Socket.make({
        reader: Effect.succeed({
          pull: Effect.map(Queue.take(input), (s): [string] => [s]),
          upgrade: () => Effect.void,
        }),
        writer: Effect.succeed({
          write,
          writeAll: (chunks) => Effect.forEach(chunks, write, { discard: true }),
        }),
      });
    };
    let pendingServer = "";
    const clientSocket = joinSocketFrameParts(socket("client", s2c, c2s));
    const serverSocket = splitLargeSocketFrames(socket("server", c2s, s2c));
    const { protocol, httpEffect } = yield* RpcServer.makeProtocolWithHttpEffectWebsocket;
    const receivedHeaders: unknown[] = [];
    const recordingProtocol = {
      ...protocol,
      run: (write: Parameters<typeof protocol.run>[0]) =>
        protocol.run((id, message) => {
          if (message._tag === "Request") receivedHeaders.push(message.headers);
          return write(id, message);
        }),
    };
    yield* httpEffect.pipe(
      Effect.provideService(HttpServerRequest.HttpServerRequest, {
        headers: Headers.fromInput(upgradeHeaders),
        upgrade: Effect.succeed(serverSocket),
      } as HttpServerRequest.HttpServerRequest),
      Effect.forkScoped,
    );
    const handlers = group.toLayer({
      echo: ({ value }) => Effect.succeed(value),
      fail: () => Effect.fail({ code: "NO" }),
      die: () => Effect.die("fixture defect"),
      serverInterrupt: () => Effect.interrupt,
      multipleCauses: () =>
        Effect.failCause(
          Cause.fromReasons([Cause.makeFailReason({ code: "ONE" }), Cause.makeDieReason("two")]),
        ),
      wait: ({ key }) =>
        Effect.gen(function* () {
          const gate = yield* Deferred.make<string>();
          gates.set(key, gate);
          yield* Queue.offer(started, key);
          return yield* Deferred.await(gate);
        }),
      numbers: () => Stream.fromIterable([1, 2, 3]).pipe(Stream.rechunk(1)),
      broken: () => Stream.concat(Stream.make(1), Stream.fail({ code: "MID" })),
      endless: () => Stream.concat(Stream.make(1), Stream.never),
      subscribeServerConfig: () => Stream.range(1, 10).pipe(Stream.rechunk(1)),
      "terminal.attach": () =>
        Stream.fromIterable([
          { type: "output", threadId: "thread", terminalId: "terminal", data: "é".repeat(32768) },
          { type: "activity", label: "complete" },
        ]).pipe(Stream.rechunk(1)),
    });
    yield* RpcServer.make(group, { disableTracing: true, disableFatalDefects: true }).pipe(
      Effect.provide(handlers),
      Effect.provideService(RpcServer.Protocol, withOutputWindow(recordingProtocol)),
      Effect.forkScoped,
    );
    const clientProtocol = yield* RpcClient.makeProtocolSocket({
      pingTimeout: "15 seconds",
      retryTransientErrors: false,
      retryPolicy: Schedule.recurs(0),
    }).pipe(Effect.provideService(Socket.Socket, clientSocket));
    let nextId = 0;
    const client = yield* RpcClient.make(group, {
      generateRequestId: () => RpcMessage.RequestId(nextId++),
    }).pipe(Effect.provideService(RpcClient.Protocol, clientProtocol));
    const until = (direction: string, tag: string) =>
      Effect.gen(function* () {
        while (true) {
          const item = yield* Queue.take(observed);
          if (item.direction === direction && item.message._tag === tag) return item.message;
        }
      });
    const action = (action: string, fields = {}) => entries.push({ action, ...fields });
    const raw = (value: unknown) =>
      Effect.flatMap(clientSocket.writer, (writer) =>
        writer.write(typeof value === "string" ? value : JSON.stringify(value)),
      );
    if (name === "unary-success")
      yield* client.echo({ value: "hello 🌍" }, { headers: { "x-fixture": "yes" } });
    if (name === "typed-failure") yield* Effect.exit(client.fail());
    if (name === "defect") yield* Effect.exit(client.die());
    if (name === "server-interrupt") yield* Effect.exit(client.serverInterrupt());
    if (name === "multiple-causes") yield* Effect.exit(client.multipleCauses());
    if (name === "unknown-tag") {
      yield* raw({ _tag: "Request", id: 0, tag: "absent", payload: null, headers: [] });
      yield* until("server", "Exit");
    }
    if (name === "malformed") {
      yield* raw("{");
      yield* until("server", "Defect");
      yield* raw({ _tag: "Ping" });
      yield* until("server", "Pong");
    }
    if (name === "inactive-interrupt") {
      yield* raw({ _tag: "Interrupt", requestId: 99 });
      yield* client.echo({ value: "done" });
      yield* raw({ _tag: "Interrupt", requestId: 0 });
      yield* raw({ _tag: "Ping" });
      yield* until("server", "Pong");
    }
    if (name === "notification") {
      yield* raw({
        _tag: "Request",
        id: "",
        tag: "echo",
        payload: { value: "silent" },
        headers: [],
        isNotification: true,
      });
      yield* raw({ _tag: "Ping" });
      yield* until("server", "Pong");
    }
    if (name === "string-id" || name === "batch") {
      const request = {
        _tag: "Request",
        id: "string",
        tag: "echo",
        payload: { value: "value" },
        headers: [],
      };
      yield* raw(name === "batch" ? [request, { _tag: "Ping" }] : request);
      yield* until("server", "Exit");
    }
    if (name === "ping-timeout") {
      const call = yield* Effect.exit(client.wait({ key: "timeout" })).pipe(Effect.forkScoped);
      yield* Queue.take(started);
      action("tick", { now: 5000 });
      yield* TestClock.adjust("5 seconds");
      yield* until("server", "Pong");
      // Keep the server readable but discard its pongs on the return path.
      dropServerFrames = true;
      action("drop-server-frames");
      for (let i = 0; i < 4; i++) {
        action("tick", { now: 10000 + i * 5000 });
        yield* TestClock.adjust("5 seconds");
      }
      const result = yield* Fiber.join(call);
      if (result._tag !== "Failure") return yield* Effect.die("ping timeout did not fail the call");
      action("expect-ping-timeout");
      action("disconnect");
      yield* Queue.fail(c2s, closed);
      yield* Queue.fail(s2c, closed);
    }
    if (name === "stream") yield* Stream.runCollect(client.numbers());
    if (name === "stream-failure") yield* Effect.exit(Stream.runCollect(client.broken()));
    if (name === "discard") yield* client.echo({ value: "ignored" }, { discard: true });
    if (name === "interrupt-unary") {
      const fiber = yield* client.wait({ key: "a" }).pipe(Effect.forkScoped);
      yield* Queue.take(started);
      action("interrupt", { id: 0 });
      yield* Fiber.interrupt(fiber);
      yield* until("server", "Exit");
    }
    if (name === "interrupt-stream") {
      const fiber = yield* Stream.runForEach(client.endless(), () =>
        Queue.offer(started, "chunk"),
      ).pipe(Effect.forkScoped);
      yield* Queue.take(started);
      action("interrupt", { id: 0 });
      yield* Fiber.interrupt(fiber);
      yield* until("server", "Exit");
    }
    if (name === "concurrent") {
      const a = yield* client.wait({ key: "a" }).pipe(Effect.forkScoped);
      yield* Queue.take(started);
      const stream = yield* Stream.runCollect(client.numbers()).pipe(Effect.forkScoped);
      yield* until("server", "Chunk");
      const b = yield* client.wait({ key: "b" }).pipe(Effect.forkScoped);
      yield* Queue.take(started);
      action("release", { key: "b", value: "second" });
      yield* Deferred.succeed(gates.get("b")!, "second");
      yield* Fiber.join(b);
      action("release", { key: "a", value: "first" });
      yield* Deferred.succeed(gates.get("a")!, "first");
      yield* Fiber.join(a);
      yield* Fiber.join(stream);
    }
    if (name === "keepalive") {
      action("tick", { now: 5000 });
      yield* TestClock.adjust("5 seconds");
      yield* until("server", "Pong");
    }
    if (name === "window" || name === "terminal-window") {
      const fiber = yield* Stream.runCollect(
        name === "window" ? client.subscribeServerConfig() : client["terminal.attach"](),
      ).pipe(Effect.forkScoped);
      for (let i = 0; i < (name === "window" ? 8 : 1); i++) yield* until("server", "Chunk");
      action("release-acks");
      holdAcks = false;
      const writer = yield* clientSocket.writer;
      for (const ack of heldAcks) yield* writer.write(ack);
      yield* Fiber.join(fiber);
    }
    if (name === "eof" || name === "ping-after-eof") {
      const fiber = yield* client.wait({ key: "a" }).pipe(Effect.forkScoped);
      yield* Queue.take(started);
      action("eof");
      yield* clientProtocol.send(0, { _tag: "Eof" });
      if (name === "ping-after-eof") {
        yield* raw({ _tag: "Ping" });
        yield* until("server", "Pong");
      }
      action("release", { key: "a", value: "done" });
      yield* Deferred.succeed(gates.get("a")!, "done");
      yield* Fiber.join(fiber);
    }
    if (name === "disconnect") {
      const a = yield* Effect.exit(client.wait({ key: "a" })).pipe(Effect.forkScoped);
      yield* Queue.take(started);
      const b = yield* Effect.exit(
        Stream.runForEach(client.endless(), () => Queue.offer(started, "chunk")),
      ).pipe(Effect.forkScoped);
      yield* Queue.take(started);
      action("disconnect");
      yield* Queue.fail(s2c, closed);
      yield* Queue.fail(c2s, closed);
      yield* Fiber.join(a);
      yield* Fiber.join(b);
    }
    observedHeaders = receivedHeaders;
  }).pipe(
    Effect.scoped,
    Effect.provide(RpcSerialization.layerJson),
    Effect.provide(TestClock.layer()),
  );
  await Effect.runPromise(program);
  await NodeFSP.writeFile(
    new URL(`${name}.json`, destination),
    JSON.stringify(
      {
        effectVersion,
        name,
        frameParts: true,
        upgradeHeaders,
        receivedHeaders: observedHeaders,
        entries,
      },
      null,
      2,
    ) + "\n",
  );
  console.log(`${name}: ${entries.filter((e) => "direction" in e).length} frames`);
}

const framingCases = [49150, 49151, 49152, 49153]
  .map((n) => "x".repeat(n))
  .concat(["界".repeat(16384), "🌍éa".repeat(30000)]);
await NodeFSP.writeFile(
  new URL("framing-cases.json", destination),
  JSON.stringify(
    {
      effectVersion,
      cases: framingCases.map((frame) => ({ frame, parts: splitSocketFrame(frame) })),
    },
    null,
    2,
  ) + "\n",
);
