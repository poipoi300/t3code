# t3-telemetry

Native, transport-neutral telemetry for the Rust server. Not a Wasm dependency.
All client-facing results are existing `t3-contracts` types. No new wire DTOs need
to move to contracts. The crate never discovers an account home or opens provider
credential files implicitly. Its paths, identity documents, consent and
configuration are supplied by the composition root. `identity_for_paths` and
`owners_for_paths` optionally read explicit caller-supplied credential paths;
tests supply only temporary fixtures.

## Server integration

Call `logging::initialize(LoggingOptions::from_config(&resolved, version,
channel, hostname)).await` **once**, before starting domain services. This installs
the global `tracing` dispatcher and configured meter provider. Keep the returned
`TelemetryHandle`; after domains stop, call `handle.shutdown().await`. It joins the
local writer, flushes the three SDK providers concurrently within **one three-second
deadline** (Effect's OTLP exporters flush for up to three seconds; the SDK's own
per-signal timeouts would run in sequence, and metrics ignores its own), then flushes
the local trace buffer. An export still hung at the deadline is left on its own thread
and shutdown returns `Error::Export`.
`build` returns a scoped dispatcher instead, for embedders and isolated tests.
No `RUST_LOG` or OTEL environment discovery supersedes `t3-config`'s resolved levels,
signals or credentials. SDK HTTP requests are restricted to that resolved signal.

Construct `Arc<ResourceTelemetry>` with the server PID, start one `ResourceWorker`
with whether OTLP metrics are configured, and retain it through shutdown.
`worker.shutdown().await` joins collection. An idle server does not walk the process
table: subscriptions request one-second collection, battery changes it to five
seconds, and low-power/serious thermal states to fifteen. Enabled process-group
metrics request collection once per minute even without subscribers. Dropping the
last subscription releases diagnostics demand. Native OS calls run on the blocking
pool; there is no resource-monitor subprocess to install, crash or restart.

Each RPC calls exactly one service method:

| RPC | Service method | Result |
| --- | --- | --- |
| `server.getTraceDiagnostics` | `TraceDiagnostics::read()` | `ServerTraceDiagnosticsResult` |
| `server.getProcessDiagnostics` | `ResourceTelemetry::process_diagnostics()` | `ServerProcessDiagnosticsResult` |
| `server.getHostResources` | `HostResources::read()` | `HostResourcesSnapshot` |
| `server.getProcessResourceHistory` | `ResourceTelemetry::read_process_history(&input)` | `ServerProcessResourceHistoryResult` |
| `server.getResourceTelemetryHistory` | `ResourceTelemetry::read_history(&input)` | `ResourceTelemetryHistory` |
| `server.retryResourceTelemetry` | `ResourceTelemetry::retry()` | `ResourceTelemetryRetryResult` |
| `server.signalProcess` | `ResourceTelemetry::signal_process(&input)` | `ServerSignalProcessResult` |
| `subscribeResourceTelemetry` | `ResourceTelemetry::subscribe()` | initial snapshot plus changes |
| `server.checkOtlpEndpoint` | `OtlpEndpointCheck::check(&input)` | `OtlpEndpointCheckResult` |

`TraceDiagnostics::new` takes the resolved trace filename and backup count.
`HostResources::default` owns the five-second single-flight cache.
`ResourceSubscription::recv` silently skips lagged entries, preserving the sliding
eight-snapshot stream. Its initial snapshot and receiver are acquired under one
lock. Handlers own authorization and RPC cancellation; the subscription must stay
alive until the RPC stream ends. Do not destructure away its demand lease.

`t3-server` wires all nine methods through `adapters/telemetry.rs` and
`handlers/telemetry.rs` under `local-services`. Its binary uses
`runtime::start_with_global_tracing`: ownership is acquired before logs are opened
or pruned. The telemetry domain starts first and shuts down last, after streams
release their leases. Trace diagnostics flush the local sink before reading it.
Library `start`/`start_with` leave the global subscriber to their embedding host;
they still serve diagnostics and resource streams. Configuration reports whether
the file tracer is initialized and which configured signals are exporting.

## Source behavior

* Local spans go through the SDK's batch processor: ending a span only queues it
  (bounded SDK queue, 2,048 spans), and the SDK worker thread redacts, formats and
  hands them to the sink every batch window or each 512 spans, instead of every span
  end running that work behind one processor-wide lock.
* `serverLogger.ts`, `observability/Observability.ts`, shared `observability.ts`
  and `logging.ts`: console logging, level filters, sampled spans, trace events,
  size-based NDJSON rotation, startup backup pruning, bounded batching, shutdown
  flushing, independent OTLP traces/metrics/logs using standard Rust OTEL SDK
  HTTP JSON and protobuf exporters. Files keep the resolved
  `logs/server.trace.ndjson`, `.1` through `.N` names; `max_files` counts backups.
  Defaults come exclusively from `t3-config` (10 MiB, ten backups, one-second
  local batch window, ten-second OTLP interval). Oversized records are dropped;
  buffers are bounded by max(max-file-size, 4 MiB), and disk failure drops its
  current batch. `TraceSink::dropped` exposes loss. `record_browser` accepts
  browser trace records through the same redacting sink.
* Resource attributes preserve `service.name=t3code-server`, version, namespace,
  runtime, managed-by, process-wide service instance, hostname, release channel
  and deployment environment; operator values win for the latter identifiers.
  Rust truthfully reports `process.runtime.name=rust`, rather than Node's version.
* `observability/Metrics.ts`, `resourceTelemetry/ProcessGroupMetrics.ts`: metric
  primitives and name fixture, millisecond histograms with the source's 35
  exponential boundaries, fixed group names, provider-script classification,
  and resetting empty groups to zero. `Metrics` takes a supplied OTEL meter.
* `observability/RpcInstrumentation.ts`: `ws.rpc.<method>` spans, all 220 aggregate
  labels, request counters/duration and interrupt outcomes. The four source
  diagnostic methods suppress nested tracing without disabling their logs.
* `diagnostics/*.ts`: streamed rotated-file aggregation, browser/effect errors
  and interruptions, bounded top/recent summaries, malformed-line counts,
  missing-file/partial-read errors, backend-only process diagnostics and legacy
  history. Signals refuse the server, foreign processes and stale PID/start-time
  identities; the native sampler rechecks identity immediately before signaling.
* `resourceTelemetry/Model.ts`, `ResourceTelemetryHistory.ts`,
  `ResourceAttribution.ts`, `HostResources.ts` and the existing
  `native/resource-monitor/src/main.rs`: subtree discovery with process-age
  checks, cumulative CPU/I/O deltas, 30-second delta safety limit, PID reuse as
  exit/start, Electron deduplication within two seconds, separate backend,
  Electron and monitor groups, historical root identities, boundary proration,
  absent-process baselines and observed window RSS peaks. History retains at
  most one hour, 3,600 snapshots, 20,000 entries and an estimated 64 MiB.
* `HostResources` samples CPU over 200 ms. Linux honors affinity CPU ids, cgroup
  v2 CPU capacity and memory limits, reclaimable cache and host pressure. macOS
  uses `vm_stat` through `t3-process`, with a one-second timeout. Windows uses
  sysinfo's native APIs and available physical memory including standby pages.
  Windows process counters explicitly say `all-io`; other platforms say `storage`.
* `telemetry/AnalyticsService.ts`, `Identify.ts`: persistent anonymous ID,
  SHA-256 legacy distinct-ID priority (Codex workspace, Claude installation,
  anonymous install), independently hashed account/device owners, separate
  1,000-owner caps per observer, consent gates at record **and** send, bounded
  FIFO batches, 10-second sends and ticks, one-second shutdown deadline,
  exponential half-to-full jitter (2-second initial ceiling, 300-second cap),
  30-minute failed-batch expiry and permanent 400/401/413 rejection. Retried
  events retain their UUID and original timestamp. Failed identity persistence
  disables anonymous-only delivery. Supplied provider documents may still
  establish a hashed provider identity, matching the source.
* `telemetry/FeatureUsage.ts`, `ProviderDimensions.ts`,
  `ProviderUsageAnalytics.ts`, `DelegatedTaskAnalytics.ts`: feature allowlists,
  closed handoff settings, provider kind without instance names, reviewed catalog
  model labels, run usage ledger and completion deduplication. Provider reports
  distinguish unavailable, partial and complete usage, root and native children,
  replay, continuation, steering, missing lifecycle observations and late children.
  The 2,048-entry ledger evicts in insertion order. Provider/registry callers
  supply contract objects; the crate does not depend on a provider implementation.
* `observability/EventLoopMonitor.ts`, `HeapAttribution.ts`, `DefectReporter.ts`:
  portable injected-reading stall and heap attribution algorithms, thresholds,
  cooldown, recent-span selection and stable spike/stall span names. Defect
  reporting accepts structural error tags, never arbitrary exception payloads.

`fixtures/typescript-span-names.json` records 2,130 literal Effect.fn/withSpan
names, paths and line numbers from server/shared TypeScript, excluding tests and
dynamic names. Other fixtures contain the metric names, RPC feature mappings
and reviewed vendor catalog labels. They are local fixtures, not edits to the
workspace's shared `rust/fixtures`.

## What leaves the machine

| Destination and gate | Data |
| --- | --- |
| Default `https://ingest.t3.codes/batch/`, only with environment telemetry enabled, policy allowed, current shell discovery complete, readable consent and a distinct ID | event UUID, event name, captured timestamp, SHA-256 distinct/install/owner IDs; OS/platform/architecture, optional WSL distro, app version/channel/mode/client type; the event properties below |
| Explicit resolved OTLP trace endpoint, unless SDK disabled | span names/IDs/parents/kinds/timestamps/durations, sanitized operational attributes/events/links/status and the resource attributes above |
| Explicit resolved OTLP log endpoint, unless SDK disabled | operational log severity/body, sanitized fields and resource attributes |
| Explicit resolved OTLP metric endpoint, unless SDK disabled | configured counter/histogram/gauge series and sanitized labels; fixed process-group totals, with no process names, commands or PIDs |
| An operator's endpoint check | empty selected-signal export; only that signal's currently configured exact URL receives its credentials; redirects are rejected |

Product events/properties supplied by the source callers:

* `server.boot.heartbeat`: thread/project counts.
* `install.owner_observed`: owner source/hash and server/client observer.
* `client.thread.started`, `client.turn.requested`: contract client-origin
  dimensions (`surface`, `appVersion`/`clientAppVersion`, `clientOs`, optional
  mobile `os`, `clientDeviceType`, web-only `webDeployment`/`clientBrowser`,
  mobile `osMajorVersion`/`clientOsMajorVersion` and `deviceModel`/`clientDeviceModel`,
  `connectionMethod`); thread starts additionally include workspace-strategy kind.
* `feature.used`: allowlisted feature, closed variant and optional surface.
* `mcp.tool.invoked`: tool, catalog-filtered caller/target provider/model,
  origin/depth, duration, outcome/stable error code, runtime/interaction mode,
  cross-provider flag and closed handoff settings.
* `mcp.delegated_task.finished`: final status, filtered caller/target dimensions,
  cross-provider flag, completion-wake kind and duration seconds.
* `provider.turn.completed`, `provider.request.completed`: report version,
  provider/catalog model, closed effort/tier, runtime/interaction/origin,
  lifecycle/segment/child/continuation status, token counters, usage completeness,
  confirmed/mixed model flags and duration; no run/thread/turn IDs.
* `chatgpt.auth.started/completed`, `chatgpt.transfer.started/completed`: the
  authentication owner supplies `flow`, auth `intent` and `callbackMode`;
  completions add outcome,
  duration, failure stage, `accountCountScope`, `connectedAccountCount`,
  `connectedConnectionCount`, `savedConnectionCount` and
  `unidentifiedConnectedConnectionCount`. Email and account names are not sent.

`AnalyticsService::record` is the source-compatible low-level enqueue API for
those trusted producers. Its properties must be structural dimensions; never
pass a request payload. Provider and feature helpers make that selection for
their callers. Analytics is enabled by default as in TypeScript, but construction
does not send or create identity files. A missing/unreadable `Consent` snapshot
is fail-closed. Update the shared consent after desktop shell discovery and
policy changes. `T3CODE_TELEMETRY_ENABLED=false` and managed policy denial always
win; the public PostHog ingestion key is not an account secret. OTLP opt-out is
separate from product analytics opt-out, as in TypeScript.

No prompt/file contents, provider credentials, bearer tokens, cookies or query
tickets belong in these APIs. Redaction covers sensitive field names, URL ticket,
DPoP/token/password parameters in a query, fragment (pairing links are
`…/pair#token=…`) or `;` segment, OAuth `code` and `state` inside a URL's query or
fragment (a bare `code=1` stays readable), URL user-info, bearer strings and private
CLI arguments; nested values are bounded and strings truncated. Numeric provider
token counters (`inputTokens`, `total_tokens` and the other usage counters analytics
already exempts, in any of camel, snake or dotted spelling) keep their values in local
traces, logs and OTLP although their names contain "token"; any other value under such
a name is redacted. This is stricter than the TypeScript
field behavior. Process command lines, names and local diagnostic paths are
returned **only** to authorized diagnostic clients as in TypeScript, with known
credential/prompt arguments redacted. They are never analytics dimensions.
Local traces are always local unless the operator separately enables OTLP.

## Native differences and boundaries

Outbound HTTP (OTLP exporters, the endpoint check, analytics) behaves like Node's
fetch: ambient `HTTP_PROXY`/`HTTPS_PROXY`/`ALL_PROXY` are ignored, and the bundled
Mozilla roots are trusted plus every certificate in the PEM file named by
`NODE_EXTRA_CA_CERTS` (read when a client is built; an unreadable file adds nothing,
as in Node).

Inherited from TypeScript: `server.checkOtlpEndpoint` posts to any http(s) URL a
settings writer supplies, so such a session learns whether an address is reachable
from the server and which status it returns. Credentials go only to the exactly
configured URL, redirects are not followed, the timeout is five seconds and no body
is returned; restricting the URL set or the scope is left to a product decision.

The collector runs in-process, preserving the monitor's sysinfo semantics and
protocol-v3 snapshot input through `ingest_native`. Its health therefore has no
sidecar PID/version and no subprocess handshake/backoff. `ResourceSampler` is the
platform/testing seam; retry requests another sample and generation. Its CPU
baseline is established by the first sample; there is no blocking warmup sleep.
`Clock`/`TokioClock`, explicit `*_at` methods and Tokio's paused time support
deterministic cadence/history tests.

Desktop IPC belongs to the shell/composition owner: call `desktop_connected`,
`ingest_desktop`, and `desktop_disconnected` with decoded contract messages.
The worker checks stale data without collecting processes; initial deadline is
90 seconds, later data allows max(90 seconds, idle interval + 30 seconds).
Pass a customized idle interval to `update_desktop_health` when needed. No raw
descriptor adoption, desktop updater, shell environment mutation or native
library FFI is introduced here.

For outgoing control, construct `desktop::DesktopTelemetryControl` from the
host's existing async writer. `announce_server_ready` sends canonical version-1
`serverReady` NDJSON when command admission becomes ready; it does not wait for
provider checks or resource samples. `send` serializes complete control frames
on that writer and propagates I/O failures. Default is a no-op for web/test hosts.
The composition root schedules the announcement after command readiness and keeps
HTTP readiness fallback; desktop/provider startup ordering remains with their owners.

Rust has neither a V8 heap nor Node's single event loop. It does not fabricate
`t3_server_heap_used_bytes`, Node event-loop utilization or V8 heap snapshots.
The portable `HeapAttribution` and `StallMonitor` accept readings from a runtime
owner that can measure those quantities; they are not started against RSS as a
fake heap. `HeapSnapshot.ts`'s `.heapsnapshot` files remain Node-specific.
Rust has no distinct Fatal tracing level; Fatal maps to Error, and All maps to
Trace. Rust SDK scope metadata, default exporter queue/batch limits and console text
format differ from Effect's; signal wire protocols, resource/span identities,
names, configured intervals and shutdown flush remain compatible. Exporter HTTP
calls are bounded and do not follow redirects. Array attributes are serialized
and redacted as strings; trace payloads are stricter and may drop sensitive fields
that TypeScript would have retained. Windows only supports sysinfo's available
signals (SIGKILL; SIGINT can return false); POSIX signal paths are unexecuted here.

## Verification

From the Rust workspace, retaining `.cargo/config.toml`'s configured job limit:

```powershell
$env:CARGO_TARGET_DIR='D:\t3code-rust-target\server-l'
cargo fmt -p t3-telemetry --check
cargo clippy -p t3-telemetry --all-targets -- -D warnings
cargo test -p t3-telemetry
cargo run -p t3-telemetry --example diagnostics
```

Tests use temporary paths and ephemeral loopback collectors only. Coverage ports
behavior from the TypeScript diagnostics, analytics, identity, feature, provider
usage, resource model/history, host, stall and heap attribution tests. It includes
real Windows native collection and standard SDK JSON/protobuf decoding. POSIX
collection and signal delivery require execution on those platforms.

Windows runtime example: `desktopControl={version:1,type:serverReady}`,
`traceRecords=2`, `parseErrors=0`,
`nativeStatus=healthy`, `processes=1`, `hostCpuCount=32`,
`hostMemorySampled=true`, `externalExports=false`.

Review fixes, 2026-11-14: formatting and strict Clippy pass; 33 tests pass, including
fragment/OAuth redaction, batched local spans keeping numeric token counters, consent
withdrawn mid-flush, a redirecting ingest host, the shared shutdown deadline against a
collector that never answers, and a child-process run with an ambient proxy and
`NODE_EXTRA_CA_CERTS` against a local TLS collector.
