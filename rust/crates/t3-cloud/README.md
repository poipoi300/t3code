# t3-cloud

Native, transport-neutral environment-side connectivity to the existing T3
Connect relay. The deployed Worker is unchanged. This crate does not target
`wasm32-unknown-unknown`.

## API and host integration

### Server route seams (round 2)

Transports decode the contracts and build `api::RequestContext` from a verified
session, then call one method below. `api::reply` supplies no-store response data
and `HttpFailure::from` maps typed failures without exposing causes or credentials.
Construct `api::CloudService` with `CloudServices`; native connector ownership is
`managed::NativeManagedEndpoint`. No dependency on `t3-server` is introduced.

| Route or RPC | Service method | Access / composition |
|---|---|---|
| POST `/api/connect/link-proof` | `CloudService::link_proof` | relay:write; original request authority, no forwarded authority |
| POST `/api/connect/relay-config` | `CloudService::relay_config` | relay:write; persists trust, registers/recovers, starts connector, confirms origin |
| GET `/api/connect/link-state` | `CloudService::link_state` | relay:read; includes linkableHere, revocation, hostname, latest reachability |
| POST `/api/connect/reachability` | `CloudService::check_reachability` | relay:read; injected native/fake network probe |
| POST `/api/connect/unlink` | `CloudService::unlink` | relay:write; stops owned connector and clears trust/preferences |
| POST `/api/connect/preferences` | `CloudService::preferences` | relay:write; omitted hold preference does not contact relay |
| POST `/api/t3-connect/health` | `CloudService::health` | signed relay proof, no ordinary client session |
| POST `/api/t3-connect/mint-credential` | `CloudService::mint_credential` | signed relay proof + auth-backed CredentialIssuer |
| GET `/oauth/mcp/authorize` hosted decision | `CloudService::approval_redirect` | call only after MCP client/redirect validation; peer clients stay local |
| guest Connect hostname | `CloudService::state` → tunnelHostname | inject into GuestThreads; guest content/storage are owned by t3-host |
| `peerLinks.list`, `peerLinks.link`, `peerLinks.unlink` | `PeerLinks::{list,link,unlink}` | host RPC guard access:read / access:write |

Voice is independent of CloudLink: `t3-textgen::CloudSpeech::{configured,
set_key,models,transcribe}` and `SpeechService` already own the voice routes.
Their credential adapter belongs to the host vault/settings integration. Do not
duplicate speech or t3-host's `GuestShareContent` here.

- `cloud::CloudLink`: link proofs, install/unlink, registration, tunnel recovery,
  confirmed-origin fallback, preferences, health/mint responses, signed activity
  publication, held-hook wake and delivery-proof verification.
  `register_account` requests a new challenge and installs the relay's returned
  environment credential; repeating it rotates that credential. The relay has no
  independent environment-bearer refresh endpoint.
- `oauth::TokenManager`: persisted CLI access/refresh tokens, serialized early
  refresh, cancellation-safe rotation and permanent/transient rejection handling.
- `tunnel::start_connector`: supervise a supplied cloudflared executable through
  `t3-process`, watch readiness/configuration and classify connection/recovery
  events without exposing connector output. `ConnectorHandle::stop` joins cleanup;
  dropping the handle cancels its owned child. The host resolves a trusted,
  pinned executable and handles `RecoveryRequired` by recovering/restarting it.
- `tunnel::Ingress::channel`: bounded stream of `AcceptedConnection` values with
  `AsyncRead + AsyncWrite` I/O and authenticated principals. Cloudflare transports
  normal HTTP/WS to the environment; there is no custom relay framing protocol.
  The HTTP server performs the ordinary WS upgrade, passes its application I/O
  here, and serves accepted streams with its ordinary RPC handler. A loopback
  connector address never bypasses authentication.
- `peer::{PeerLinks, PeerMcpClient}`: durable peer links, PKCE MCP authorization,
  selected-machine identity checks, capability fencing, address failover,
  session renewal, caller mode/sender propagation, scoped retry IDs and bounded
  forwarded wait chunks. Linked callers cannot forward into another link.
- `guest::GuestSharing`: invite/session MACs and live authorization checks over a
  `GuestThreads` adapter. That adapter must atomically enforce redemption,
  pending-message limits and share identity/expiry at command commit, project the
  guest view and end replaced shares. `classify_uploads` validates upload metadata.
- `direct`: LAN endpoint construction, Tailscale discovery/serve parsing and
  commands through `t3-process`; `retry` supplies relay backoff/cooldown policies.
- `http::Transport`, `store::Secrets`, `Clock` and the guest/auth interfaces are
  injectable boundaries. `ReqwestTransport` bounds response bodies and disables
  redirects. Its errors omit URLs, headers, bodies and nested transport causes.

Enable `auth` to use `t3-auth::ServerSecretStore` and `EnvironmentAuth` directly.
The required auth APIs are atomic secret creation, standard-scope single-use
two-minute pairing grants bound to the client's P-256 thumbprint, and ordinary
RPC WebSocket authentication (cookies, bearer/DPoP, tickets, policy and revocation).
DPoP verification itself remains in `t3-auth`; it is not reimplemented here.

The host authenticates and authorizes link/config/preferences RPCs, applies
managed-policy gates and exposes the contracts' HTTP/error envelopes. It owns
startup reconciliation, the first-registration fleet jitter, replay-marker
pruning, activity projection/catch-up scheduling and debounced reconnect hook
wakes. Retry closures must rerun operations so proofs and challenges are fresh.

## Credential and proof lifecycle

All storage names and encoded bytes retain their TypeScript representations.
Production storage must be private and implement `Secrets::create` atomically.
`Secret`, `Sensitive`, environment keys and HTTP diagnostics redact Debug output;
serialization intentionally exposes values only at storage/transport boundaries.

| Credential/proof | Generation and storage | Verification and expiry |
|---|---|---|
| Environment Ed25519 identity | OS random 32-byte seed, PKCS#8 private PEM and SPKI public PEM in JSON `cloud-link-ed25519-key-pair`; migrate the old private/public secret pair, rereading the winner of creation races. | Private parsing is lazy. EdDSA signatures use the matching SPKI public key. Durable until explicitly replaced; unlink retains identity. |
| Link proof (`t3-env-link+jwt`) | EdDSA with environment identity; carries challenge, descriptor, public key, endpoint, local origin and scopes. Uses challenge `iat` when present, otherwise local injected clock; `exp = iat + 300`. Not persisted. | Existing relay authenticates/spends its challenge and verifies environment signature, identity, capabilities and lifetime. Local generation checks loopback target and request port; client entry point refuses forwarded authority and restricts non-desktop requests to loopback. |
| Environment bearer | Opaque credential issued by relay account linking; stored as `cloud-relay-environment-credential` alongside relay URL/issuer, user and cloud mint SPKI key. | Relay verifies it. Re-link replaces it; unlink removes it. A current registration's 401 marks `cloud-link-revoked`; unreachable relay retains it. Stale responses cannot revoke/overwrite a newer installation. No locally invented expiration. |
| Connector token | Relay-issued `cloud-endpoint-runtime-config` JSON. Passed only as `TUNNEL_TOKEN`, never argv. | Cloudflare validates it. Tunnel recovery replaces runtime; release/unlink removes it. Confirmed-origin reuse requires exact runtime and origin. Raw child output never becomes a diagnostic. |
| Register/recover proof (`t3-env-managed-tunnel-recovery+jwt`) | Environment EdDSA; environment issuer/subject, relay audience, random jti, action, account, origin and registration tunnel ID; 60-second lifetime. Not persisted. | Existing relay checks signature and bindings alongside environment bearer. Retries create fresh proofs. |
| Relay health/mint requests (`t3-cloud-health+jwt`, `t3-cloud-mint+jwt`) | Issued by existing relay with its Ed25519 key. | Verify installed cloud SPKI key, exact type/EdDSA, normalized relay issuer, environment audience/id, linked account and exact scope. Require positive lifetime no longer than 300 s, `iat` at most 60 s ahead, age at most 300+60 s, and expiration with 60 s tolerance. Mint additionally requires matching `cnf.jkt` and client thumbprint. Atomically spend both jti and nonce under TS `cloud-{health,mint}-{jti,nonce}-*` names; host uses auth's pruning job. |
| Minted pairing grant / signed mint response (`t3-env-mint+jwt`) | Auth issues a single-use standard-client grant, subject `cloud-connect`, label `T3 Connect connect`, bound to the client thumbprint. Environment EdDSA response binds credential, request nonce, environment and client key. | Grant expires within 120 s; auth verifies/spends it during normal bootstrap and verifies the client's DPoP. Relay verifies environment signature, nonce and client binding. No grant is stored by this crate. |
| Signed health response (`t3-env-health+jwt`) | Environment EdDSA response with nonce, descriptor, online status and checked-at time; 300 s lifetime. | Existing relay verifies linked environment key and request binding. Not persisted. |
| Activity proof (`t3-env-activity+jwt`) | Environment EdDSA, random jti, environment/thread identity and full state or null tombstone; 300 s lifetime; environment bearer accompanies it. | Existing relay verifies signature/state identity. Publication requires the local opt-in flag. Failure details are redacted and details bounded before signing. |
| Held-hook delivery (`t3-relay-hook-delivery+jwt`) | Existing relay signs queued delivery metadata. | Installed cloud key, relay issuer, environment audience/id, hook/delivery IDs and exact original received-at header; max age 25 h with 60 s tolerance. Metadata proof never replaces normal hook token/provider-signature checks. Relay owns offline queue persistence/forwarding. |
| CLI OAuth credentials | Caller supplies initial authorization tokens; camelCase JSON `cloud-cli-oauth-token`. Refresh uses form encoding, refresh token and client ID; rotated refresh/access values persist together even after requester cancellation. | Refresh five minutes before local expiry; preserve optional account identity. Permanent 4xx (except 408/429) requires login, leaves stored token available for explicit removal. HTTPS endpoint only, no redirects. |
| Peer MCP token | Dynamic MCP registration and random PKCE verifier/challenge; pairing code goes only to the selected peer's decision endpoint. Validate callback origin/path/state; exchange once. Token bytes live in a random `peer-link-*` secret; SQLite stores only the name and metadata. | Probe expected environment/capabilities before disclosing a code; re-probe identity and mode/fence support before use. Expiry comes from returned lifetime; unfenced legacy rows expire at linked-at. Peer verifies bearer and applies link/caller mode ceilings. Unlink deletes row and secret. |
| Guest signing key | OS random 32 bytes, atomic `guest-share-signing-key`; existing nonempty legacy keys retain their bytes. | HMAC-SHA256; empty stored key is rejected. Key persists across restarts. |
| Guest invite/session | Random 16-byte share ID. Invite is `shareId.base64url(HMAC("guest-invite:" + shareId)[0..16])`; session is base64url JSON `{kind:"session",threadId,shareId}` plus full MAC. Nothing secret in thread projection. | Constant-time MAC comparison, exact two segments. Every request checks current share ID, 24 h share expiry, enabled flag, archive/delete and serving hostname; session also requires redemption. Atomic command spends invite once. Replacement/closure revokes all tokens; streams recheck at most every 30 s plus each backend event. |

## Port coverage and remaining integration

Implemented service behavior follows `apps/server/src/cloud/{CloudLink,
environmentKeys,CliState,CliTokenManager,managedTunnelStartup,relayResponse}.ts`,
`cloud/ManagedEndpointRuntime.ts`, `relay/relayEnvironmentClient.ts`, the signed
publication boundary in `relay/AgentAwarenessRelay.ts`, held-hook wake/verification,
`peer/{PeerLinks,PeerMcpClient,PeerForwarding}.ts`, the token/authorization core of
`guest/GuestShareService.ts`, `guest/guestAttachments.ts`, and `packages/tailscale`.

Not implemented in this crate: cloud CLI browser/device login presentation,
OS boot-service install/update/launcher and cloudflared download/pinning
(`cloud/{bootService,pinnedRuntime,selfUpdate,...}.ts`); full reachability/UI status
aggregation (`cloud/ConnectReachability.ts`); orchestration-backed link cards and
remote task followers (`peer/{PeerLinkRequests,RemoteDelegation}.ts`); activity
event projection, deduplication, completion confirmation and retry workers
(`relay/AgentAwarenessRelay.ts`); guest snapshot/history/item/render/media/file
authorization/storage and HTTP/multipart/cookie adapters
(`guest/{GuestShareService,guestView,http}.ts`). These require the host's
orchestration, attachment and transport services; they are not silently stubbed.
The relay Worker, push services and queue are intentionally not rewritten.

Known differences: service errors are redacted Rust enums, not Effect causes;
peer error messages omit untrusted remote text while retaining recognized failure
codes. JSON dates use Chrono's RFC3339 range. JWT claim integers and CLI OAuth
refresh lifetimes reject integral decimal/exponent JSON syntax. Peer token
lifetimes use the contracts' finite `JsNumber` and require positive safe integers,
including integral decimal/exponent values. Fractional lifetimes are not supported.
Base64url decoding is
stricter than Node's guest-token decoder (no padding, alternate alphabet or
noncanonical trailing bits). Replay IDs accept only filesystem-safe ASCII
alphanumeric, hyphen and underscore values up to 512 bytes; relay UUIDs fit.
Rust truncation cannot emit lone UTF-16 surrogates; activity details stay within
160 UTF-16 units, guest filenames are capped at 255 Unicode scalar values rather
than TS's 255 UTF-16 units. Request/response size limits are explicit. Peer
origins with embedded userinfo are rejected instead of discarding it. Only
HTTPS relay origins are accepted; fake relay tests inject the
transport. LAN addresses are supplied by the host rather than enumerated here.
Peer MCP tool exchanges have a 65-second request timeout (long waits are split
into 50-second chunks); TS bounds the handshake but otherwise leaves tool calls
open. Peer listings currently probe sequentially with the normal 3/10-second
address timeouts rather than TS's shorter concurrent listing probes.
No new shared wire types need moving to `t3-contracts`: local structs are service
options/results, I/O boundaries or private persisted token/session helpers.

## Verification

Tests use an in-process scripted relay, temporary SQLite/secret directories,
virtual clocks and an ephemeral loopback HTTP fixture. They never access a real
relay, credentials, Tailscale daemon or live T3 state. `auth` adds a real auth
integration test. `tools/interop.ts` and `examples/interop.rs` exercise both JWT
directions with the actual TypeScript relay helper and temporary synthetic keys.
Windows is the verification platform; native POSIX connector behavior is unverified.
Run the probe with `tools/interop.ps1` after setting `CARGO_TARGET_DIR`; it removes
its temporary synthetic material on completion.

Set `CARGO_TARGET_DIR` to the assigned build directory for every command, then
run from `rust/`: `cargo fmt -p t3-cloud`,
`cargo clippy -p t3-cloud --all-targets --features auth -- -D warnings`, and
`cargo test -p t3-cloud --features auth`.

Verified on Windows, 2026-11-13: scoped format and all-target Clippy with default
features and `auth`, both with `-D warnings`, pass; 40 default-feature tests and
41 tests with `auth` pass, including real owned-child shutdown and real auth
integration. The bidirectional TypeScript/Rust JWT probe
passes. No real relay, Tailscale daemon or live credentials were used.

The inherited baseline had 38 passing default-feature tests. Regression checks
first reproduced empty guest-key acceptance, lost peer refusal codes and raw
failed-run detail publication; each passes after its fix. Full host integration
and native POSIX runtime verification remain as described above.
