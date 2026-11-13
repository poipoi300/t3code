# t3-auth

Native, transport-neutral authentication over an injected `t3_persistence::Database`
and explicit state/secrets paths. Never opens a default T3 home. Wire types come
from `t3-contracts::auth`.

## Source credential inventory (recorded before implementation)

The TypeScript checkout is the specification, not the older inventory notes.

| Credential | Generation/format | Storage, comparison, expiry |
|---|---|---|
| Session cookie / bearer | UUIDv4 sid; `base64url(JSON).base64url(HMAC-SHA256(payload))`; claims `v:1,kind:"session",sid,sub,scopes,method,jkt?,rtc?,plk?,iat,exp`, milliseconds, accepts v1/v2 | `auth_sessions`: session_id,subject,scopes (JSON),method,client_label,client_ip_address,client_user_agent,client_device_type,client_os,client_browser,issued_at,expires_at,last_connected_at,revoked_at. Default 30 days; DPoP exchange 1 hour; constant-time MAC verification. Effective scopes come from the row. |
| WebSocket ticket | Same MAC format; `v:1,kind:"websocket",sid,aud?,jkt?,iat,exp` | Stateless, 5 minutes; checks parent row expiry/revocation; aud rpc/media; only explicit rpc tickets bind jkt. |
| Pairing / app approval | 12 rejection-sampled characters from `23456789ABCDEFGHJKLMNPQRSTUVWXYZ` (32 symbols, 60 bits); UUIDv4 id. App prefixes READ/SUPERVISED/EDITS/AUTO/FULL | Plaintext `auth_pairing_links`: id,credential,method,scopes,subject,label,proof_key_thumbprint,created_at,expires_at,consumed_at,revoked_at. Five minutes, dev startup 24 hours, custom TTL supported. Issued formats case-insensitive; other credentials exact. Atomic SQL update gates expiry, proof key, overlap/all-required scopes and approval subject. SQL/string comparison is not constant-time. |
| Legacy desktop bootstrap | Supplied by desktop; opaque exact token | In-memory map, reusable 24 hours, standard scopes; ordinary map equality. |
| Rotating desktop bootstrap | Lowercase hex HMAC-SHA256 of UTF-8 `t3-desktop-bootstrap:<floor(now/43200000)>`, key is supplied UTF-8 desktop secret | Accept previous/current/next 12-hour windows, constant-time UTF-8 comparison; reusable. Takes precedence over legacy seed. Secret supplied by caller, not created by auth. |
| Reusable dev credential | Exact configured T3CODE_DEV_AUTH_TOKEN, only web + dev URL + nonempty token | Constant-time SHA256 comparison; sid `dev-auth-<sha256 hex>`, cookie `t3_dev_session_<same hex>`. Row expires 9999-12-31T23:59:59.999Z; insert-if-absent preserves revocation. Child bearer/DPoP sessions have normal lifetimes. Rotation/removal rejects old raw token and dev tickets, leaves old rows visible. |
| Signing secrets | OS CSPRNG, raw 32 bytes | `<secretsDir>/server-signing-key.bin` and `mcp-oauth-signing-key.bin`. Directory 0700/files 0600 on POSIX. Atomic temp+hard-link create; atomic rename replace. No keychain, encryption, or rotation in this layer. Existing nonempty bytes are reused; empty keys return `Error::EmptySecret`. |
| MCP client id | base64url JSON `{v:1,n,r,p?}` + HMAC-SHA256 of `mcp-client-id.<body>` | Stateless signed registration, no expiry. Same MCP signing secret. Constant-time MAC check. |
| MCP CSRF | base64url HMAC-SHA256 over `mcp-csrf.` + JSON `[holder,clientId,redirectUri,codeChallenge]`; holder sid or `dpop:<jkt>` | Stateless, constant-time decoded-byte comparison, lifetime follows approving session. |
| MCP authorization code | 32 random bytes, base64url | In-memory map; 60 seconds, removed atomically before later checks, including failed PKCE. PKCE S256 (43..128 ASCII verifier); constant-time digest comparison. Client/redirect/resource strings use ordinary equality. |
| DPoP proof | ES256 compact JWT, public EC P-256 JWK, typ dpop+jwt, htm,htu,jti,iat,ath? | ±300 seconds inclusive, compact ECDSA, accepting high-S as the installed noble NIST configuration does. Public thumbprint/ath/method/url use ordinary equality. Replay marker `dpop-proof-<base64url SHA256(jkt+":"+jti)>.bin`, exclusive hard-link publication. Contents thumbprint/jti/iat/consumedAt lines. |
| Replay cleanup | dpop-proof-, cloud-mint-jti-, cloud-mint-nonce-, cloud-health-jti-, cloud-health-nonce- + *.bin | Remove only mtime strictly older than one day; temp files/other secrets untouched. Scheduler belongs to runtime. |

Sources: `apps/server/src/auth/{SessionStore,PairingGrantStore,ReusableDevAuth,PayloadSigner,ServerSecretStore,EnvironmentAuth,McpOAuth,dpop,replayMarkers}.ts`,
`apps/server/src/persistence/{AuthSessions,AuthPairingLinks}.ts`,
`packages/shared/src/{desktopBootstrapToken,dpop,dpopCommon}.ts`.

Cookies are unsigned session-token carriers: HttpOnly, Path=/, SameSite=Lax,
Expires, and Secure on HTTPS (including forwarded HTTPS). TypeScript omits Secure. Desktop name `t3_session_<port>`;
production remote web `t3_session_<first12 sha256(environmentId)>`; other web
`t3_session_<port>_<first12 sha256(stateDir)>`. Legacy `t3_session` accepted only
for production remote web, after primary cookie/bearer/DPoP selection. Dev cookie
is last and disabled by any Authorization header. Cookie authentication checks
Origin, same-origin Sec-Fetch-Site, and configured trusted origins. Forwarded host
is accepted only with `AuthConfig::trust_forwarded_host`.
Credential responses carry Cache-Control: no-store and Pragma: no-cache.
Pairing URL replaces path with `/pair`, removes query token, puts token in fragment.
Startup subject administrative-bootstrap is hidden; current source grants it
the **same standard scopes** as CLI codes, including access/relay write.

Remaining inherited behavior: plaintext short pairing codes have no rate limit;
legacy desktop tokens use map equality; signed session expiration remains authoritative
rather than row expiration (WS checks both). Signing keys are plaintext files without
automated rotation. Startup grants retain full standard scopes; legacy audience-less
WS tickets remain supported; reusable dev identifiers remain hashes of the credential.

## API and integration

`EnvironmentAuth::new(db, secrets, config, policy, clock)` assembles the stores.
The caller opens `Database` through `t3-persistence`, supplies the exact secrets
directory via `ServerSecretStore::open`, a decoded optional managed policy, and
`clock::SystemClock::new()` in production. `TokioClock::at` supports paused Tokio
time; tests also exercise a manually advanced `Clock` without wall-clock waits.

- `SessionStore`: issue/verify, short-lived WebSocket tickets, row lookup, active
  listing, atomic replacement/revocation, connection reference counts and last
  connected time, unbounded change subscriptions, and `watch_invalidation` /
  `await_invalidation` following a DPoP renewal chain. Clones share connectivity
  and renewal state; construct one store per running server.
- `PairingGrantStore`: issue/list/revoke/consume, seeded and rotating desktop
  bootstrap, hidden per-access MCP approval codes, and change subscriptions.
  Listing and event payloads omit credentials. SQL consumption is atomic across
  independent database handles, not merely within a Rust mutex.
- `EnvironmentAuth`: browser pairing and replacement, narrowed OAuth bootstrap
  exchange, CLI bearer issuance, startup recovery/fragment URLs, cookie/header
  selection, origin checks, authenticated state, HTTP/WS/MCP authentication,
  current-client marking and self-revocation protection, policy revocation.
- `authorization`: the 220-method scope table, dynamic settings/provider/project/
  asset/thread checks, typed `EnvironmentAuthorizationError`, delegation
  validation and `execute_rpc`. Device-list checks combine the middleware's
  read requirement with `apps/server/src/ws.ts`'s retry/update requirement.
  No server guard expands legacy scopes into finer permissions.
- `oauth::McpOAuth`: signed registration/discovery, verified redirect validation,
  approval details and CSRF, pairing/session approval, denial, atomic single-use
  code exchange, PKCE and peer-link flags. `dpop` exposes proof verification and
  persistent replay prevention. `secrets` exposes one-shot replay pruning;
  `utils` supplies metadata, cookie names, pairing URLs and internal-path checks.

`Request` carries transport data. Its `Headers` type normalizes names at insertion,
lookup and removal, and preserves duplicates during collection. Ambiguous duplicate
headers have an empty lookup value: presence still disables dev-cookie fallback and
ambiguous Origin/DPoP/Authorization values cannot authenticate. Collect raw header
pairs; an adapter must not discard duplicates before building this type.
`Cookie` contains every response attribute, including Max-Age=0 for the cookie expired
by dev login. `create_browser_session` takes the request to select Secure for all cookies.
Apply `CREDENTIAL_RESPONSE_HEADERS` and `dpop_challenge` at the HTTP adapter.

The RPC guard decodes every WS method using its `RpcMethod::Payload` from
`t3-contracts::rpc`. Non-object or undecodable payloads produce `InvalidPayload` before
any scope decision or handler execution. The six input-dependent rules use canonical
contract payloads; the git rules call the contract permission helper. The fixture test
walks all guarded methods, and positive dynamic tests serialize real contract types.
Pairing issuance always receives the caller's scopes and an explicit grant; delegation
is checked inside the issuing call. Missing scopes never imply standard scopes.
`exchange_bootstrap` takes the request, verifies its DPoP proof, validates/consumes the
bootstrap credential and scope grant, then records replay state before issuing a session.
Its caller cannot supply an unchecked proof thumbprint or compose replay recording.

Store mutations finish commit and change publication in a Tokio completion task
even if their awaiting request is cancelled: `t3-persistence` blocking closures
otherwise outlive that request and could leave revoked sockets unaware.

## Tests and interoperability

Behavior cases are consolidated into tables and integrated tests against real
SQLite/filesystem dependencies; no store, crypto, or process mocks are used.

| TypeScript tests under `apps/server/src/auth/` | Rust tests |
|---|---|
| `SessionStore.test.ts` | `tests/lifecycle.rs`: expiry, all revocation modes, proof-key renewal chain including revoking an earlier renewal and disconnecting another socket, metadata/connectivity, persisted dev isolation/rotation, claim authority, failed replacement rollback |
| `PairingGrantStore.test.ts` | `tests/lifecycle.rs`: lowercase/single use, eight racing independent connections, proof/scope rejection preserving codes, reusable exact/rotating desktop tokens and expiry, redacted lists/events |
| `ServerSecretStore.test.ts`, `replayMarkers.test.ts` | `tests/storage_utils.rs`: reuse, twelve concurrent creators, actual read/rename failures, pruning boundaries and protected secret names |
| `RpcAuthorization.test.ts` | `tests/authorization.rs`: every source declaration, sensitive RPCs, unary/stream entry guards, settings/providers/secrets, assets, prepared worktrees and thread-context PR operations |
| `EnvironmentAuth.test.ts`, `EnvironmentAuthAdmin.test.ts` | `tests/environment.rs`, `tests/dpop.rs`, `tests/oauth.rs`: dev fallback and rotation, scope narrowing and concurrent lower-level retries, startup grants, desktop rotation, current/revoke semantics, origins, bearer/DPoP schemes, proof-bound RPC/media/legacy tickets, replay, policy enforcement |
| `EnvironmentAuthPolicy.test.ts`, `utils.test.ts` | `tests/storage_utils.rs`: bind/policy/cookie matrix, metadata inference and presented identity |
| `http.test.ts` | `tests/environment.rs`: selected cookie attributes, dev cross-instance acceptance, normal-cookie expiry and DPoP challenge data; HTTP route registration itself belongs to the adapter |
| `McpOAuth.test.ts` | `tests/oauth.rs`, `tests/dpop.rs`: discovery, redirect/registration validation, hosted HTTPS callbacks/public PKCE, one-time MCP-only tokens, failed PKCE spending, scoped and app-specific approval codes, peer restriction, CSRF scope/holder binding and DPoP renewal; cloud handoff routing remains below |
| `dpop.test.ts` | `tests/dpop.rs`: real AlreadyExists replay mapping versus other filesystem errors |

Additional tests pin high-S signature acceptance, cancellation after SQLite
starts a revocation, corrupt persisted dates, and cookie/internal-path isolation.

From `rust/`, set `CARGO_TARGET_DIR` to the task's assigned build directory, then run
`cargo fmt -p t3-auth`, `cargo clippy -p t3-auth --all-targets -- -D warnings`,
and `cargo test -p t3-auth`.

The automated bidirectional probe runs real TypeScript services and Rust against a
new temporary database and secret directory, removes it afterward, and prints no
credentials. From the repository root, one command runs all five phases:

```powershell
pwsh -NoProfile -File rust/crates/t3-auth/tools/interop.ps1
```

It checks exact scopes, jkt/rtc/plk claims, session/ticket verification, pairing and app
codes, desktop derivation, and MCP registration through TypeScript's real private
`parseClientId` (via `validateAuthorization`). Both directions reject tampering, expiry,
revocation and consumed pairing codes. Actual `verify_request` /
`verifyRequestDpopProof` calls prove each process rejects markers written by the other,
including after reopening. The complete command passed on Windows on 2026-11-13.

## MCP resource binding

TypeScript's `auth/McpOAuth.ts:requestUrls` uses the request origin as issuer and
`${origin}/mcp` as resource. At authorization and code exchange, an omitted RFC 8707
resource means that endpoint; a supplied value must have the same origin/path after
trailing slashes are stripped. Queries are ignored and nonempty fragments refused.
The code is bound to the signed client registration, redirect, S256 PKCE challenge,
canonical resource, approved access and 60-second expiry, and is spent on exchange.
Client identity is bound at this exchange, not by a claim in the bearer access token.

The actual TS access token (`SessionStore.ts:SessionClaims` and
`EnvironmentAuth.ts:issueMcpClientSession`) has **no audience or resource claim**.
It is a normal v1 session with `sub: "mcp-client"`, bearer method, approved scopes,
30-day millisecond expiry, optional runtime ceiling and peer-link flag. TS resource
verification checks its HMAC, signed expiry, persisted session and revocation, then
MCP subject/method. Effective scopes come from the row; access maps to read-only or
the runtime ceiling. Tool write checks reject read-only clients. It does not recheck
resource/client id or persisted expiry. These are distinctions from the stricter Rust
resource-server contract, not guarantees supplied by TS.

Approved scopes match `packages/contracts/src/auth.ts:mcpClientScopes`: read-only
gets `orchestration:read`; other levels also get `orchestration:operate`; full-access
additionally gets `settings:write` and `source-control:write`. The approval decision
determines the grant, rather than the authorization request's `scope` parameter.

`issue_mcp_session(resource, label, access, peer_link, client)` now requires the
public resource URL and signs a string `aud` naming its canonical origin plus `/mcp`.
`McpOAuth::exchange_code` supplies the already validated pending resource. OAuth
endpoint URLs, credentials, queries, fragments and other paths are refused as issuer
configuration; `same_resource` retains TS's request-parameter normalization rules.
Generic `IssueSession::mcp_resource` exposes the same issuance binding.

`SessionStore::verify_mcp(token, expected_resource, required_scopes)` returns a
`VerifiedSession` after checking signature, signed and row expiry, live row/revocation,
MCP bearer identity in both claims and row, absence of proof binding, exact string
audience, optional signed `resource`, mandatory `orchestration:read` and every
requested live row scope. Multiple-resource audience arrays are refused. Verified
`oauth_audience` and `resource_indicator` are separate from the existing WebSocket
`audience` field. `authenticate_mcp` uses this verifier for the request's `/mcp`
origin/path (query excluded). Environment isolation continues to use each environment's
signing key and session database; no new environment-id claim or database schema is
introduced. These secret-bearing session types still have no `Debug` implementation,
and errors never echo the resource or token.

General `SessionStore::verify` retains audience-less TS session compatibility and
shares the unchanged signing file, two-segment format and database rows. Strict MCP
verification refuses old audience-less MCP tokens: clients must authorize again
against Rust. Tokens still issued by unmodified TS require the same reauthorization;
there is no compatibility bypass on the strict MCP endpoint. Rust-issued audience-bound
tokens remain acceptable to the TS resource server, whose schema ignores extra claims.

`tests/mcp.rs` isolates missing/wrong/multiple audiences, another endpoint/environment,
resource mismatch, signed and persisted expiry, revocation, missing read/operate/settings/
source-control permissions, claim/row identity mismatch, proof binding, unknown sessions,
tampering and secret-free resource errors. `tests/oauth.rs` pins the emitted audience
and code binding to a different valid registration, origin, redirect, PKCE and resource.
The existing secret-format tests are unchanged.

The interop command requires an explicit `CARGO_TARGET_DIR` and preserves it. Its real
TS OAuth registration/approval/exchange produces a token accepted by Rust's generic
verifier and refused by strict MCP verification for missing audience. A separate,
explicitly extended fixture signed by TS's real `PayloadSigner` proves audience-aware
verification. Rust's real OAuth exchange produces a token accepted by TS's real
`authenticateMcpClient`; expiry/revocation/tampering and cross-process revocation are
checked too. No credentials are printed.

Dependent owners: `t3-server` HTTP/adaptor paths need no changes; their `mcp_token`
already calls `exchange_code`, which now propagates resource internally. In
`t3-mcp/src/oauth.rs`, keep the actual request-resource vs configured-resource check;
replace the block from `sessions.verify` through manual payload `resource` validation
with `let session = self.auth.sessions.verify_mcp(token, self.resource.as_str(), &[]).await.ok()?;`.
Retain tool-specific scope checks using `session.scopes`. Remove now-unused base64,
JSON Value and session-method imports. Its `tests/oauth.rs` direct issuer call needs
`"https://example.test/mcp"` as the first argument and should assert successful native
issuance; retain the missing-audience negative fixture using a generic session. Its
array-audience success assertion must become rejection. The current production
manual checks already accept the new string audience, but the shared verifier avoids
duplicating the security contract. No dependant files were edited.

## Deliberate TypeScript differences and mixed deployments

| Behavior | TypeScript source behavior | Rust behavior and mixed deployment impact |
|---|---|---|
| Cross-process live-session revocation | `SessionStore.ts:awaitInvalidation` waits for local PubSub or expiry; CLI/another server cannot wake it. | `SessionInvalidation::wait` rereads every renewal-chain row at most one second after its prior check, as well as on local changes/expiry. Rust sockets close promptly after either implementation or a CLI revokes a row. TS sockets retain the old limitation. No credential or disk-format change. |
| HTTPS cookies | `auth/http.ts` omits Secure for normal, dev and expired cookies. | All browser response cookies set Secure for HTTPS, including `X-Forwarded-Proto: https`; plain HTTP LAN remains usable. Both servers accept identical cookie names/values. TS can later emit a cookie without Secure; mixed deployments should account for its weaker responses. |
| Forwarded origin host | `EnvironmentAuth.ts:browserOriginAllowed` trusts `X-Forwarded-Host` unconditionally. | Default false; set `AuthConfig::trust_forwarded_host` only for a trusted proxy/tunnel that overwrites that header. Host, trusted origins and same-origin fetch metadata retain source behavior. Mixed deployments keep valid credentials; Rust proxy-origin requests can need this explicit configuration. |
| Trailing token segments | `SessionStore.ts` ignores segments after body/signature for sessions and tickets. | Exactly two nonempty segments are required. Neither implementation issues extras, so issued credentials interoperate; unsafe suffixed presentations are deliberately refused only by Rust. |
| Bootstrap DPoP replay ordering | `auth/http.ts:token` records the proof before checking the bootstrap credential. | `exchange_bootstrap` verifies proof cryptography, checks/consumes the credential and scope grant, then records the marker before session issuance. Rejected credentials create no marker; a marker/storage failure issues no token. Successful proofs keep the exact TS marker format. TS may still create markers for failed credentials. |
| Empty configured secrets | `PayloadSigner.ts` / `desktopBootstrapToken.ts` die on zero-length HMAC material; empty legacy/dev credentials are ignored. | Constructors and desktop derivation/verification return typed `EmptySecret`; explicit empty desktop seed/dev configuration is also rejected. Nonempty short keys and whitespace-only material remain accepted because TS accepts their UTF-8 bytes. Valid issued credentials interoperate. |
| Pairing delegation | TS HTTP handler composes default full scopes and a separate delegation check. | Every public issuing call takes caller scopes and an explicit grant, validating it before writes. `AuthCreatePairingCredentialInput` without scopes/access is rejected; clients must state one. Credential format is unchanged. |
| Invalid RPC payload | TS transport decodes all methods and its guard redecodes dynamic ones. | The final Rust guard redecodes all methods and refuses malformed payloads itself. Correct payloads and required scopes are unchanged. |

## Security review closure and evidence

| Finding | Resolution / proof |
|---|---|
| H1 | Nonempty initialized signer state; typed validation on direct/store/session/environment/OAuth loaders and desktop helpers. Empty-file/config tests cover every entry point. |
| M1 | Method-specific contract decoding; fixture traversal rejects arrays and malformed objects for every guarded method. Typed positive cases cover every dynamic scope rule. |
| S1 | All public pairing issuers validate delegation inside the issuing call. Tests cover missing access:write, unheld scopes, access-derived elevation, omitted/empty scopes and a valid narrow grant. |
| S2 | Header type normalizes names and preserves duplicates. Mixed-case Origin/Authorization and duplicate tests reject bypasses. |
| L1 | Cookie/authorization-error/approval-result Debug redactions; remaining local secret containers have no Debug. Contract-owner changes are listed below. |
| L2 | Watch receiver read per registration; true → false → true test reuses the same service. |
| L3 | Signed empty jkt normalizes to unbound on session and ticket paths; explicit empty expected thumbprints fail real proof validation. |
| L4 | Checked TTL additions; ticket/pairing overflow tests return Date instead of panicking. Seed construction also checks overflow. |
| L5 | Typed macro lookup, cached cookie names/origins, borrowed Value decoding. Existing scope/cookie/origin matrices preserve behavior. |
| L6 | Grant-source struct/enum, typed approval/page errors, exhaustive enum conversions and database string deserialization. MCP approval/access/persistence tests cover the decisions. |
| L7 | Approval access/method, actual startup/runtime policy counts and retained issuance cause; tests include a real failing SQLite insert trigger. |
| L8 | UUID builder, library hex encoder, fallible signer construction; MAC vectors, issuance, storage races and empty-key errors pin behavior. |
| L9 | Multiline SQL, explicit imports, one pairing column list. Real SQLite single-use/replacement/revocation tests retain semantic proof. |

The initial regression run failed all eight newly added behavioral tests: empty
signing file, empty bootstrap configuration, malformed guarded payload, mixed-case
Origin, secret Debug, overflow, trailing token and forwarded-host trust. The Debug
state assertion then failed independently. External revocation separately failed
with a 30-day deadline, then passed with the one-second recheck. After the API
migration, controlled reinstatements of the reviewed behavior also made the
delegation, HTTPS-cookie, empty-jkt/thumbprint, captured peer-setting, lost audit
access/cause/counts, and premature replay-marker tests fail; each fix was restored
after its failing run. Source-quality/type-shape
findings L5/L6/L8/L9 preserve behavior; their SQL/format/crypto invariants are checked
without inventing an incorrect runtime outcome just to make another test red.

The review's suggested **32-byte minimum** is deliberately not adopted. TS
`auth/PayloadSigner.ts:fromSecret` and shared `desktopBootstrapToken.ts:deriveToken`
call `Mac.key`, which rejects only zero-length input. Rust's existing
`storage_utils::mac_and_base64_formats_are_compatible` pins a valid 20-byte RFC
HMAC vector. Rust now refuses zero bytes while retaining nonempty legacy keys
and UTF-8 whitespace for interoperability. No security finding is dismissed;
this adjusts the recommendation's threshold to the requested compatibility rule.

Verification of the initial security fixes (2026-11-13): scoped format, clippy with `-D warnings`, and all
**62 Windows tests** pass with no skips. The one-command bidirectional probe
passes. The secret creator/permission test also passes on WSL Ubuntu, exercising
actual 0700 and 0600 assertions. Linux tooling/cache/artifacts were isolated
under the assigned `rust/target/agents/t3-auth` directory. Native macOS remains
unverified. A transient missing sibling manifest was resolved by its owner;
no sibling source or manifest was edited here.

## Public API changes from the reviewed crate

- `PayloadSigner::new(Vec<u8>) -> Result<PayloadSigner>`; its initialized HMAC state
  guarantees sign/verify never use an empty key. `from_store` propagates this error.
- `AuthConfig::validate() -> Result<()>` is new; `trust_forwarded_host: bool` is new
  and defaults false. `EnvironmentAuth::config` is now private with `config()` for
  immutable access, keeping cached origin data consistent with configuration.
- `PairingGrantStore::new(...) -> Result<PairingGrantStore>` checks configuration
  and bootstrap TTL overflow. `IssuePairing::scopes` is now `Vec<Scope>`; its empty
  default is refused. `issue_one_time_token(caller_scopes, input)` and
  `EnvironmentAuth::create_pairing_link(caller_scopes, input)` validate delegation.
- `issue_pairing_credential(input)` is replaced by
  `issue_pairing_credential_for(caller_scopes, input)`. Explicit `access` derives that
  access's scopes; otherwise explicit `scopes` are required. Startup issuance states
  standard scopes explicitly as a trusted server operation.
- `current_desktop_token(...) -> Result<String>` and
  `valid_desktop_token(...) -> Result<bool>` reject empty secrets.
  `pairing::access_name(...) -> Result<String>` uses the contract serializer rather
  than maintaining a second table of wire spellings.
- `Request::headers` is `Headers`, not `BTreeMap`. `Headers` supports `Default`,
  `Clone`, collection/from arrays, `insert`, `append`, `remove`, and `get`; names are
  case-insensitive, and duplicate collection is preserved. `insert`/`remove` return
  `()`. `browser_origin_allowed` now takes `trust_forwarded_host: bool`.
- `create_browser_session(request, credential, metadata, previous)` takes the request.
  `Cookie::session(..., secure)` and `Cookie::expire(name, secure)` require the Secure
  decision. `Cookie::Debug` redacts its value while retaining useful attributes.
- `exchange_bootstrap(request, credential, requested, metadata)` replaces its old
  caller-provided thumbprint with proof verification/replay handling in the service.
- `McpOAuth::new(auth, watch::Receiver<bool>)` replaces the captured boolean and
  observes every registration's current linked-environments setting.
- `McpOAuth::approve` returns `ApprovalResult { redirect, granted }`. `granted` is
  `Option<ApprovalGrant { access, method: ApprovalMethod }>`; denial has no grant.
  The result's Debug redacts the entire redirect (codes/state can be in its query).
  `AuthorizationError::Debug` redacts redirect/state. Secret-bearing local types
  without Debug remain unprintable, including config, sessions, requests, bootstrap
  responses, OAuth clients, authorization requests and CSRF approval sessions.
- `enforce_policy() -> Result<PolicyRevocations { sessions, pairing_links }>` and
  `initial_policy_revocations()` expose actual successful revocation counts.
- New errors: `EmptySecret`, `InvalidSigningKey`, typed `McpApprovalReason` in
  `McpApprovalCode`, typed `ApprovalPage`, and `OAuthSessionIssue { source: Box<Error> }`.
  HTTP adapters should map the last error to OAuth `invalid_grant` while retaining
  its source for internal auditing. No pseudo OAuth `"page"` error remains.
- `RPC_REQUIRED_SCOPES` now stores `Scope` values instead of wire strings; one macro
  generates both that table and the direct method lookup. `required_scope_for_device_list` now returns `Result<Scope>` and refuses invalid
  device payloads independently; other lookup/guard signatures are unchanged. TTL overflow now returns `Date` instead of panicking/wrapping.
- `SessionInvalidation`'s public API is unchanged; its wait now checks shared rows
  every second. Cookie names and trusted origins are cached during construction.

Both `RuntimeMode` and `AuthMcpClientAccess` belong to `t3-contracts`, so Rust's orphan
rule prohibits implementing `From` between them in this crate. Exhaustive local match
functions replace the former serde round trips instead. Database enum text is decoded
with a string deserializer, never spliced into JSON. These preserve TS's wire names.
UUID construction uses `uuid::Builder`, hexadecimal uses `base16ct`, and signer setup
has no panic. The single-use/revocation SQL is expanded for review and imports name
helpers explicitly. These source-quality changes do not invent new runtime behavior;
existing MAC/UUID/storage/atomicity tests verify their observable invariants.

## Contract Debug redactions required from the contracts owner

No `t3-contracts` file was edited. In `rust/crates/t3-contracts/src/auth.rs`, redact:

| Type | Secret field(s) |
|---|---|
| `AuthBrowserSessionRequest` (line 140) | `credential` |
| `AuthTokenExchangeRequest` (281) | `subject_token` |
| `AuthAccessTokenResult` (329) | `access_token` |
| `AuthWebSocketTicketResult` (343) | `ticket` |
| `AuthPairingCredentialResult` (379) | `credential` |
| `AuthMcpAuthorizationRequest` (889) | `state` (and redirect URI if it may contain state) |
| `AuthMcpApprovalDetails` (950) | `csrf_token` |
| `AuthMcpApprovalRedirect` (981) | entire `redirect_to` (authorization code/state query) |
| `AuthMcpApprovalPairingCode` (1005) | `code` |
| `AuthMcpApprovalBrowserSession` (1021) | `csrf_token` |
| `AuthMcpTokenRequest` (1061) | `code`, `code_verifier` |
| `AuthMcpTokenResult` (1107) | `access_token` |

Their derived-Debug wrappers also expose these fields recursively:
`AuthMcpApprovalDecision` and `AuthMcpApprovalDecisionRequest`. Signed MCP registration
IDs are capabilities too: consider redacting `AuthMcpRegisteredClient::client_id`,
`AuthMcpAuthorizationRequest::client_id`, and `AuthMcpTokenRequest::client_id` with them.

## Remaining integration and known differences

- No HTTP/WS server, routing, HTML renderer or MCP invocation context is included.
  Remaining: `apps/server/src/auth/http.ts`, `mcpOAuthHttp.ts`, `mcpOAuthHtml.ts`,
  `McpOAuth.ts:layerMcpClientAuthenticator`, `apps/server/src/ws.ts`. The auth
  decisions and cookie/header data are here; trace IDs, logging spans, endpoint
  error envelopes, HTTP rendering and subscriptions must be applied by adapters.
- `mcpOAuthHttp.ts`'s hosted `/approve-agent` versus local `/connect-agent`
  handoff needs `cloud/CloudLink.ts` and cloud public configuration. Its routing
  test is not ported. Relay/cloud JWT issuance and installation, peer networking,
  and guest-share credentials remain in `apps/server/src/cloud/`, `peer/`, and
  `guest/{GuestShareService,http}.ts`; they are separate services, not an auth
  mode implemented inside this directory. Guest tokens are not accepted as
  ordinary environment sessions.
- Managed-policy loading/platform discovery and provider/settings enforcement
  remain in `policy/ManagedPolicy.ts`, `managedSettings.ts`, `managedProviders.ts`
  and shared `managedPolicyFile.ts`. This crate receives the decoded document and
  ports auth-policy/remote/connect limits and startup revocation. The caller owns
  the hourly replay-pruning schedule (`auth/replayMarkers.ts`). Generic secret
  providers in `secrets/` are unrelated to auth's raw file store and are not ported.
- Common wire types use `t3-contracts`; no existing contract crate was edited.
  **To move to t3-contracts when its helpers are ready:** local permission helper
  definitions corresponding to `clientRpcPermissions.ts` and `auth.ts`, including
  standard/MCP scopes, legacy response projection and required-permission mapping.
  `dpop::PublicJwk` is the local wire-shaped equivalent of shared `dpopCommon.ts`;
  it can move to the shared contract layer. Issue options, stored records and
  service results without a TypeScript contract remain intentionally local.
- Error handling uses Rust enums with the original credential-failure names and
  contract scope-denial payloads; Effect's internal nested cause/span classes
  are not reproduced. Connectivity methods return errors to the caller instead
  of consuming them into Effect logs. Adapters own those mappings/logs.
- Millisecond TTL inputs are integer-valued. Serde cannot represent JavaScript
  lone UTF-16 surrogates; MCP client names issued through a surrogate at code-unit
  100 are replaced with U+FFFD; TS-issued client IDs containing a lone surrogate
  are refused by Serde. Loopback redirects distinguish absent and empty queries,
  unlike TS. DPoP iat must use integer JSON syntax, unlike TS Schema.Int which
  also accepts an integral decimal or exponent. Non-finite signed numeric claims are rejected at
  decoding; normal TypeScript-issued credentials use finite integer dates.
  Date parsing/formatting uses Chrono's supported RFC3339/year range, narrower
  than JavaScript's extreme ±275760-year Date range. Normal lifetimes and the
  year-9999 dev sentinel are verified. These pathological signed payloads do
  not have full decode/error parity.
- Internal-path checks retain the source's realpath-or-lexical fallback. A
  nonexistent file using a Windows short parent name can differ from the
  canonical long parent; actual existing files resolve consistently.
- Database schema/version support is inherited from `t3-persistence` (currently
  refuses pre-head-69 databases); this crate installs no tables or migrations.
  Windows checks and bidirectional interop pass; POSIX permission
  coverage is additionally exercised through WSL Linux. Native macOS was not exercised.
