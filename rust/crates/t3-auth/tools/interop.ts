import * as Fs from "node:fs/promises";
import * as Path from "node:path";
import * as Effect from "../../../../apps/server/node_modules/effect/dist/Effect.js";
import * as Layer from "../../../../apps/server/node_modules/effect/dist/Layer.js";
import * as Duration from "../../../../apps/server/node_modules/effect/dist/Duration.js";
import * as Redacted from "../../../../apps/server/node_modules/effect/dist/Redacted.js";
import * as DateTime from "../../../../apps/server/node_modules/effect/dist/DateTime.js";
import * as Option from "../../../../apps/server/node_modules/effect/dist/Option.js";
import { createHash } from "node:crypto";
import { isDeepStrictEqual } from "node:util";
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
import { connectDeviceVerificationCode } from "../../../../packages/contracts/src/auth.ts";
import { verifyDpopProof } from "../../../../packages/shared/src/dpop.ts";

const p256Order = BigInt("0xffffffff00000000ffffffffffffffffbce6faada7179e84f3b9cac2fc632551");
const signatureVariant = (proof: string, high: boolean) => {
  const parts = proof.split(".");
  const signature = Buffer.from(parts[2]!, "base64url");
  const s = BigInt(`0x${signature.subarray(32).toString("hex")}`);
  const low = s > p256Order / 2n ? p256Order - s : s;
  const chosen = high ? p256Order - low : low;
  return `${parts[0]}.${parts[1]}.${Buffer.concat([signature.subarray(0,32),Buffer.from(chosen.toString(16).padStart(64,"0"),"hex")]).toString("base64url")}`;
};

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
    const clientMetadata = { label: "TS Connect device", deviceType: "desktop" as const, os: "Windows" };
    const pendingKey = "TS-pending-key-for-interop";
    const deniedKey = "TS-denied-key-for-interop";
    const trustedKey = "TS-trusted-key-for-interop";
    const pending = yield* auth.issueConnectCredential({ proofKeyThumbprint: pendingKey, requireApproval: true });
    yield* rejected(auth.exchangeBootstrapCredentialForAccessToken(pending.credential, undefined, clientMetadata, { proofKeyThumbprint: pendingKey }), "TS unapproved Connect request");
    const pendingLink = (yield* pairing.listActive()).find((link) => link.connectDeviceRequest?.verificationCode === connectDeviceVerificationCode(pendingKey));
    check(pendingLink !== undefined, "TS pending link missing");
    const denied = yield* auth.issueConnectCredential({ proofKeyThumbprint: deniedKey, requireApproval: true });
    const deniedLink = (yield* pairing.listActive()).find((link) => link.connectDeviceRequest?.verificationCode === connectDeviceVerificationCode(deniedKey));
    yield* pairing.revoke(deniedLink!.id);
    yield* pairing.trustConnectDevice(trustedKey, clientMetadata);
    const trustedAt = (yield* pairing.listConnectDevices()).find((device) => device.proofKeyThumbprint === trustedKey)!.approvedAt;
    const bound = yield* sessions.issue({ proofKeyThumbprint: trustedKey, method: "dpop-access-token" });
    const boundExpired = yield* sessions.issue({ proofKeyThumbprint: trustedKey, ttl: Duration.millis(-1) });
    const unspent = yield* auth.issueConnectCredential({ proofKeyThumbprint: trustedKey, requireApproval: true });
    const verificationCodes = ["", "A", "ABCDEFGH", "abcdefghijklmnopqrstuvwxyz", "0123456789-_", "!é😀abcdZ", "abcdefg😀", "Kq3vZ8sYbTn0"].map((value) => [value, connectDeviceVerificationCode(value)]);
    const connect = { verificationCodes, pending: { ...pending, expiresAt: DateTime.formatIso(pending.expiresAt) }, pendingId: pendingLink!.id, pendingKey, denied: { ...denied, expiresAt: DateTime.formatIso(denied.expiresAt) }, deniedKey, trustedKey, trustedAt: DateTime.formatIso(trustedAt), bound: bound.token, boundExpired: boundExpired.token, boundId: bound.sessionId, expiredId: boundExpired.sessionId, unspent, clientMetadata };
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
    const proof = signatureVariant(key.sign({ method: "GET", url: "https://example.test/ws", token: issued.token, iat: Math.floor(Date.now() / 1000) }), false);
    const rustSpends = signatureVariant(key.sign({ method: "GET", url: "https://example.test/ws", token: issued.token, iat: Math.floor(Date.now() / 1000) }), false);
    const highProof = signatureVariant(proof, true);
    check(verifyDpopProof({proof: highProof, method:"GET",url:"https://example.test/ws",expectedAccessToken:issued.token,nowEpochSeconds:Math.floor(Date.now()/1000)}).ok, "Installed TypeScript high-S policy changed; review Rust's intentional strictness");
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
    yield* Effect.promise(() => Fs.writeFile(Path.join(base, "typescript.json"), JSON.stringify({ highProof, connect, mcp: mcp.access_token, mcpBound, session: issued.token, flags: flags.token, ticket: ticket.token, expired: expired.token, revoked: revoked.token, expiredTicket: expiredTicket.token, revokedTicket: revokedTicket.token, pairing: pairingCode.credential, expiredPairing: expiredPairing.credential, revokedPairing: revokedPairing.credential, approval: approval.credential, proof, rustSpends, thumbprint: key.thumbprint, clientId: client.clientId, desktop })));
    console.log("TypeScript OAuth issued an audience-less MCP token; real PayloadSigner also signed an explicitly extended audience fixture.");
    console.log("TypeScript prepared positive and negative credentials and spent a DPoP proof.");
  } else {
    const output = JSON.parse(yield* Effect.promise(() => Fs.readFile(Path.join(base, "rust.json"), "utf8")));
    if (phase === "finish") {
      yield* rejected(verifyRequestDpopProof({ request: request(output.tsSpends), expectedAccessToken: output.session }), "TypeScript proof replay after Rust recheck");
      console.log("TypeScript confirmed persistent replay rejection after both processes reopened.");
      return;
    }
    const connect = output.connect;
    const pending = (yield* pairing.listActive()).find((link) => link.id === connect.pending.id);
    check(pending?.connectDeviceRequest?.verificationCode === connectDeviceVerificationCode(connect.pendingKey), "Rust verification code differs");
    check(isDeepStrictEqual(pending?.connectDeviceRequest?.client, connect.clientMetadata), "Rust requested client JSON differs");
    yield* rejected(pairing.consume(connect.pending.credential, { proofKeyThumbprint: "wrong-key" }), "Rust pending request with wrong key");
    yield* rejected(pairing.consume(connect.pending.credential, { proofKeyThumbprint: connect.pendingKey }), "Rust pending request without approval");
    check(yield* auth.approveConnectDevice(connect.pending.id), "Cannot approve Rust pending row");
    yield* pairing.consume(connect.pending.credential, { proofKeyThumbprint: connect.pendingKey });
    yield* rejected(pairing.consume(connect.pending.credential, { proofKeyThumbprint: connect.pendingKey }), "Rust consumed Connect request");
    const denied = yield* auth.issueConnectCredential({ proofKeyThumbprint: connect.deniedKey, requireApproval: true });
    check(denied.credential === connect.denied.credential, "Rust denial was not reused");
    yield* rejected(pairing.consume(denied.credential, { proofKeyThumbprint: connect.deniedKey }), "Rust denied Connect request");
    check(!(yield* auth.approveConnectDevice(connect.denied.id)), "Rust denied row was approved");
    const trusted = (yield* auth.listConnectDevices()).find((device) => device.proofKeyThumbprint === connect.trustedKey);
    check(trusted !== undefined && DateTime.formatIso(trusted.approvedAt) === connect.trustedAt, "Rust trusted row/date differs");
    check(isDeepStrictEqual(trusted!.client, connect.clientMetadata), "Rust trusted client JSON differs");
    yield* pairing.trustConnectDevice(connect.trustedKey, { ...connect.clientMetadata, label: "TS refreshed Rust device" });
    check(DateTime.formatIso((yield* auth.listConnectDevices()).find((device) => device.proofKeyThumbprint === connect.trustedKey)!.approvedAt) === connect.trustedAt, "TS replaced first approval time");
    check(Option.getOrNull(yield* sessions.proofKeyThumbprintOf(connect.boundId)) === connect.trustedKey, "TS cannot read Rust session proof key");
    check((yield* sessions.revokeByProofKey(connect.trustedKey)) === 2, "TS did not revoke active and expired Rust key sessions");
    check(yield* auth.revokeConnectDevice(connect.trustedKey), "TS did not remove Rust trust");
    yield* rejected(pairing.consume(connect.unspent.credential, { proofKeyThumbprint: connect.trustedKey }), "Rust credential after TS device revocation");
    console.log("TypeScript read Rust Connect rows, refused wrong/unapproved/spent/denied credentials, approved a request, preserved trust dates and revoked key sessions plus unspent credentials.");
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
