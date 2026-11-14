# t3-preview

Native server services for browser previews, local server discovery and agent HTML
rendering. This crate does not target WASM. Wire inputs, snapshots, events,
automation results and desktop messages come from `t3-contracts`.

The service logic is Rust. The production browser adapter retains Playwright in
an owned Node process: **Node and `playwright-core` are runtime dependencies**.
Pass the package entry point in `PlaywrightOptions::playwright_module`, or install
it in `PlaywrightOptions::cwd`. No package download happens in this crate. Runtime
worker modules are embedded in the Rust binary and extracted to a private
temporary directory held until the owned child exits. Deployment does not depend
on the build machine's source path.

## WebSocket ledger

| Ledger row | Service method |
| --- | --- |
| `preview.open` | `PreviewManager::open` |
| `preview.resize` | `PreviewManager::resize` |
| `preview.adjust` | `PreviewManager::adjust` |
| `preview.close` | `PreviewManager::close` |
| `preview.list` | `PreviewManager::list` |
| `preview.reportStatus` | `PreviewManager::report_status` |
| `subscribePreviewEvents` | `PreviewManager::subscribe_events` |
| `preview.clearProfile` | `PreviewService::clear_profile` |
| `preview.reportProfiles` | `PreviewService::profile_reporter` → `ProfileReporter::report` |
| `preview.stopLocalServer` | `PortDiscovery::stop_server` |

Keep one `PreviewService` per environment, use its `manager()` for these rows,
and hold one profile reporter per connected client. Dropping the reporter removes
its live contribution without shrinking the saved profile union. Revisions are
global to the manager and events publish inside its mutation lock.

`PortDiscovery::{scan, subscribe, retain, track_agent,
register_terminal_processes, unregister_terminal}` implements the local server
feed. Hold its retention/tracking guards for the relevant host lifetimes.
Configured probes accept only bounded loopback HTTP(S) URLs and never follow
redirects. A stop rechecks the current PID/port ownership and environment,
rejects shared process groups, then signals that exact group. Windows stopping
is unsupported, matching TypeScript. It never kills by name or path pattern.

## Browser and HTML services

`PreviewService::{execute, execute_admitted}` accepts the contract automation
request. It implements status/open/navigation, resize/appearance, snapshots,
actions/evaluation/waits, dialogs, recording and close. Ownership generations,
queued mutations, handoff barriers, coalesced input, pending launch reservations,
8 tabs per agent, 32 agent-created tabs per server, idle reclamation and cleanup
live in Rust. `execute_admitted` rechecks admission inside queued mutations.
`execute` is for trusted internal callers. `cleanup_thread` and `shutdown` release
resources; hold `activity().begin(thread)` for each active provider turn.

`BrowserDriver` is the browser boundary. Fakes implement only its command/reply
and event methods. `prepare` binds a tab to a worker generation before opening;
`call_on` keeps queued commands and viewer cleanup on that generation. Late
disconnects retire only that worker's tabs. `PlaywrightDriver` owns the Node worker through `t3-process`,
drains both pipes, bounds messages and deadlines, and terminates only its owned
process tree. Chromium is the worker's child, controlled by Playwright/CDP.
`ManagedBrowserDriver` adds lazy pinned Chrome installation and host diagnostics.
The installer verifies exact archive size and SHA-256 before extraction, refuses
unsafe ZIP entries, publishes by rename and retains the newest other version.
An interrupted waiter does not cancel the shared installation. Sandbox disabling
must be an explicit host option; failures never trigger a sandbox-off retry.
The shared installation wait precedes the individual browser command deadline.
`with_setup_command` accepts the host CLI's resolved invocation for diagnostics.

`HtmlService::{prepare, read_source, publish, publish_admitted, preview}` covers
bootstrap/theme injection, viewport-unit refusal, local image inlining, guarded
HTML path reads, publication/recovery, optional responsive measurements and PNG
capture. File inputs reject server secrets, state databases, special files,
hardlinked HTML and oversized content. Preview browsers use fresh contexts, a
public-network-only SOCKS proxy, blocked top navigation/popups and disabled
WebRTC. Browser/proxy resources close on failure and timeout, including a browser
launch that finishes after its request expired. Publication cancellation rolls
back the attachment. Browser concurrency is limited to two.
Parsing, regex scans, image inlining and publication writes run on blocking
workers. A write job owns its rollback guard until writing finishes, including
when its async waiter is cancelled.

`McpAdapter` implements all 12 methods of `t3_mcp::services::PreviewService`, both
preview-control methods and both HTML methods. It uses `CallContext`, the live
`AccessStore` and dispatch/environment rechecks; path HTML requires full access.
Recording claims validate pending IDs and size before publishing to the thread.
Use this adapter in `t3-mcp::Services`; there is no second browser service there.

## Host integration ownership

| Seam | Required host implementation |
| --- | --- |
| Electron browser | The desktop shell supplies `DesktopIpc`: bootstrap descriptor IO and a secret, loopback, single-client WebSocket relay for CDP. Forward shell contract events to `DesktopBrowserChannel::receive` and CDP writes through `DesktopBrowserCommand::Cdp`. The shell owns webviews, their sizing and native pickers; it does not launch the server's headless browser. |
| Stream HTTP routes | The HTTP owner authenticates `/api/preview-stream/ws` with orchestration-read, derives `ViewerOptions::can_operate` from preview-operate and `interactive`, disables WebSocket compression and pumps `PreviewViewer::output/input/acknowledge`. `stream::FrameFlow` and `check_socket_buffer` provide one-frame pacing and the 8 MiB socket bound. Use 4404 for gone, 1012 for reconnect and the contract host-setup close code with `stream::host_setup`. |
| Downloads/uploads | The HTTP owner authenticates read/download and operate/upload, parses multipart limits (20 files, 100 MiB total), then calls `open_download` or `answer_file_chooser`. Returned download paths are resolved from saved tab artifacts, never caller paths. |
| Provider lifecycle | Orchestration calls browser cleanup and holds activity guards. The provider credential owner implements `HtmlRenderRecovery`, or connects `MemoryHtmlRecovery::{take, clear}` to credential reuse/replacement/clear. Attachment retention remains the attachment owner's job. |
| Remote/browser hosts | MCP composition must choose the destination environment and automation host before this local service. The TypeScript `mcp/PreviewAutomationBroker.ts` host registry/focus/routing is outside this crate; it is not duplicated here. |
| Browser setup CLI | The CLI owner implements privileged library/AppArmor setup. This crate diagnoses and returns the setup command; it never invokes sudo or changes host security policy. |

`rust/crates/t3-orchestration` currently reports cleanup effects as unsupported;
its owner must call `cleanup_thread`. Its existing HTML-render message handling
does not supply provider recovery ownership.

## Source coverage and differences

The specification is `apps/server/src/preview/` (Manager, SessionControl,
PreviewAgentActivity, DesktopBrowserChannel, PortScanner, PreviewBrowser/Host,
ServerBrowser, ServerBrowserContexts/Page/Stream and PDF navigation),
`apps/server/src/htmlRender/`, their tests, and their WS/MCP/HTTP callers.
`driver/{page,contexts,pdf,proxy,scripts}.mjs` retain browser-side source behavior
with TypeScript/Effect wrappers removed. The worker adapts these helpers; Rust
owns service state and admission. `assets/` contains baked output of the shared
HTML bootstrap/theme helpers. `driver/html-bootstrap.mjs`, `theme.mjs` and
`key-map.mjs` preserve that generation source and are not imported by the worker.

Known differences and limits:

- This is not a Node-free browser implementation. HTML capture uses Playwright
  CDP sessions instead of TypeScript's direct Chrome debugging pipe. Node and
  its Playwright package must be shipped by the host. Real Chrome/Electron rendering
  has not been exercised here; tests never launch a real browser.
- The standalone `PreviewAutomationBroker` host routing, actual HTTP registration,
  Electron bootstrap/relay and privileged setup CLI await their owners above.
  The service methods and explicit integration seams are implemented here.
- Tab IDs use the same sequence/server-epoch scheme as TypeScript. OS error
  details and some browser diagnostic prose are not identical.
- Browser commands/replies have a 150 MiB bound. Interrupted partial writes close
  the worker rather than leaving its protocol desynchronized. IPC setup/sends
  have additional five-second bounds. Interrupted HTML rendering closes owned
  resources; capture has a 19-second worker deadline within the 20-second service
  budget, and measurement uses 5.5 seconds within six seconds.
- A worker exit closes headless sessions and releases desktop connections while
  retaining their manager sessions for shell reattachment. A subsequent command
  starts a fresh worker. The source runs these resources in the server process.
  Unknown/malformed worker events are logged and skipped; oversized or partial
  protocol frames still disconnect the worker.
- Public installer errors and root/sandbox/library diagnosis are ported; AppArmor
  restriction probing, distro package selection and profile installation belong
  to the unported setup CLI. Pruning compares numeric version components rather
  than locale-aware natural sorting of arbitrary directory names.
- MCP calls use this environment's server browser. They do not emit the optional
  website `toolIcon` enrichment performed by the TypeScript broker handlers.
  Screenshot saving/image resizing is the existing `t3-mcp::ArtifactStore` seam.
- `HtmlRenderReference` is **to move to `t3-contracts`** (the source type was in
  shared HTML helpers). Recovery records are local API types. File identity uses
  safe Rust OS APIs; Windows checks descriptor identities and link count, while
  POSIX additionally checks the pre-open device/inode. POSIX process/procfs and
  signal paths are implemented but were not executable on this Windows host.

## Verification

From `rust/`, with the assigned external target directory:

```text
cargo fmt -p t3-preview
cargo clippy -p t3-preview --all-targets -- -D warnings
cargo test -p t3-preview
```

The focused tests use fake browsers, real owned Node processes, local HTTP/SOCKS
sockets, ZIP archives and isolated files. They cover admission revocation,
ownership/handoff races, popup and pending-launch limits, profiles, URL/file/ZIP
refusals, publication rollback, relay cancellation, viewport sequencing,
redaction, stalled CDP cleanup, stream motion/backpressure and installer retry,
retention and cancellation. They do not access live T3 state or start dev servers.
