# t3-server

Native Tokio/axum composition root and Effect RPC WebSocket transport. Does not
compile for Wasm. No default home is opened by the library: callers supply
`adapters::config::ConfigInputs` explicitly. The `t3` binary discovers the account
home only when invoked. Tests exclusively use temporary homes and port zero.

## Public API and layout

- `runtime::start(inputs)` returns once the server is ready, in TypeScript's order
  (`server.ts`, `serverRuntimeStartup.ts`):
  1. Bind the listener, so the actual port is used for cookies and the runtime file.
  2. Open local services; ownership is acquired before the database migrates.
  3. Read and follow the linked-environments setting for peer links.
  4. Start domain services, each returning after its own recovery.
  5. Build the handler table from them.
  6. Mark HTTP as listening.
  7. Publish `welcome`, activate domains, and write the runtime file (a write failure
     is logged, as in TypeScript).
  8. Open the command gate and publish `ready`.

  It returns `RunningServer`: `address`, `pairing_credential`, `phase`, `wait_ready`,
  `cancel`, `shutdown`, `wait`, `run_until_signal`. Shutdown closes the command gate,
  stops the listener, cancels and joins every connection, shuts domains down in reverse
  start order while the database is open, drops every handle, closes the database and
  releases ownership last. It continues past errors and reports them together. Signals
  (POSIX SIGINT and SIGTERM; Windows Ctrl+C, Ctrl+Break, close, shutdown) and lost
  ownership cancel the same token.
- `services::ServiceRegistry` is the state every transport sees: handlers,
  cancellation, connection tracker, `Lifecycle` (phases Starting, CommandReady,
  Ready, Stopping; `admit_command` is the command gate; listening is tracked
  separately), `Domains` and local config/auth services.
- Domain services: an adapter builds its service from a `DomainContext` (local services,
  the shared `Database`, the shutdown token, the captured account home and environment).
  `runtime::start_domains` starts, behind each feature, git, terminal, usage,
  orchestration, then scheduler, and inserts each into `Domains`. Services with a shutdown
  or activation step implement `DomainRuntime`. `handlers::registry_with(&domains)` hands
  each service to its handler module; handlers and routes read it with
  `ServiceRegistry::domain::<T>()` (`orchestration()` for the orchestration adapter).
  Scheduler sources stay parked: with no command intake, its dispatch fails with an
  explicit error. Terminals open where asked, without worktree recovery. Usage has no
  provider sources yet.
- `http/` contains auth/static routing; `ws.rs` drives `t3_rpc::Server`, preserving
  cancellation, EOF, application ping/pong, stream ACK windows, header context and
  negotiated frame splitting. Input and assembled messages are capped at 100 MiB.
  Each connection tracks session invalidation and disconnect cleanup.
- `adapters/` is the only sibling-service contact surface. `handlers/` has core,
  orchestration, workspace/terminal, source-control and optional-service ownership
  modules.

`local-services` is a default optional feature enabling config/auth/persistence.
`--no-default-features` compiles the transport/registry foundation independently.
Optional orchestration/workspace/git/forge/terminal/scheduler/usage/providers
features reserve dependencies and adapter ownership; they **do not** assemble
those services. Provider protocol clients are not complete provider services.

## Implemented first slice

Eleven HTTP method/path registrations including wildcard static and the Rust-local
GET `/healthz`: descriptor, session state, browser pairing, legacy and audience
WebSocket tickets, pairing creation, client revocation, bootstrap OAuth exchange,
GET `/ws`, and static fallback. `/healthz` explicitly reports
`orchestrationReady:false`. No public health route exists in the TS source.

The core RPCs work (see "WebSocket and core RPC" below). Reads redact settings for clients;
unassembled providers/editors are empty and domain capabilities are not advertised. Methods
without a service have explicit typed marker declarations in
`handlers/unsupported.rs`; adding/removing a contract breaks the coverage test.
They terminate with structured `ServerDefect::NotImplemented` on Effect's `Die`
channel, after authorization. Existing `Fail` unions do not admit this error.
`ServerDefect` is a **candidate to move to t3-contracts** if shared unsupported
behavior is standardized; no existing wire DTO was duplicated.

Static serving streams files, supports extensionless/SPA fallback, HEAD,
non-HTML weak ETags and Last-Modified/304 precedence, HTML revalidation,
manifest-listed immutable assets, MIME types and traversal checks. HTTP gzip and
production CORS follow the source surface. Cookies, tickets, origin/DPoP and
execution-time scope decisions use t3-auth; persistence uses t3-persistence only.
Runtime state uses the actual port and owner UUID. Signals cover Windows
Ctrl+C/Break/Close/Shutdown and POSIX SIGINT/SIGTERM.

## WebSocket and core RPC

`ws.rs` upgrades `GET /ws`: the `orchestrationProtocol=2` check (426 with code
`orchestration_protocol_incompatible`) runs before any credential is read, then
`EnvironmentAuth::authenticate_websocket` accepts cookie, bearer, RPC-audience ticket and
DPoP query proof, then a plain (non-upgrade) request gets 426. The connection owns a fresh
`t3_rpc::Server`; the session is marked connected before the loop and marked disconnected on
every exit, including a panic. Session expiry or revocation, server shutdown and a peer
close end it; the first two send a close frame. A handler that panics fails that call with a
defect; a malformed frame gets a `Defect` reply and the connection carries on. Incoming
messages, and U+001E-joined parts, are capped at 100 MiB (close code 1009). Responses are split
into 48 KiB parts only when the query has `frameParts`. `clientCapabilities=...,
settingsSecretReferences` is read per socket and decides how settings are redacted.

`registry.rs`: a method is registered or in `handlers/unsupported.rs`, exactly once, with the
contract's call kind (`validate`, unit tests, `tests/coverage.rs`). Decode, then the execution-time
scope check, run for unsupported methods too, before anything is answered.

`handlers/core.rs` serves `server.probe`, `server.getConfig`, `subscribeServerConfig`,
`subscribeAuthAccess`, `subscribeServerLifecycle`, `server.upsertKeybinding`,
`server.removeKeybinding` and `server.updateSettings`. Lifecycle `welcome` and `ready` are
published when `LocalServices::open` returns, because only configuration and auth are served
there; a root that adds domain barriers should publish `ready` again after them.

Compatibility decisions, with evidence:

- **Unsupported methods** answer on Effect's `Die` channel with
  `{"_tag":"NotImplemented","method":...}`, since no `Fail` union has a shared member for it.
  `tests/c_effect.rs` runs the product's real client stack (`WsRpcGroup`,
  `RpcClient.makeProtocolSocket`, the frame-part joiner) from Node against a live server: a unary
  call and a stream each fail with that defect and the same session keeps serving.
- **permessage-deflate** is absent. The TypeScript server enables it (`server.ts`), but no client
  requires it: it is an extension a client may offer and a server may decline (RFC 7692), the
  Effect socket layer and the browser/React Native `WebSocket` offer nothing special, and only the
  transfer-budget measurement client asks for it. The cost is wire size on large JSON streams.
  axum/tungstenite cannot offer it; `yawc` (with its `axum` feature) implements it and would
  replace axum's `WebSocketUpgrade`.
- A single frame over the limit is refused from its header while the peer is still sending, so the
  peer sees close code 1009 or, if the operating system resets first, a reset. Over-limit parts get 1009.

## Run and verify

From the repository root in PowerShell, always use the assigned target:

```powershell
$env:CARGO_TARGET_DIR='C:\Users\Administrator\t3code-rust\rust\target\agents\server-a'
cargo run -p t3-server --manifest-path rust/Cargo.toml --bin t3 -- serve --base-dir <disposable-directory> --host 127.0.0.1 --port 0
cargo fmt -p t3-server --manifest-path rust/Cargo.toml --check
cargo clippy -p t3-server --manifest-path rust/Cargo.toml --all-targets -- -D warnings
cargo test -p t3-server --manifest-path rust/Cargo.toml
cargo clippy -p t3-server --manifest-path rust/Cargo.toml --no-default-features --all-targets -- -D warnings
```

The binary prints a secret pairing fragment URL for its operator. Serve it only
from a private console. Bundled static discovery checks `client/index.html` by
the executable, then the source checkout's `apps/web/dist`; it does not build web.

`tests/composition.rs` spawns the `t3` binary against a temp home with a cleared
environment, reads its announced origin and pairing URL, checks that the runtime file
records the bound port, and drives health, descriptor, pairing, ticket, WebSocket,
`server.probe`, `server.getConfig` and the config stream. It then stops the binary:
SIGTERM with a clean exit on POSIX, a forced kill on Windows (raising Ctrl+Break in
another process needs `unsafe`). A restart on the same home proves that ownership
and the database were released. It also checks that a second server on a home that
is already owned is refused without touching the owner's runtime state.

`tests/composition_web_boot.rs` runs the TypeScript web client's start-up sequence
(`composition_web_boot_client.ts`, real client-runtime code in the web app's order)
and prints each step as served, served with a difference, or unsupported, with the
missing Rust service method: the to-do list for booting the existing web client.

`tests/coverage.rs` compares all 220 WsRpcGroup fixture methods and stream flags
with the typed registry. `tests/transport.rs` starts real HTTP/WebSocket listeners,
pairs once, rejects reuse, obtains scoped tickets, drives a real t3-rpc client,
reads configuration/settings and live auth/config streams, checks scope denials
(including discarded unary/stream calls), unsupported completion, ticket audiences,
revocation disconnection, static cache/path/HEAD behavior, CORS, and shutdown/restart
with a live connection. Tests await network/file events and use no sleeps.

## Remaining port and differences

The external plan at
`C:/Users/Administrator/t3code-rust-notes/briefs/server-plan.md` contains eight
ownership packages, every source HTTP registration, all 220 RPC call mappings,
CLI declarations and the ordered Replay milestone. That milestone is **not met**:
projects, shell/thread reads and Replay turn dispatch await orchestration wiring.

Unported sources include server.ts/serverRuntimeStartup.ts domain startup,
ws.ts's remaining registrations, orchestration-v2/http.ts, project/http.ts,
auth/mcpOAuthHttp.ts, mcp/McpHttpServer.ts, scheduledTasks/webhookRoute.ts,
assets/upload HTTP transport, guest/cloud/device/speech routes, diagnostics,
provider discovery/auth/install and the CLI command families in cli/. Managed
policy loading/enforcement and desktop bootstrap transport are not assembled.
Private-network fencing, dev proxy and Tailscale options are explicitly rejected;
serve is a foreground config/auth server and rejects unsupported CLI flags.

The config watcher API exposes only an opaque Drop-aborts handle, so this root
retains it through shutdown but cannot await the watcher's task finalizer. Domain
shutdown (provider/worker/scheduler/terminal/git) is specified in the plan, not
simulated here. SQLite schema compatibility is inherited from persistence
(head >=69). POSIX runtime/signals are unexecuted on this Windows host.

Other incomplete parity: WebSocket permessage-deflate is absent; HTTP compression
is gzip only; dev/asar static behavior is absent; JSON extractor errors use axum's
422 rather than all Effect error envelopes; some auth internal failure categories
use the generic internal-error reason, client-presented OAuth metadata is not yet
fully forwarded, and managed-policy error rendering is not wired. CORS is the
production wildcard mode because dev mode is rejected. Missing reserved API routes
return 404 rather than SPA HTML. Settings secret-reference capabilities are
conservatively treated as absent. Newer/future contract refinements are limited by
the serde validators supplied by t3-contracts.
