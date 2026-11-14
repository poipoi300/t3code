# t3-mcp

Native MCP server and provider callback boundary. This crate does not target WASM.

OAuth integration: the default `auth` feature provides `OAuthAuthenticator::new`
over the HTTP owner's `EnvironmentAuth` and the exact public `/mcp` resource URL.
Install it in `HttpState.clients`. Requests fail closed without an authenticator.
It delegates token verification to `t3-auth::SessionStore::verify_mcp`: native
signature, signed expiry, persisted row expiry/revocation,
MCP subject and bearer method, and refuses proof-bound/browser/WebSocket tokens.
Each request must match the configured resource origin/path. A signed `aud`
claim naming that resource is mandatory; a signed `resource` claim, if present,
must also match; array audiences are refused. `EnvironmentAuth::issue_mcp_session`
and the OAuth exchange now issue the signed audience consumed by this verifier.
Directly issued MCP tokens succeed; generic sessions without an audience remain
refused. Tests exercise the real auth implementation, including live read grants
and tool-specific operate/settings/source-control grants.
`AuthenticatedClient` carries the verified row scopes separately from runtime
modes; all tools require orchestration read, mutations also require operate,
settings mutations require settings write, and worktree handoff/PR guide writes
require source-control write. No tool call trusts a token's claimed scope list
over the live session row. OAuth verification tests cover every tool admission.

`McpServer` combines `Services` (one async trait per toolkit) with `AccessStore`.
`transport::router` serves `/mcp`; `SessionRegistry` issues process-local hashed
credentials, with independent browser/device capabilities, idle expiry, trusted
touch and revocation. `ProviderSessionConfig::http_server` supplies the provider's
endpoint and headers. `PeerService` and `ArtifactStore` are integration seams for
linked environments and saved screenshots. Service implementations must call
`CallContext::check_dispatch` under their command lock before thread writes;
environment writes recheck with `recheck_environment` after waiting for a lock.

The catalog includes every toolkit: attachment 3, device 4, environment 2, HTML 2,
message 1, orchestrator 20, preview 12, preview controls 2, project 7, pull requests 9,
theme 2, thread 7, tool search 1, worktree 3 (75 total across 14 toolkits). Normal listing omits `tool_search`;
deferred provider listing starts with four discovery tools. Capability checks,
read-only clients, caller liveness, linked-origin boundaries and dispatch mode
escalation refusal are implemented separately from discovery.

`bridge::run_stdio_bridge` relays raw foreign JSON-RPC to HTTP, preserving
Unicode and session headers, serializing the handshake while allowing replies,
cancellations and concurrent tools to pass open SSE streams. `with_tool_session`
offers initialized scoped tool calls, answers server ping/unknown requests and
terminates even after a failed or cancelled handshake. Cancellation cleanup is
spawned on the current Tokio runtime, which must remain alive to complete DELETE.
`terminal_result` removes duplicate JSON text for CLI output; the CLI entry point
and provider process launch remain the host's responsibility.
`ProviderSessionConfig::acp_environment` supplies the ACP endpoint/credential.

`export.ts` imports the production toolkit declarations and the real TypeScript
HTTP server, and extracts tool names, descriptions, annotations, decoded and
advertised schemas, and provider instruction builders. It writes only this crate.
From `apps/server`, run:

```powershell
node ../../rust/crates/t3-mcp/export.ts --check
```

Omit `--check` to regenerate. It also generates `services.rs` and its routing test
using available `t3-contracts` input/output types. Twelve access/exposure modes
are captured for both protocol revisions. No advertised schema is approximated;
Rust `tools/list` values and SDK serialization are compared with these fixtures.
Instruction fixtures cover Codex browser/device contexts, Claude system prompt
assembly, ACP interaction/capability transitions, and shared runtime/prompt text.
No production model-facing wording is changed by this port.

The SDK is pinned to `rmcp = 3.10.0`, the latest stable version reported by
https://crates.io/api/v1/crates/rmcp on 2026-11-13. Its upstream is
https://github.com/modelcontextprotocol/rust-sdk/. Its streamable HTTP transport
supports both `2025-06-18` and `2026-07-28`; `Tool` accepts explicit JSON schemas
without deriving/reformatting them. The real SDK HTTP parity test verifies exact
tool lists, modern per-request metadata, the legacy initialization fallback and
DELETE termination. Authentication and T3 visibility remain outside the SDK.

`apps` ports external MCP app request dispatch, app-callable tool checks, fork
lineage ownership, bounded model context, its SQLite store, and HTML snapshots
from `apps/server/src/mcpApps/`. It uses `t3-persistence` for SQLite access.

`adapters::ServiceAdapters` composes concrete sibling bindings without replacing
one toolkit's other bindings. Enable `orchestration`, `workspace`, `scheduler`,
`git`, or `forge` independently; none is a default feature. The complete 75-tool
binding inventory is [fixtures/bindings.json](fixtures/bindings.json).
There are 21 optional concrete routes (some with unavailable branches), 20
preview/device/HTML host-trait routes, 32 missing sibling-service routes, and
two built-in routes (message and discovery). Each inventory entry lists its actual native
service calls or the missing method returned by the fallback.
An absent adapter or unavailable branch returns `isError: true` with
`structuredContent: {available: false, tool, missingServiceMethod, error}`.

`services::MessageService::send_to_user` uses `message::send_to_user` by default.
It returns the exact Markdown and whitespace as `sentMessage`, plus the acknowledgement
extracted from the TypeScript handler, without creating an orchestration command
or a separate conversation-message row. Caller liveness is required; approval-required
and plan-mode provider callers are allowed. Input length is 1..4,000 UTF-16 units.
Provider ingestion persists the existing `dynamic_tool` item and its compact
`sentMessage` output; the client presents it as assistant-authored text.

The merged TypeScript also includes the theme supplement. Theme read/set declarations
and their service trait are generated; set requires environment-write authority and
valid theme/appearance input. `t3-server::EnvironmentThemeService` is not yet a native
service available to this crate, so publication/selection returns the named unavailable
result until a host implements `services::ThemeService`. Such implementations must
hold the caller's thread lock and call `CallContext::recheck_environment` before apply.

The exporter checks the current source context budgets: Claude 5,930 bytes, Codex
7,324 bytes, and MCP list 49,806 bytes (48,566 before the two-tool theme supplement).
Only `request_secret` and `send_to_user` have Anthropic always-load metadata.
Remote-machine instructions and running-task guidance come from the current source;
the shared `next` contract also accepts guidance from other server versions.

Available native calls:

| Adapter | Tool methods | Native service calls |
| --- | --- | --- |
| `OrchestrationAdapter` | send, wait, interrupt/stop | `ThreadManagementService::{send_to_thread, wait_for_thread, interrupt_thread, stop_thread, stop_delegated_tasks}`; stop retries inspect their persisted command events |
| `OrchestrationAdapter` | metadata update, configure, organize, pending request, transfer, queue | `Dispatcher::dispatch_with_limit`, `ThreadManagementService::records`, `ProjectionReader::shell`, `queued_runs_in_delivery_order` |
| `OrchestrationAdapter` | workflow stop, background wait | `ThreadStopService::{stop_workflow, await_background_tasks}` |
| `OrchestrationAdapter` | project list/read | `stores::projects::{list, get}` over `StoreDatabase` |
| `OrchestrationAdapter` | attachment send | `ThreadMessageIntake::send_to_thread` with `AttachmentBackend` |
| `AttachmentAdapter` | prepare/discard | `AttachmentStore::{issue_upload, delete_pending}`; discard requires the preparing caller |
| `SchedulerAdapter` | list/delete/update/run now | `ScheduledTaskService::{list, get, delete, upsert, run_now}`; bound task modes also use `ProjectionReader::shell` |
| `WorktreeAdapter` | list | `Git::list_branches` after resolving the target workspace |

Project reads map persisted rows to the public `Project` contract. Repository
identity and discovered favicon enrichment from `ProjectEnrichmentService.ts`
remain unavailable; explicit persisted favicon overrides are preserved.

Missing sibling methods (defaults name each exact tool in the binding inventory):

- `t3-orchestration::OrchestratorMcpService::{orchestrator_capabilities,
  delegate_task, task_status, task_cancel, request_secret, create_threads,
  t3_thread_list, t3_thread_read, t3_environment_links, t3_environment_link,
  t3_environment_unlink}`. Native delegation, secret delivery, peer setup and
  MCP timeline formatting have no corresponding aggregate service yet.
- `ThreadManagementService::{t3_thread_search, settle_thread}`. The existing
  search is per-thread; settlement lacks deferred own-agent settlement admission.
- `ProjectService::{t3_project_create, t3_project_update, t3_project_delete,
  t3_project_clone, t3_thread_launch}`. Low-level project decision/commit primitives
  do not implement filesystem registration, cloning or launch policy.
- `ThreadPullRequestService` is an empty sibling module. All nine PR tool methods
  remain unavailable; `t3-forge` implements forge operations, not thread link,
  watch or guide policy. The `forge` feature reserves that dependency boundary.
- `WorktreeMcpService::{t3_worktree_status, t3_worktree_handoff}`.
- `t3-config::EnvironmentMcpService::{t3_environment_read,
  t3_environment_preferences_update}`.
- Schedule creation and binding transitions need
  `OrchestratorMcpService::{schedule_task, scheduled_task_workspace_strategy}`
  to resolve project defaults and Git base/workspace policy. Existing task edits
  are supported. Thread wait does not yet return delegated wakeup advice.

`services::{PreviewService, PreviewControlsService, HtmlService, DeviceService}`
are the host traits for `t3-preview` and `t3-device`. Browser/device processes and
`apps/server/src/htmlRender/` publication/rendering are intentionally not
implemented here. MCP app snapshots and dispatch from `apps/server/src/mcpApps/`
are implemented separately in `apps`.

`provider_sessions::ProviderCredentials` ports credential reuse/rotation from
`ProviderSessionManager.ts`: `prepare` reserves before asynchronous validation,
returns a cancellation-safe `PreparedCredential`, and `publish(runtime_id)`
publishes ownership before releasing the reservation. `release_runtime`,
`detach`, `disable` and `touch` implement targeted cleanup and trusted liveness.
Provider, subagent, preview/device access changes and expired/revoked tokens
rotate credentials; stale cleanup cannot erase a replacement. The host resolves
the desired `CredentialRequest`; this module never launches provider processes.
**Integration blocker:** native `ProviderSessionManager::SessionCredentials`
returns only `{id, issued}` and reserves *after* `prepare().await`. It needs a
reservation lifetime plus publication/drop hooks before it can safely use this
implementation; an adapter to that lossy interface would reintroduce the release
race. Until then hosts can hold `PreparedCredential` directly across provider
opening and call `publish` / `release_runtime` at their lifecycle boundaries.
The generated JSON decoder preserves the exported schema, string trimming,
option-record conversion, JSON-text schedules and clamped read limits, and
validates decoded constraints including UTF-16 string limits,
but does not claim equivalence of every Effect refinement or decode-error text.
Local MCP scope, access, catalog, and inline input wrappers are crate API types;
no replacement domain contract types need moving to `t3-contracts`.

Checks from `rust/`, always with the assigned `CARGO_TARGET_DIR`: `cargo fmt -p
t3-mcp`, `cargo clippy -p t3-mcp --all-targets -- -D warnings`, `cargo test -p
t3-mcp`. Tests use fixtures, service fakes, isolated SQLite, and ephemeral HTTP;
they never access provider stores or live T3 state. Windows is the verified host.

Current specification: TypeScript merged at `817a9186f9`, including the theme
supplement. Fixture regeneration check, formatting, default Clippy with warnings
denied, and all 36 default Rust tests pass. TypeScript's AgentContextBudget test
also passes (two cases). New witnesses cover verbatim message output, UTF-16
boundaries, plan/caller liveness, theme authority and rechecking, mixed-version
`next`, always-load metadata, budgets and exact modern/legacy HTTP tool lists.
The current `cargo check --all-features` also passes. The earlier optional-adapter
run passed 35 tests before this catalog refresh;
SQLite integration covers metadata/stop replay, mode changes under dispatch,
public project mapping, uploads and schedules. Paid model evals were not
run: descriptions, model instructions and acknowledgement text are extracted
from the current production source without authored wording changes.
