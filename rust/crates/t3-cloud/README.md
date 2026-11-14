# t3-cloud

Native, transport-neutral environment-side connectivity to the existing T3
Connect relay. The deployed Worker is unchanged. This crate does not target
`wasm32-unknown-unknown`.

## API and host integration

### Server route seams (round 2)

Transports decode the contracts and build `api::RequestContext` from a verified
session, then call one method below. `api::reply` supplies no-store response data
and `HttpFailure::from` maps typed failures without exposing causes or credentials.
Enable the `auth` feature and construct `api::CloudService` with `CloudServices`; native connector ownership is
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
| POST `/api/t3-connect/mint-credential` | `CloudService::mint_credential` | signed relay proof + concrete AuthCredentialIssuer |
| GET `/oauth/mcp/authorize` hosted decision | `CloudService::approval_redirect` | call only after MCP client/redirect validation; peer clients stay local |
| guest Connect hostname | `CloudService::state` → tunnelHostname | inject into GuestThreads; guest content/storage are owned by t3-host |
| `cloud.getRelayClientStatus` | `RelayClient::status` | relay:read |
| `cloud.installRelayClient` stream | `RelayClient::install_with_progress` | relay:write; canonical progress/completion/error stream |
| `peerLinks.list` | `PeerService::list` | access:read; returns `{links}` |
| `peerLinks.link` | `PeerService::link` | access:write; pairing input wrapped in Sensitive |
| `peerLinks.unlink` | `PeerService::unlink` | access:write; canonical `{removed}` result |
| `peerLinks.answerRequest` | `PeerService::answer_request` | access:write; records outcome through orchestration |

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

Enable `auth` and construct `cloud::AuthCredentialIssuer::new(shared_auth.clone(),
shared_settings.clone())` and put it in an `Arc` for `CloudServices.issuer`. Both
`CloudServices` and `CloudService`, as well as `CloudLink::mint`, are available only
with `auth`. They take the concrete issuer; the injectable `CredentialIssuer` trait
was removed, including its test implementations. Tests use the same real-auth
constructor as the host. A compile-pass doc test pins the concrete field type and
a compile-fail test rejects an unrelated issuer. The adapter reads the current
`SettingsService::snapshot()` on every mint and calls
`EnvironmentAuth::issue_connect_credential`, which owns `connect_trust_lock` shared
with exchange/revoke and finishes mutations after cancellation. Do not construct
per-request auth instances or hand-build pairing grants. Keep the settings watcher
running as for the rest of the server. Pending credentials still go back to the
relay as signed responses, exactly as TypeScript does; auth refuses their exchange
until approval. Denied requests reuse the same unusable credential until expiry.
The ordinary RPC WebSocket auth adapter handles cookies, bearer/DPoP and tickets.

Construct `relay_client::RelayClient` once with `RelayClientOptions` derived from
host paths/platform (`win32`/`darwin`/`linux`, `x64`/`arm64`), the configured
executable override, split PATH directories, `pinned_asset(platform, arch)`, and a
live managed-install policy watch. Use `NativeDownload` and `NativeProbe` in
production; tests replace both. `NativeManagedEndpoint::with_relay_client` resolves
the executable when a connector starts, so installing after server startup works.
Installation checks the upstream pin and SHA256 before execution/activation,
uses a process-owned cross-process lock, and never falls back past an override.
Drop the install stream to cancel, or call `shutdown()` to cancel and join.

Enable `orchestration` for `OrchestrationStore::new(management, projections,
repository_adapter, event_committer)`. It returns the shared store and an owned
`EventForwarder`; retain and shut down the latter. The repository adapter supplies
the project's canonical repository key. Construct `PeerLinkRequests::new(links,
store, clock)` and `api::PeerService::new(links, requests)`. Remote delegation uses
`RemoteDelegation::new(environment_id, store, PeerMcpClient, links)`; retain its
`start()` runner and shut it down. Task reads are narrow; restart scans use the
projection's outstanding remote tasks. Lag requests a rescan rather than inventing
a task ID. Three consecutive follower defects fail unfinished tasks with a generic
warning; ordinary transport errors/success reset the streak. Published results and
remote stop obligations are preserved. Keep `PeerLinks.enabled` synchronized with
the live linked-environments setting.
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
| Minted pairing grant / signed mint response (`t3-env-mint+jwt`) | Auth issues a single-use standard-client grant, subject `cloud-connect`, label `T3 Connect`, bound to the client thumbprint. Environment EdDSA response binds credential, request nonce, environment and client key. | Trusted/approval-disabled grants expire within 120 s; untrusted approval requests expire within 600 s and cannot be spent until approved. Auth verifies/spends it during normal bootstrap and verifies the client's DPoP. Relay verifies environment signature, nonce and client binding. No grant is stored by this crate. |
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

Not implemented in this crate: cloud CLI browser/device login presentation and
OS boot-service install/update (`cloud/{bootService,pinnedRuntime,selfUpdate,...}.ts`);
automatic reachability restart/background aggregation (`cloud/ConnectReachability.ts`);
activity event projection, deduplication, completion confirmation and retry workers
(`relay/AgentAwarenessRelay.ts`). The signed publication boundary is implemented,
but its worker still needs the authoritative awareness projection/feed. Guest
content/history/media/storage are supplied by `t3-host`; HTTP cookie/multipart/SSE
composition belongs to `t3-server`. Native launcher supervision is owned by
`t3-host::service_launcher`; this crate does not duplicate it. Cloudflared download,
pinning, status and install streams, link cards and remote task followers are now
implemented. The upstream boot reset-failed delta and compact-visit awareness-worker
classification cannot be attached to a nonexistent local boot/awareness worker;
these remain explicit coverage gaps, not no-op matches.
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
open. Peer listings race addresses with a three-second deadline and retain the row order.
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

Verified after the re-review follow-up on Windows: `cargo fmt -p t3-cloud`,
all-target Clippy with both default features and `auth,orchestration` and
`-D warnings`, 50 default-feature tests, 61 tests with `auth`, and two auth doc
tests (compile-pass/compile-fail) pass. Connect service/mint tests now require
`auth`, matching the production API, and use the real issuer instead of stand-ins.
All cargo commands used `D:\t3code-rust-target\cloud`, two jobs and sccache; no
wrapper fallback or machine-memory retry was needed. No commits were made.
The inherited baseline had 38 passing default-feature tests. Earlier regression
checks reproduced empty guest-key acceptance, lost peer refusal codes and raw
failed-run detail publication. The Connect regression first reproduced immediate
acceptance of an untrusted grant with approval enabled, then passed through the
real signed cloud mint and real auth store after the fix. `tests/auth_adapter.rs`
covers pending refusal, denial reuse/expiry, trusted mint, corrected label, live
setting changes, binding, one-use consumption, and RPC tickets.

JWT/cnf audit against current `CloudLink.ts`, `linkChecks.ts` and
`packages/shared/src/relayJwt.ts`: no omitted trust check found. EdDSA/type,
installed issuer/key, audience/environment/account, exact scope, bounded lifetime,
clock tolerance/max age, matching `cnf.jkt` and both persistent replay guards are
checked before auth issuance. `tests/protocol.rs` rejects independently signed
wrong-account/environment/audience/scope/clock/key/cnf/type requests before consuming
replay guards. Rust is stricter on numeric JSON syntax, JWT type spelling,
critical/b64 headers and filesystem-safe replay IDs as noted above; valid relay
proofs match. Mint response expiry passes auth's answer through without comparing it against the
earlier relay-proof clock. Auth owns the two/ten-minute issuance lifetimes; replay
storage and settings/DB work can advance its issue time past proof verification.
A one-millisecond positive skew first reproduced rejection; separate clocks ahead
and behind now return the auth expiry and still refuse unapproved consumption. No tokens or credentials enter Debug,
errors or application logs.


## Approval re-review follow-up

The server must depend directly on `t3-cloud` with `features = ["auth"]`
(and `"orchestration"` for the native peer store). It cannot get Connect mint
composition from a default-feature dependency. `t3-host` currently imports only
`t3-cloud::guest` and guest/token primitives: its dependency without `auth` is
correct for that responsibility. Do not add a second auth service there.

Additional coverage uses real auth, SQLite and synthetic P-256 DPoP signatures:
unapproved/missing-proof/wrong-key bootstrap is refused, approval permits the
bound one-use session and remembers the key, then a trusted mint has a two-minute
expiry. An actual settings I/O failure produces a redacted internal failure and
no auth grant, and a repaired read resumes approval. Managed policy denial runs
before replay consumption, remains live when changed, and leaves state readable.
