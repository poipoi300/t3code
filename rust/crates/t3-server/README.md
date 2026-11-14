# t3-server

The native composition root (Tokio and axum) and the Effect RPC WebSocket transport: the Rust
`t3` server. It does not compile for Wasm. The library never opens a default home: callers pass
`adapters::config::ConfigInputs`, and only the `t3` binary discovers the account home. Tests use
temporary homes and port zero.

## Start-up and shutdown

`runtime::start(inputs)` (and `start_with(inputs, StartOptions)` for injected provider adapters
and Replay (dev)) returns once the server is ready, in TypeScript's order (`server.ts`,
`serverRuntimeStartup.ts`):

1. Load the managed policy and decide the bind address (`checkServerReachability`): loopback
   when no host is given, never a host the organisation's policy forbids.
2. Bind the listener, so cookies and the runtime file use the real port.
3. Open local services (`LocalServices::open`): ownership of the home before the database
   migrates, settings, keybindings, environment identity, auth.
4. Start the domain services (`runtime::start_domains`), each after its own recovery, and put
   them in `Domains`.
5. Build the handler table (`handlers::registry_with`), mark HTTP listening.
6. Publish `welcome`, activate domains, write the runtime file, open the command gate, publish
   `ready`.

`RunningServer` exposes `address`, `pairing_credential`, `rpc_methods_served`,
`unserved_methods`, `phase`, `wait_ready`, `shutdown`, `wait`, `run_until_signal`. Shutdown
closes the command gate, stops accepting, ends HTTP connections at once (TypeScript's grace is
0), joins every WebSocket, shuts domains down in reverse start order while the database is
open, closes the database and releases the home last. HTTP requests must send their headers
within 60 s and keep their body moving within 300 s (Node's defaults).

## Adding a domain group

1. **Adapter** (`src/adapters/<group>.rs`): builds the crate's services once from a
   `services::DomainContext` (local services, the shared `Database`, the shutdown token, the
   account home, the captured environment) and holds what the handlers need. If it has a
   shutdown or activation step, implement `services::DomainRuntime`.
2. **Start it** in `runtime::start_domains`, behind its feature, with
   `domains.insert(...)` or `domains.insert_runtime(...)`. Dependencies come from `Domains`
   (`domains.get::<T>()`) or from the context; never build a second copy of another domain's
   service. Present order: telemetry, host, workspace authority, git, devices, workspace
   services, terminal, usage, orchestration, scheduler, projects.
3. **Handlers** (`src/handlers/<group>.rs`): `register_with(registry, domains)` registers each
   method with `registry.register::<Rpc, _, _>` or `register_stream`, decoding nothing itself:
   the registry decodes the payload and the RPC guard checks the method's scope (from
   `t3-auth`'s table) before the handler runs. A handler calls one service method and maps the
   error to the contract's typed failure (`Failure::Typed`); anything else is a defect. When the
   adapter is absent, register `registry.unimplemented::<Rpc>()`: the client receives Effect's
   unknown-request-tag defect and treats the method as unsupported. Add the call to
   `handlers/mod.rs` and remove the methods from `handlers/unsupported.rs`.
4. **Tests**: drive a started server over a real socket (`tests/composition_domains.rs` has a
   `Server` helper with `call`, `chunk` and `reconnect_with(scopes)`). `tests/scope_sweep.rs`
   checks every served method against a session missing one required scope, with the contract's
   sample payloads; new methods are included automatically.

Paths a client names (cwd, files, clone destinations) are checked against the workspace
authority (`workspaces.rs`, port of `KnownWorkspaces.ts`: managed worktree folders, live project
roots, live thread worktrees under a managed folder or listed by `git worktree list`). Device
`open` takes the thread's workspace from the authority, never from the client.

## Transport

`ws.rs` upgrades `GET /ws`: the `orchestrationProtocol=2` check (426) runs before any credential
is read, then cookie, bearer, one-use RPC ticket or DPoP query proof; a foreign Origin cannot use
the cookie. Scopes are fixed at upgrade; revocation, expiry and shutdown close the socket and
end its streams. Messages and joined parts are capped at 100 MiB (close 1009); responses are
split only for `frameParts`. A panicking handler fails its call; a malformed frame gets a
`Defect` reply. Calls are polled in rotation so a busy stream cannot starve the others.

`http/` serves the auth routes, MCP OAuth, orchestration reads (protocol header, then
`orchestration:read`), the transcript export, workspace assets and uploads, scheduler webhooks,
the device hub and static files. CORS: production wildcard without credentials; in dev mode only
the dev URL's origin, the desktop renderer origins and `T3CODE_DEV_ALLOWED_ORIGINS`, with
credentials (TypeScript `layerBrowserApiCors`).

## Dev mode

`t3 serve --dev-url <url>` or `VITE_DEV_SERVER_URL` (as in TypeScript, an ambient variable turns
it on): the Replay (dev) provider is registered and seeded in settings, loopback page requests
redirect to the dev URL, and CORS allows the dev origin. With an implicit home, state lives in
`<base>/dev`; with an explicit one, in `userdata`. `t3 serve` prints a pairing URL on the
server's own origin; `t3 pair` prints TypeScript's link on the dev URL and a
`Server pairing URL:` for apps that pair with the server directly (desktop, mobile). Without
`serve`, dev mode needs the project auto-bootstrap, which is not ported yet.

## Differences from TypeScript

- Managed policy: read as TypeScript reads it, from the Group Policy registry key on Windows, the
  configuration profile on macOS and the policy file (`t3_config::policy_sources`; its README
  lists the sources and the stricter refusals). The `T3CODE_DEV_MANAGED_*` overrides need the
  non-default `dev-policy-overrides` feature in a debug build. Policy-locked settings are not
  applied.
- Not ported or refused at start: LAN fencing (`--lan`), Tailscale Serve, the desktop bootstrap
  descriptor, project auto-bootstrap from the working directory.
- Request bodies time out per read (300 s idle) rather than over the whole request.
- WebSocket permessage-deflate is absent (an optional extension no client requires).
- MCP access tokens carry an audience the Rust endpoint verifies; agents re-authorise once after
  switching from a TypeScript server (pending the owner's decision).
- `t3 doctor`'s compact command is printed in its plain form; the install-launcher resolution of
  `resolveHostCliCommand` is not ported.
- Served methods and the blockers of the rest: `briefs/server-unserved.md` in the notes, and
  `RunningServer::unserved_methods`.

## Run and verify

```powershell
$env:CARGO_TARGET_DIR='<your target directory>'
cargo run -p t3-server --manifest-path rust/Cargo.toml --bin t3 -- serve --base-dir <disposable-directory> --port 0
cargo test -p t3-server --manifest-path rust/Cargo.toml --test composition_domains
cargo clippy -p t3-server --manifest-path rust/Cargo.toml --all-targets -- -D warnings
cargo clippy -p t3-server --manifest-path rust/Cargo.toml --no-default-features --all-targets -- -D warnings
```

The binary prints a secret pairing URL; show it only on a private console. `tests/coverage.rs`
checks that all 220 contract methods have exactly one disposition; `tests/composition_web_boot.rs`
runs the web client's start-up sequence and prints the served count.
