// @effect-diagnostics nodeBuiltinImport:off globalConsole:off
// Runs the web client's start-up against a live Rust t3-server, in the order the web app runs
// it, with the product's own client-runtime code wherever it can be imported:
//   apps/web/src/environments/primary/auth.ts      auth.session, auth.browserSession, auth.session
//   client-runtime connection/resolver.ts           descriptor, primary socket URL (cookie auth)
//   client-runtime rpc/session.ts                   socket open, subscribeServerConfig, server.probe
//   client-runtime state/server.ts                  subscribeServerLifecycle
//   client-runtime state/shellSnapshotHttp.ts       HTTP shell snapshot (early load)
//   client-runtime state/shell.ts                   orchestration.subscribeShell
//   apps/web/src/lib/backgroundActivityReporter.ts  server.reportClientActivity
// Only the browser itself is emulated: a cookie jar for fetch and the WebSocket upgrade.
// Prints one JSON line: { steps: [{ step, call, status, detail, missing? }] }.
// status: served | difference | unsupported | failed.
// Usage: node composition_web_boot_client.ts <http base url> <pairing credential>
import * as Cause from "../../../../packages/client-runtime/node_modules/effect/dist/Cause.js";
import * as DateTime from "../../../../packages/client-runtime/node_modules/effect/dist/DateTime.js";
import * as Duration from "../../../../packages/client-runtime/node_modules/effect/dist/Duration.js";
import * as Effect from "../../../../packages/client-runtime/node_modules/effect/dist/Effect.js";
import * as Exit from "../../../../packages/client-runtime/node_modules/effect/dist/Exit.js";
import * as Layer from "../../../../packages/client-runtime/node_modules/effect/dist/Layer.js";
import * as Option from "../../../../packages/client-runtime/node_modules/effect/dist/Option.js";
import * as Scope from "../../../../packages/client-runtime/node_modules/effect/dist/Scope.js";
import * as Stream from "../../../../packages/client-runtime/node_modules/effect/dist/Stream.js";
import * as Socket from "../../../../packages/client-runtime/node_modules/effect/dist/socket/Socket.js";
import { WS_METHODS } from "../../../../packages/contracts/src/index.ts";
import { ORCHESTRATION_V2_WS_METHODS } from "../../../../packages/contracts/src/orchestrationV2.ts";
import {
  layerRemoteHttpClient,
  makeEnvironmentHttpApiClient,
} from "../../../../packages/client-runtime/src/rpc/http.ts";
import * as RpcSession from "../../../../packages/client-runtime/src/rpc/session.ts";
import { fetchRemoteEnvironmentDescriptor } from "../../../../packages/client-runtime/src/environment/descriptor.ts";
import {
  appendOrchestrationProtocol,
  orchestrationProtocolCompatibilityError,
} from "../../../../packages/client-runtime/src/connection/compatibility.ts";
import { appendClientConnectionParams } from "../../../../packages/client-runtime/src/authorization/remote.ts";
import { fetchEnvironmentShellSnapshot } from "../../../../packages/client-runtime/src/state/shellSnapshotHttp.ts";

const [httpBaseUrl, credential] = process.argv.slice(2);
if (!httpBaseUrl || !credential) throw new Error("usage: <http base url> <pairing credential>");
const origin = new URL(httpBaseUrl).origin;

// The browser's cookie jar for this origin (the web primary uses `credentials: "include"`).
const cookies = new Map<string, string>();
const cookieHeader = () => [...cookies].map(([k, v]) => `${k}=${v}`).join("; ");
const browserFetch: typeof globalThis.fetch = async (input, init) => {
  const request = new Request(input, init);
  if (new URL(request.url).origin === origin && cookies.size > 0) {
    request.headers.set("cookie", cookieHeader());
  }
  const response = await globalThis.fetch(request);
  for (const line of response.headers.getSetCookie()) {
    const [pair] = line.split(";");
    const index = pair.indexOf("=");
    if (index > 0) cookies.set(pair.slice(0, index).trim(), pair.slice(index + 1).trim());
  }
  return response;
};
// The browser sends the session cookie on the same-origin WebSocket upgrade.
const browserWebSocket = (url: string | URL, protocols?: string | Array<string>) =>
  new (globalThis.WebSocket as any)(url, {
    protocols,
    headers: { cookie: cookieHeader() },
  }) as globalThis.WebSocket;

const layerHttp = layerRemoteHttpClient(browserFetch);
const STEP_TIMEOUT = Duration.seconds(10);

// Where a step is unsupported, the Rust service method it is waiting for.
const MISSING: Record<string, string> = {
  [ORCHESTRATION_V2_WS_METHODS.subscribeShell]:
    "TransportService::subscribe_shell (t3-orchestration), registered by package D",
  [WS_METHODS.serverReportClientActivity]:
    "a client-activity lease service (TS background/ClientActivity), not ported",
  "GET /api/orchestration/shell":
    "OrchestrationAdapter shell snapshot read (D), served by an HTTP route (B)",
};

type Status = "served" | "difference" | "unsupported" | "failed";
interface StepReport {
  step: number;
  call: string;
  status: Status;
  detail: string;
  missing?: string;
}
const steps: Array<StepReport> = [];

const describeCause = (cause: Cause.Cause<unknown>): { notImplemented: boolean; text: string } => {
  const reasons = (cause as any).reasons as ReadonlyArray<any> | undefined;
  for (const reason of reasons ?? []) {
    const value = reason._tag === "Die" ? reason.defect : reason.error;
    if (typeof value === "string" && value.startsWith("Unknown request tag: "))
      return { notImplemented: true, text: value };
    if (value?._tag === "NotImplemented") return { notImplemented: true, text: `NotImplemented ${value.method}` };
    const status = value?.status ?? value?.response?.status ?? value?.cause?.response?.status;
    if (status === 404) return { notImplemented: true, text: "HTTP 404 (route not served)" };
  }
  return { notImplemented: false, text: Cause.pretty(cause).split("\n").slice(0, 3).join(" | ") };
};

/** Runs one step; `check` returns "" when the answer matches what the web client needs. */
const step = <A, E, R>(
  call: string,
  effect: Effect.Effect<A, E, R>,
  check: (value: A) => string,
) =>
  Effect.gen(function* () {
    const exit = yield* Effect.exit(Effect.timeout(effect, STEP_TIMEOUT));
    const report: StepReport = { step: steps.length + 1, call, status: "served", detail: "" };
    if (Exit.isSuccess(exit)) {
      const difference = check(exit.value);
      if (difference !== "") Object.assign(report, { status: "difference", detail: difference });
    } else {
      const { notImplemented, text } = describeCause(exit.cause);
      Object.assign(report, { status: notImplemented ? "unsupported" : "failed", detail: text });
      if (notImplemented) report.missing = MISSING[call] ?? "unknown";
    }
    steps.push(report);
    return Exit.isSuccess(exit) ? Option.some(exit.value) : Option.none<A>();
  });

const program = Effect.gen(function* () {
  const http = yield* makeEnvironmentHttpApiClient(httpBaseUrl);

  // apps/web/src/environments/primary/auth.ts bootstrapServerAuth
  yield* step("GET /api/auth/session", http.auth.session({ headers: {} }), (s: any) =>
    s.authenticated === false ? "" : "expected an unauthenticated session before pairing",
  );
  yield* step(
    "POST /api/auth/browser-session",
    http.auth.browserSession({ payload: { credential } }),
    (r: any) => (r.authenticated === true && cookies.size > 0 ? "" : "no authenticated cookie session"),
  );
  yield* step("GET /api/auth/session", http.auth.session({ headers: {} }), (s: any) =>
    s.authenticated === true ? "" : "session not authenticated after pairing",
  );

  // connection/resolver.ts: prepare the primary connection
  const descriptor = yield* step(
    "GET /.well-known/t3/environment",
    fetchRemoteEnvironmentDescriptor({ httpBaseUrl }),
    (d: any) => (orchestrationProtocolCompatibilityError(d) === null ? "" : "protocol incompatible"),
  );
  if (Option.isNone(descriptor)) return;
  const d: any = descriptor.value;
  const wsBase = new URL(httpBaseUrl);
  wsBase.protocol = wsBase.protocol === "https:" ? "wss:" : "ws:";
  wsBase.pathname = "/ws";
  appendClientConnectionParams(wsBase, { surface: "web", deviceType: "desktop" } as any, "direct", {});
  const target = {
    _tag: "PrimaryConnectionTarget",
    environmentId: d.environmentId,
    label: d.label,
    httpBaseUrl,
    wsBaseUrl: wsBase.toString(),
  };
  const prepared: any = {
    environmentId: d.environmentId,
    label: d.label,
    httpBaseUrl,
    socketUrl: appendOrchestrationProtocol(wsBase.toString()),
    httpAuthorization: null,
    target,
    descriptor: d,
  };

  // rpc/session.ts with the web's options (apps/web/src/connection/runtime.ts)
  const scope = yield* Scope.make();
  const factory = yield* RpcSession.make({
    environmentThemes: true,
    usageLimitSources: true,
    usageLimitsCommand: true,
  }).pipe(Effect.provideService(Socket.WebSocketConstructor, browserWebSocket as any));
  const session = yield* factory.connect(prepared).pipe(Scope.provide(scope));
  const config = yield* step(
    "WS /ws (cookie) + subscribeServerConfig snapshot",
    Effect.andThen(session.ready, session.initialConfig),
    (c: any) => (c.environment.environmentId === d.environmentId ? "" : "environment mismatch"),
  );
  if (Option.isNone(config)) return;
  const c: any = config.value;
  const providers = c.providers?.length ?? 0;
  if (providers === 0) {
    steps[steps.length - 1] = {
      ...steps[steps.length - 1],
      status: "difference",
      detail: "config has no providers and no available editors (provider registry not assembled)",
    };
  }
  yield* step(WS_METHODS.serverProbe, session.probe, () => "");

  // state/server.ts
  yield* step(
    WS_METHODS.subscribeServerLifecycle,
    session.client[WS_METHODS.subscribeServerLifecycle]({}).pipe(Stream.take(2), Stream.runCollect),
    (events: any) => {
      const types = [...events].map((e: any) => e.type).join(",");
      return types === "welcome,ready" ? "" : `expected welcome,ready; got ${types}`;
    },
  );

  // state/shellSnapshotHttp.ts early load, then state/shell.ts
  yield* step(
    "GET /api/orchestration/shell",
    fetchEnvironmentShellSnapshot({ prepared, signer: Option.none() }).pipe(Effect.provide(layerHttp)),
    () => "",
  );
  yield* step(
    ORCHESTRATION_V2_WS_METHODS.subscribeShell,
    session.client[ORCHESTRATION_V2_WS_METHODS.subscribeShell]({}).pipe(Stream.take(1), Stream.runCollect),
    () => "",
  );

  // apps/web/src/lib/backgroundActivityReporter.ts createActivityReport
  yield* step(
    WS_METHODS.serverReportClientActivity,
    session.client[WS_METHODS.serverReportClientActivity]({
      environmentId: d.environmentId,
      clientId: "web-boot-probe",
      clientKind: "web",
      visible: true,
      focused: true,
      recentlyInteracted: true,
      appState: "active",
      scopes: [{ type: "provider-status" }],
      ttlMs: 45_000,
      observedAt: DateTime.makeUnsafe(Date.now()),
    } as any),
    () => "",
  );
  yield* Scope.close(scope, Exit.void);
}).pipe(Effect.provide(layerHttp), Effect.scoped);

Effect.runPromiseExit(program as Effect.Effect<void, unknown, never>).then((exit) => {
  const fatal = Exit.isFailure(exit) ? Cause.pretty(exit.cause).split("\n").slice(0, 6).join(" | ") : null;
  console.log(JSON.stringify({ steps, fatal }));
  process.exit(fatal === null ? 0 : 1);
});
