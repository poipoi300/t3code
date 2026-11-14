# t3-device

Native Tokio port of `apps/server/src/device/`; no WASM target. Device and RPC
values come from `t3-contracts`. There are no replacement wire types in this crate.

## Service and transport bindings

`DeviceService::new` takes the local host, settings, optional host factory,
launchers, installer, and accounts. Settings changes to host definitions must
call `reconcile`; `configure` handles enablement and agent access. Dropping a
subscription receiver unsubscribes it. Await `stop` before ending the runtime;
dropping a Rust value cannot perform asynchronous cloud cleanup.

| RPC ledger row | Service entry point |
| --- | --- |
| `device.configure` | `configure` |
| `device.list` | `list_request`, dispatching `inspect`, `update_tool`, `retry_host`, or `list` |
| `device.testHost` | `test_host` |
| `device.easAccount` | `eas_account` |
| `device.simlockIdentity` | `simlock_identity` |
| `device.open` | `open` with `OpenContext.workspace_root` |
| `device.close` | `close` |
| `device.shutdown` | `shutdown` |
| `device.detail` | `detail` |
| `device.action` | `action` |
| `subscribeDeviceState` | `state_stream` (initial snapshot and subsequent revisions) |

Service snapshots contain all host kinds in `hosts`, like the TypeScript decoded
type. **Apply `service::wire_state` before serializing RPC results or stream
events.** It implements the TypeScript schema's encoding transform: machine
hosts in `hosts`, cloud summaries in `easHosts` and `simlockHosts`.

With `features = ["mcp"]`, `mcp::DeviceAdapter` implements
`t3_mcp::services::DeviceService` for `device_list`, `device_open`, `device_close`,
and `device_screenshot`. Supply `mcp::Threads` for live thread shells and workspace
roots. Mutations recheck environment access and thread dispatch under the service
lock, including after boot. Quick-start text has four exact TypeScript goldens.

`proxy::DeviceHubProxy::router` mounts `/api/device-hub/{*path}` in Axum. Supply
`Authenticator` that checks the requested read/operate scope, cookies, bearer
credentials, and WebSocket tickets using the server's authorization service.
It has no permissive default. The proxy allowlists routes, removes client
credentials and upstream cookies, bridges binary WebSockets, streams HTTP
bodies, and repeats a completed MJPEG still frame once after 100 ms.

## Ported behavior

The host registry, reconciliation, consent gates, inspection and retry, partial
discovery retention, boot progress, authoritative emulator identifiers, thread
claims, release-before-close, and state fan-out port `DeviceService.ts` and
`DeviceMultiHost` behavior. `actions` and `hub` port `DeviceActions.ts` and
`DeviceHubOperations.ts`, including recorded adb/simctl output, screenshot fallback,
Android orientation/permissions, and ffmpeg/libx264 admission.

`local` ports SDK discovery, camera/runtime environment, hub supervision, cached
daemon health and legacy daemon stop from `LocalDeviceHost.ts`. `ssh` ports
`SshDeviceHost.ts`, `localSshDeviceHost.ts`, and `sshDeviceScript.ts`, including
self-host detection, port-forward ownership, health failure reset, three-miss
reconnect and bounded retries. It does not terminate local processes discovered
by a stored PID or command-line pattern.

`cloud::expo` uses the original GraphQL operations, credential precedence,
schema checks, safe error classification and 30-second requests. `cloud::eas`
ports slots, the persistent owned-session ledger, uncertain-create retention,
reaping, retries, preview routing and credential retention for failed stops.
`cloud::simlock` ports origin-bound identities, catalog filtering, slots, CLI
sessions, failed/cancelled-open cleanup, two-minute renewal without a TTL override,
404 removal and maximum-age release. `project` conservatively scans static Expo
project IDs without executing application configuration.

`tools` ports pinned installs (hub 0.22.1, agent-device 0.21.36), npm/pnpm fallback,
staging/sentinels, inventory and maintenance pruning. `agent` ports stable target
paths, private atomic configuration, session release, and bound/unbound launchers.
`FileLaunchers::bind` refreshes the unbound shim in `device/bin`. Supply
`ToolchainAgentCli` as its `cli_provider` to install the CLI on first use;
`NativeHostFactory` supplies that provider to Simlock automatically.

The three embedded compatibility payloads are retained from the TypeScript:
`launcher.mjs` from `AgentDeviceShim.ts`, and `remote_environment.sh` plus
`remote_bootstrap.cjs` from `sshDeviceScript.ts`. They preserve the agent CLI and
remote bootstrap protocols; this is not a removal of Node from device tooling.
The remote bootstrap still requires a supported Node runtime and manages its
recorded remote children. Rust starts and owns each local process root through
`t3-process`, including Node, npm, SSH, adb and xcrun. Batch arguments use its safe
Windows handling; no manually assembled `cmd.exe` argument string is used.

## External seams and native implementations

| Seam | Implementation / responsibility |
| --- | --- |
| `io::Commands`, `io::Spawner`, `io::ManagedChild` | `NativeCommands`, `NativeSpawner`: `t3-process`, bounded collection, timeouts, owned-child cleanup |
| `io::Http` | `NativeHttp`: reqwest streaming, request/body deadlines, optional size limits, no redirects |
| `host::{DeviceHost, DeviceOperations, Routing, SessionRelease, HostFactory}` | Local, SSH, EAS and Simlock implementations; `NativeHostFactory` composes remote hosts |
| `local::{Machine, Ports}` | `NativeMachine`, `NativePorts`: SDK/environment/filesystem probes and loopback port allocation |
| `ssh::Dns` | `NativeDns`: bounded resolution for self-host detection |
| `tools::ProcessInventory` | `NativeProcessInventory`: read-only PID/command inspection for pruning; unknown inventory preserves installs |
| `host::AgentCliProvider`, `service::ToolInstaller` | `ToolchainAgentCli`, `Toolchain`: pinned tool provisioning |
| `agent::Launchers` | `FileLaunchers`: isolated files and CLI release |
| `cloud::expo::{ExpoApi, ExpoProvider}` | `ExpoHttp`, `SavedExpo`: API and credential resolution |
| `service::{Settings, Accounts}` | `MemorySettings` for isolated use; `AccountService` for Expo/Simlock; durable settings adapter belongs to server composition |
| `cloud::Secrets` | `LiteralSecrets` supports literals and refuses references; supply the owning secret resolver for references |
| `service::Admission`, `mcp::Threads` | Application authorization and live thread state; MCP adapter supplies admission checks |
| `proxy::{Authenticator, SocketConnector}` | Server auth adapter; `NativeSocketConnector` uses tungstenite with cancellable connection ownership |

## Integration remaining and intentional differences

The owning server/config/MCP crates must construct these adapters, subscribe
settings changes to `reconcile`, register the RPC methods, map typed errors, apply
`wire_state`, mount the proxy, and await shutdown. Those crates were not edited.
No durable settings, secret-provider, thread-store or authentication implementation
is duplicated here. There is no automatic settings watcher or executable entry
point in this crate. These are composition requirements, not missing device
service methods.

Differences from TypeScript:

- Local stale-PID reaping is omitted because this task permits terminating only
  owned children. Old local tools may require their own normal shutdown path.
- HTTP URLs must be HTTP(S), have no userinfo/control characters, and Simlock
  gateways must be origins with no query or fragment. Redirects are refused.
  Proxy paths also reject encoded or nested-encoded dot segments and separators
  before authorization. Local and private-network Simlock origins remain supported.
  Option-like, empty, NUL/CR/LF selectors and unsafe Windows launcher paths are
  refused before execution, including externally supplied EAS IDs and Simlock models.
  Lease records reject path segments and command-like IDs; URL actions reject
  empty, NUL/CR/LF and option-like arguments. Odd display names stay visible.
- Public errors retain contract tags where available but omit raw causes,
  command output and credentials. Some guidance is shorter; host ambiguity uses
  Rust list formatting. Account checks carry the contract account/identity error
  tags with safe messages. No Effect spans/log messages are
  reproduced. Credential-bearing service values have redacted or absent `Debug`.
- Open/configure/close/shutdown run in owned Tokio tasks and finish if their
  caller drops. This preserves cleanup/state consistency, but differs from
  interruption of TypeScript local operations. The runtime must remain alive.
- Session timestamps use nanosecond ISO strings instead of millisecond strings
  to distinguish replacements during stale-list reconciliation. Session names,
  IDs and on-disk formats otherwise retain their TypeScript forms.
- Readiness and mutation commits share a service lock; discovery is limited to
  four concurrent hosts, matching TypeScript's lifecycle serialization. SSH endpoint
  snapshots use a separate short-lived lock from startup. Fan-out is an unbounded per-subscriber channel; consumers
  must drain or drop it. The MJPEG tracker copies bounded buffers instead of
  retaining zero-copy chunk views.
- The static project scanner is conservative and may refuse dynamic/ambiguous
  configurations; it never runs JavaScript. Rust strings cannot preserve lone
  UTF-16 surrogates. Expo enums refuse unknown session states, matching the current
  generated schema rather than guessing how a future state should behave.
- Process errors, cancellation and Windows batch encoding follow `t3-process`;
  unsafe batch arguments fail before spawning. Default output collection is
  bounded at 8 MiB per stream (screenshot fallback allows 64 MiB). Exact native
  process error text and termination timing are not TypeScript-compatible.
- Socket upgrade/framing uses Axum/tungstenite; connection is bounded to ten
  seconds. Both directions enforce a 100 MiB frame/message limit. The upstream
  cap is stricter than Node's global WebSocket. WebSocket compression is not negotiated, unlike the TypeScript
  server's optional permessage-deflate; the Axum/tungstenite bridge does not
  implement that extension. Framework-specific rejection bodies,
  fragmentation safeguards and WebSocket close reasons can differ. HTTP proxy
  streams deliberately retain the TypeScript's lack of a total
  deadline; dropping the downstream body cancels its upstream read.
- Windows SDK PATH merging respects the existing key's case; pruning compares
  active Windows paths without case sensitivity and accepts either separator.
  Maintenance unlock normally awaits asynchronous filesystem operations, with
  blocking-pool cleanup if its future is cancelled.

## Verification

The default suite has 107 tests; enabling `mcp` adds seven (114 total).

Tests use fakes and recorded TypeScript outputs for every device/tool/API seam,
paused Tokio clocks for renewals/reconnects, and explicit events for concurrency.
The only real subprocess fixtures are Node running a temporary fake CLI via
`t3-process`; real network tests use owned ephemeral loopback listeners for
WebSocket binary forwarding and cancellation. No emulator, simulator, adb, xcrun,
ffmpeg, npm/pnpm install, SSH host, Expo API, Simlock gateway or connected device
was exercised. macOS/iOS runtime behavior and POSIX launcher execution remain
unverified on this Windows machine; the paths compile without macOS tooling and
are driven through fakes here.

From `rust/`, set `CARGO_TARGET_DIR=D:\t3code-rust-target\server-i`, retain the
workspace job limit, then run `cargo fmt -p t3-device`,
`cargo clippy -p t3-device --all-targets -- -D warnings`, and
`cargo test -p t3-device`. Also run Clippy/tests with `--features mcp` for the
optional adapter. No subagents, product dev server, commits or pushes were used.
