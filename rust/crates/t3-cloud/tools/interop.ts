import { generateKeyPairSync } from "node:crypto";
import { readFile, writeFile } from "node:fs/promises";
import { createRequire } from "node:module";
import { join } from "node:path";
import { pathToFileURL } from "node:url";
import { signRelayJwt, verifyRelayJwt, RELAY_HEALTH_REQUEST_TYP, RELAY_HEALTH_RESPONSE_TYP } from "../../../../packages/shared/src/relayJwt.ts";

const require = createRequire(new URL("../../../../apps/server/package.json", import.meta.url));
const Effect = await import(pathToFileURL(require.resolve("effect/Effect")).href);
const Schema = await import(pathToFileURL(require.resolve("effect/Schema")).href);
const NodeCrypto = await import(pathToFileURL(require.resolve("@effect/platform-node/NodeCrypto")).href);
const [mode, directory] = process.argv.slice(2);
if (!directory || !["prepare", "verify"].includes(mode)) throw new Error("Expected prepare|verify and a temporary directory");

if (mode === "prepare") {
  const { privateKey, publicKey } = generateKeyPairSync("ed25519");
  const now = Math.floor(Date.now() / 1000);
  const claims = { iss: "https://interop.test", aud: "t3-env:interop", sub: "synthetic-user", jti: "interop-jti", iat: now, exp: now + 300, environmentId: "interop", nonce: "interop-nonce", scope: ["environment:status"] };
  const proof = await Effect.runPromise(signRelayJwt({ privateKey: privateKey.export({ type: "pkcs8", format: "pem" }).toString(), typ: RELAY_HEALTH_REQUEST_TYP, payload: claims }).pipe(Effect.provide(NodeCrypto.layer)));
  await writeFile(join(directory, "typescript.json"), JSON.stringify({ now, publicKey: publicKey.export({ type: "spki", format: "pem" }).toString(), proof }));
  console.log("TypeScript signed a synthetic relay health request.");
} else {
  const { publicKey, proof } = JSON.parse(await readFile(join(directory, "rust.json"), "utf8"));
  const claims = await Effect.runPromise(verifyRelayJwt(Schema.Struct({ requestNonce: Schema.String, status: Schema.Literal("online") }), { publicKey, token: proof, typ: RELAY_HEALTH_RESPONSE_TYP, issuer: "t3-env:interop", audience: "https://interop.test" }).pipe(Effect.provide(NodeCrypto.layer)));
  if (claims.requestNonce !== "interop-nonce") throw new Error("Incorrect nonce binding");
  console.log("TypeScript verified the Rust health response and nonce.");
}
