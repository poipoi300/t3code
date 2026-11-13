import * as Fs from "node:fs/promises";
import * as Path from "node:path";
import * as Effect from "../../../../apps/server/node_modules/effect/dist/Effect.js";
import * as Layer from "../../../../apps/server/node_modules/effect/dist/Layer.js";
import * as Duration from "../../../../apps/server/node_modules/effect/dist/Duration.js";
import * as Redacted from "../../../../apps/server/node_modules/effect/dist/Redacted.js";
import { createHash } from "node:crypto";
import * as HttpServerRequest from "../../../../apps/server/node_modules/effect/dist/http/HttpServerRequest.js";
import * as NodeCrypto from "../../../../apps/server/node_modules/@effect/platform-node/dist/NodeCrypto.js";
import * as NodeServices from "../../../../apps/server/node_modules/@effect/platform-node/dist/NodeServices.js";
import * as Session from "../../../../apps/server/src/auth/SessionStore.ts";
import * as Pairing from "../../../../apps/server/src/auth/PairingGrantStore.ts";
import * as Secrets from "../../../../apps/server/src/auth/ServerSecretStore.ts";
import * as EnvironmentAuth from "../../../../apps/server/src/auth/EnvironmentAuth.ts";
import * as McpOAuth from "../../../../apps/server/src/auth/McpOAuth.ts";
import * as PayloadSigner from "../../../../apps/server/src/auth/PayloadSigner.ts";
import * as ManagedPolicy from "../../../../apps/server/src/policy/ManagedPolicy.ts";
import * as Settings from "../../../../apps/server/src/serverSettings.ts";
import * as Sqlite from "../../../../apps/server/src/persistence/Sqlite.ts";
import * as Config from "../../../../apps/server/src/config.ts";
import * as Environment from "../../../../apps/server/src/environment/ServerEnvironment.ts";
import { makeDpopKey } from "../../../../apps/server/src/auth/dpop.testkit.ts";
import { verifyRequestDpopProof } from "../../../../apps/server/src/auth/dpop.ts";
import { currentDesktopBootstrapToken } from "../../../../packages/shared/src/desktopBootstrapToken.ts";

const [phase, directory] = process.argv.slice(2);
if (!directory || !Path.isAbsolute(directory) || !["prepare", "verify", "finish"].includes(phase)) {
  throw new Error("Expected prepare|verify|finish and an absolute temporary directory.");
}
const base = Path.resolve(directory);
if (!Path.basename(base).startsWith("t3-auth-interop-")) throw new Error("Requires a dedicated temporary interop directory.");
await Fs.mkdir(base, { recursive: true });
const config = {
  mode: "web", port: 3773, host: "127.0.0.1", stateDir: base,
  secretsDir: Path.join(base, "secrets"), devAllowedOrigins: [],
} as unknown as Config.ServerConfig["Service"];
const authLayer = EnvironmentAuth.layer.pipe(
  Layer.provideMerge(Secrets.layer),
  Layer.provide(ManagedPolicy.layerTest()),
);
const services = McpOAuth.layer.pipe(
  Layer.provideMerge(authLayer),
  Layer.provide(Settings.layerTest({ linkedEnvironmentsEnabled: true })),
  Layer.provide(Sqlite.layerFromPath(Path.join(base, "state.sqlite"))),
  Layer.provide(Layer.succeed(Config.ServerConfig, config)),
  Layer.provide(Layer.succeed(Environment.ServerEnvironmentIdentity, {
    getEnvironmentId: Effect.succeed("interop-environment" as never),
  })),
  Layer.provideMerge(Layer.mergeAll(NodeServices.layer, NodeCrypto.layer)),
);
const check = (value: boolean, label: string) => { if (!value) throw new Error(label); };
const rejected = Effect.fn(function* <A, E, R>(effect: Effect.Effect<A, E, R>, label: string) {
  check((yield* Effect.result(effect))._tag === "Failure", `Accepted ${label}`);
});
const urls = { issuer: "https://example.test", resource: "https://example.test/mcp" };
const authorization = (clientId: string) => ({
  client_id: clientId, redirect_uri: "https://example.test/callback", response_type: "code",
  code_challenge: "a".repeat(43), code_challenge_method: "S256",
});
const request = (proof: string) => HttpServerRequest.fromWeb(new Request("https://example.test/ws", {
  headers: { host: "example.test", dpop: proof },
}));
const mcpRequest = (token: string) => HttpServerRequest.fromWeb(new Request(urls.resource, {
  headers: { authorization: `Bearer ${token}` },
}));
await Effect.runPromise(Effect.gen(function* () {
  const sessions = yield* Session.SessionStore;
  const pairing = yield* Pairing.PairingGrantStore;
  const oauth = yield* McpOAuth.McpOAuth;
  const auth = yield* EnvironmentAuth.EnvironmentAuth;
  if (phase === "prepare") {
    const issued = yield* sessions.issue({ subject: "typescript", method: "bearer-access-token", scopes: ["orchestration:read"] });
    const key = makeDpopKey();
    const flags = yield* sessions.issue({ subject: "typescript-flags", method: "dpop-access-token", scopes: ["orchestration:read"], proofKeyThumbprint: key.thumbprint, runtimeModeCeiling: "auto", peerLink: true });
    const expired = yield* sessions.issue({ ttl: Duration.millis(-1) });
    const revoked = yield* sessions.issue();
    const revokedTicket = yield* sessions.issueWebSocketToken(revoked.sessionId);
    yield* sessions.revoke(revoked.sessionId);
    const ticket = yield* sessions.issueWebSocketToken(issued.sessionId, { audience: "rpc", proofKeyThumbprint: key.thumbprint });
    const expiredTicket = yield* sessions.issueWebSocketToken(issued.sessionId, { ttl: Duration.millis(-1) });
    const pairingCode = yield* pairing.issueOneTimeToken({ label: "TS phone" });
    const expiredPairing = yield* pairing.issueOneTimeToken({ ttl: Duration.millis(-1) });
    const revokedPairing = yield* pairing.issueOneTimeToken();
    yield* pairing.revoke(revokedPairing.id);
    const approval = yield* pairing.issueOneTimeToken({ mcpApproval: "auto" });
    const proof = key.sign({ method: "GET", url: "https://example.test/ws", token: issued.token, iat: Math.floor(Date.now() / 1000) });
    const rustSpends = key.sign({ method: "GET", url: "https://example.test/ws", token: issued.token, iat: Math.floor(Date.now() / 1000) });
    yield* verifyRequestDpopProof({ request: request(proof), expectedAccessToken: issued.token });
    const client = yield* oauth.register({ redirect_uris: ["https://example.test/callback"], client_name: "TS client" });
    const verifier = "v".repeat(64);
    const authz = yield* oauth.validateAuthorization({ urls, request: {
      ...authorization(client.clientId), resource: urls.resource,
      code_challenge: createHash("sha256").update(verifier).digest("base64url"),
    } });
    const grant = yield* pairing.issueOneTimeToken();
    const redirect = yield* oauth.approve({ request: mcpRequest("unused"), authorization: authz,
      decision: { _tag: "pairing-code", code: grant.credential, access: "auto" } });
    const code = new URL(redirect).searchParams.get("code");
    check(code !== null, "Missing TypeScript authorization code");
    const mcp = yield* oauth.exchangeCode({ request: mcpRequest("unused"), urls, token: {
      grant_type: "authorization_code", client_id: client.clientId,
      redirect_uri: "https://example.test/callback", code: Redacted.make(code!),
      code_verifier: Redacted.make(verifier), resource: urls.resource,
    } });
    const mcpClaims = JSON.parse(Buffer.from(mcp.access_token.split(".")[0], "base64url").toString("utf8"));
    check(mcpClaims.aud === undefined, "TypeScript issuer audience contract changed");
    // Explicit extension fixture: production TS issuance above still has no audience.
    const body = Buffer.from(JSON.stringify({ ...mcpClaims, aud: urls.resource })).toString("base64url");
    const mcpSigner = yield* PayloadSigner.make("server-signing-key");
    const mcpBound = `${body}.${yield* mcpSigner.sign(body)}`;
    const tsClient = yield* auth.authenticateMcpClient(mcpRequest(mcp.access_token));
    check(tsClient.access === "auto", "Wrong TypeScript MCP access");
    const desktop = yield* currentDesktopBootstrapToken("desktop-secret", Date.now());
    yield* Effect.promise(() => Fs.writeFile(Path.join(base, "typescript.json"), JSON.stringify({ mcp: mcp.access_token, mcpBound, session: issued.token, flags: flags.token, ticket: ticket.token, expired: expired.token, revoked: revoked.token, expiredTicket: expiredTicket.token, revokedTicket: revokedTicket.token, pairing: pairingCode.credential, expiredPairing: expiredPairing.credential, revokedPairing: revokedPairing.credential, approval: approval.credential, proof, rustSpends, thumbprint: key.thumbprint, clientId: client.clientId, desktop })));
    console.log("TypeScript OAuth issued an audience-less MCP token; real PayloadSigner also signed an explicitly extended audience fixture.");
    console.log("TypeScript prepared positive and negative credentials and spent a DPoP proof.");
  } else {
    const output = JSON.parse(yield* Effect.promise(() => Fs.readFile(Path.join(base, "rust.json"), "utf8")));
    if (phase === "finish") {
      yield* rejected(verifyRequestDpopProof({ request: request(output.tsSpends), expectedAccessToken: output.session }), "TypeScript proof replay after Rust recheck");
      console.log("TypeScript confirmed persistent replay rejection after both processes reopened.");
      return;
    }
    const session = yield* sessions.verify(output.session);
    const rustMcp = yield* auth.authenticateMcpClient(mcpRequest(output.mcp));
    check(rustMcp.access === "auto" && rustMcp.label === "Rust MCP client", "Wrong Rust MCP identity/access");
    const rustMcpSession = yield* sessions.verify(output.mcp);
    check(rustMcpSession.scopes.join(" ") === "orchestration:read orchestration:operate", "Wrong Rust MCP scopes");
    const rustClaims = JSON.parse(Buffer.from(output.mcp.split(".")[0], "base64url").toString("utf8"));
    check(rustClaims.aud === urls.resource, "Missing Rust MCP audience");
    for (const token of [output.mcpExpired, output.mcpRevoked, output.mcp + "x"]) {
      yield* rejected(auth.authenticateMcpClient(mcpRequest(token)), "Rust expired, revoked or tampered MCP token");
    }
    yield* sessions.revoke(rustMcp.sessionId);
    console.log("TypeScript accepted the Rust OAuth MCP token, checked access/scopes, rejected expiry/revocation/tampering and revoked its shared row.");
    check(session.subject === "rust", "Wrong Rust session subject");
    check(session.scopes.join(" ") === "orchestration:read", "Wrong Rust session scopes");
    const flags = yield* sessions.verify(output.flags);
    check(flags.proofKeyThumbprint === output.thumbprint && flags.runtimeModeCeiling === "auto" && flags.peerLink === true, "Lost Rust jkt/rtc/plk claims");
    yield* sessions.verifyWebSocketToken(output.ticket);
    for (const [label, token] of [["expired", output.expired], ["revoked", output.revoked], ["tampered", output.session + "x"]]) {
      yield* rejected(sessions.verify(token), `Rust ${label} session`);
    }
    for (const token of [output.expiredTicket, output.revokedTicket, output.ticket + "x"]) yield* rejected(sessions.verifyWebSocketToken(token), "Rust invalid ticket");
    yield* pairing.consume(output.pairing);
    yield* rejected(pairing.consume(output.pairing), "Rust spent pairing code");
    for (const token of [output.expiredPairing, output.revokedPairing]) yield* rejected(pairing.consume(token), "Rust unavailable pairing code");
    yield* pairing.consume(output.approval, { mcpApproval: "auto" });
    // validateAuthorization invokes the service's actual private parseClientId, including its MAC and payload decoder.
    const client = yield* oauth.validateAuthorization({ urls, request: authorization(output.clientId) });
    check(client.client.name === "MCP client", "Wrong Rust registered client");
    yield* rejected(oauth.validateAuthorization({ urls, request: authorization(output.clientId + "x") }), "Rust tampered MCP client ID");
    yield* rejected(verifyRequestDpopProof({ request: request(output.proof), expectedAccessToken: output.session }), "Rust-spent DPoP proof");
    yield* verifyRequestDpopProof({ request: request(output.tsSpends), expectedAccessToken: output.session });
    const ts = JSON.parse(yield* Effect.promise(() => Fs.readFile(Path.join(base, "typescript.json"), "utf8")));
    yield* rejected(auth.authenticateMcpClient(mcpRequest(ts.mcp)), "Rust-revoked TypeScript MCP token");
    yield* rejected(verifyRequestDpopProof({ request: request(ts.rustSpends), expectedAccessToken: ts.session }), "Rust-spent TypeScript DPoP proof");
    check(output.desktop === (yield* currentDesktopBootstrapToken("desktop-secret", output.now)), "Wrong Rust desktop token");
    console.log("TypeScript accepted Rust credentials and rejected tampering, expiry, revocation, single-use reuse and shared DPoP replays.");
  }
}).pipe(Effect.provide(services), Effect.scoped));
