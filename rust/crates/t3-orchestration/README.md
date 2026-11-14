# t3-orchestration

## Constructing and registering Codex

Enable `codex-adapter`. Build `CodexAdapterOptions::new(instance_id, clock, ids)` and pass it to
`adapters::codex::codex_registration(options, continuation_key, enabled, credential_access)`.
Append the returned `AdapterRegistration` to the same vector used for Replay in
`AdapterRegistry::new`, then inject that registry as `RuntimePorts.providers`. Mirror the instance
in host settings as `{"driver":"codex","enabled":true}` and select its instance ID in `ModelSelection`.
Construction and registration do not spawn a process; opening a session does.

The host injects:

- `instance_id`, the shared clock/ID generator, and a stable continuation key for instances sharing native history.
- `options.process`: resolved executable, launch arguments, isolated/configured environment and authentication home. The default command is `codex app-server -c thread_unload_delay_secs=0`; the host owns credential/executable leases and the credential-access gates passed to registration.
- `options.track_agent` to acquire/release the host port-discovery running-agent lease for the spawned session.
- `options.version_gate` for native-version compatibility, and `options.mcp` for each app thread's endpoint and authorization header. The adapter preserves other launch-config MCP servers.
- `options.attachments: CodexAttachmentResolver` for authoritative saved paths, document extraction sidecars and readable native image data URLs.
- `options.background: CodexBackgroundHost` for app-owned continuation offers, pending delegated wakes and confirmation that untracked provider-owned processes exited.
- `options.turn_context: CodexTurnContextProvider` for shared provider/MCP instructions, current browser/device availability, cyber-access defaults and managed-runtime service-tier policy.
- `options.usage_policy: CodexUsagePolicy` bound to `t3-provider::account_usage::{merge_codex, codex_reset_at, codex_update}` and the host usage-update sink. Its callbacks must enqueue work without blocking the native reader. This enables ordinary-allowance reset reads and later failure-card updates; sharing caps keep an unknown reset.
- `options.mcp_apps: CodexMcpAppHost` for validated/CSP-filtered stored snapshot leases, and `options.generated_image_markdown` for the shared Markdown image formatter.

These optional host traits and their inputs live in `adapters::codex`; omitted integrations remain
unavailable. The adapter owns native protocol translation, process supervision/stderr draining,
initialization and awaited cleanup. A session serves multiple workspaces: process cwd is cleared,
and cwd/app-thread identity travel on each thread RPC. `CodexAdapterV2::open_connected` accepts
an externally owned native transport; the recorded `CodexReplayAdapter` uses it without Codex turns.

Decoded protocol logging still needs a native-client frame observer plus the host's bounded logger.
The local `CodexProtocolLogger` routing/redaction shim is tested but is not connected during session construction.

## Package 05 session lifecycle integration

Construct `ProviderSessionManager::with_options(registry, Some(ingestor), options,
Some(SessionServices { persistence: Some(SessionPersistence { reader, sink,
clock, ids }), credentials }))`. All ports are injected; the manager owns native
session subscriptions and its tracked maintenance/cleanup tasks. The policy-aware
`SessionDirectory::attach_with_policy(instance, OpenSession, RuntimePolicyOptions)`
seam forwards through the manager to the adapter. Use the returned managed runtime for ensure/resume/start, subscribe,
interrupt and close; bypassing it bypasses busy admission and shared-thread safety.

The event pump persists **only session-scoped artifacts** through its ingestor;
run execution subscribes before starting and ingests run events with run/attempt
guards. This split is necessary for requests arriving before a run and prevents
duplicate ingestion. Session metadata is persisted by the manager. Inject the
same ingestor and dispatcher's `Arc<KeyedLanes>` into `RunExecutionService`.
The composition passes `Some(ingestor.clone())`; omitting it leaves idle
requests/recovered commands without a consumer before run execution subscribes.

Construct `ProviderRuntimeRecoveryService::new(database, reader, sink, sessions,
clock, ids, signals)` with the same manager and sink. `with_restart_settings(settings)`
uses package 06's native restart selector and per-project continuation settings;
`with_restart_selector` injects an explicit selector for another host. Absent either, startup still reconciles interrupted
runs but does not schedule automatic continuation. Construct the layer with
`RuntimeLayer::new(dispatcher, recovery, worker, sessions).with_run_execution(execution)`
so it cancels and joins active runs before removing sessions. Reconcile legacy shells,
await `RuntimeLayer::start()` recovery, then start the effect worker and admit
external commands. Background transcript hydration must be canceled and joined
before closing its database.

Shutdown is split for package 08: call `layer.close_admission().await`, stop
background producers, then await `layer.shutdown_worker()` (prepares restart
intent and stops the worker). Stop/join title, stop and management services while
provider sessions remain available. Finally await `layer.shutdown_sessions()`;
it cancels/joins run execution, closes sessions, then reconciles. Each stage is
idempotent; `layer.shutdown()` runs both stages for standalone callers. Finally join
ingestor/continuation work, drain the event sink and close SQLite. Preparation
must precede session removal so restart eligibility can inspect live projections.
Cleanup callbacks use the raw dispatcher after external admission closes. Native
close has a configurable 30-second default bound; failed/timed-out closes keep
their writer reservation for a subsequent cleanup retry. Shutdown closes independent
sessions concurrently.

An intentional exclusive-session detach fails subscribers after queued events
with the detached app-thread ID. Pooled detach keeps the shared stream open.
The frozen provider error currently carries a marked compatibility payload:
`Protocol.payload._tag = "ProviderAdapterEventStreamError"` and
`detachedThreadId`. The manager implements package 06's `ProviderStreamContext`;
bind `execution.with_stream_context(Arc::new(sessions.clone()))` so run execution
can check this identity and persisted workspace movement before completing a
handoff. Detail text alone is insufficient. The requested native `EventStream`
error variant belongs to the seam integrator. Subscriber cancellation is separate
from the producer's lifetime so failure signals drain; server shutdown explicitly
cancels subscribers.

MCP hosts override `SessionCredentials::prepare_reserved` and return
`PreparedSessionCredential::with_reservation(metadata, reservation)`. The boxed
`SessionCredentialReservation` must own the MCP prepared credential across
validation/open; its consuming `publish(owner)` returns a `PublishedCredential`
guard whose Drop releases exactly that owner. The owner is an opaque unique
lease key, distinct from the provider's logical session ID, so stale cleanup or
same-session reattachment cannot remove a replacement. A `t3-mcp` bridge calls
native `PreparedCredential::publish(owner)` and holds a guard calling
`ProviderCredentials::release_runtime(&owner)` on Drop. Unpublished preparation
is dropped on cancellation/failure; published ownership survives failed native
close until cleanup succeeds. Managed guards bypass the legacy `revoke` path.
The existing `{ id, issued }` prepare/revoke implementation remains supported.

`GuardedLimitRecoveryArmer::new(sink, engine, clock, ids)` implements the arming
port for `ProjectionLimitRecoveryState::new(projections, settings, armer)`. It
rechecks recovery identity and Stop on the writer transaction and invokes the
ordinary pure metadata decision there; do not wrap an unguarded dispatch as an
armer or take the dispatcher's lane and recursively dispatch into that same lane.

## Package 08 lifecycle integration

Construct `ThreadManagementService::new(ThreadManagementServices { reader,
dispatcher, committer, database, settings, hydrator, worktree_restore, signals, clock })`. Clone this
service to share its export admission limit and shutdown scope. Construct
`ThreadStopService::new(ThreadStopServices { management, sessions, agent_scope,
gate })` with the dispatcher's **same** `Arc<KeyedLanes>` as `gate`; alternatively,
`with_dispatcher_gate(management, sessions, agent_scope)` obtains it from the
management dispatcher's `command_lanes()`. The stop service exposes
`stop_workflow`, `force_stop_thread`, and `await_background_tasks` to handlers.

Use `ThreadTitleRegenerationService::new(management, database, settings, generator)`
instead of a struct literal. Execute titles with the effect's original command ID
and the runtime cancellation token.

Production settlement uses `ThreadSettlementService::with_backends(core,
ThreadSettlementBackends { workspaces, pull_requests, identities, resources,
scripts, closes_pull_request })`. The matcher must use the configured host's
close/merge command definitions. `new(core)` supports persisted-link/inactivity
sweeps without host lookup or cleanup. Wrap the service in `Arc` and call `start`
after server activation. Its merge receiver currently supplies a reference,
so the service obtains terminal timestamps through a strict summary read.

Construct `PullRequestWatchReactor::new(PullRequestWatchServices { projections,
commands, pull_requests, clock, ids }).with_identity_registry(identities)` and
`PullRequestSyncReactor::new(PullRequestSyncServices { projections, commands,
committer, pull_requests, identities, policy, clock, ids })`. Both take an `Arc`
for `start`. The watch starts immediately and every two minutes, with scoped host
change subscriptions; sync starts immediately and every minute. Bridge the host's
PR and Git global state changes to `request_sync`. The required
`PullRequestSyncPolicy` supplies authoritative sibling URLs, avatar identity and
shell command observations. No approximate production policy is installed.

Construct `ThreadPullRequestService::new(ThreadPullRequestServices { projections,
commands, committer, workspaces, pull_requests, identities, repository_selector,
linked_threads_change, opportunistic_work, ids, clock })` in an `Arc`. The repository
selector must preserve each host's rules, including Azure's decoded repository
name. The linked-thread detector comes from the PR application's event policy;
it distinguishes changes from reorder-only notifications. Call `start` after
activation and use the host background policy for `opportunistic_work`.

Inject the optional `t3-pull-request::OrchestrationPullRequestBackend` through the
existing `PullRequestBackend` seam. That crate owns application reads, host caches
and registration; these services own orchestration scheduling and dispatch.
`t3-project` owns `ProjectService` and managed scratch folders and consumes the
pure project/deletion planners here.

Shutdown order: close external admission and global-feed bridges; await discovery,
sync, watch and settlement `shutdown()`; stop and join the effect worker; await
title `shutdown()`; await stop `shutdown()`; await management `shutdown()`; then
shut down provider sessions and drain the event sink before closing SQLite.
Each service closes admission, cancels active calls and joins owned work.
Management clones share export admission and shutdown. Settlement also cancels
its settle-script token and joins completion observers. The backend must honor
that token and join its own script processes. An accepted SQLite commit can
outlive its canceled caller, so the sink must still drain. Pure project/deletion
planners and the injected deletion cleanup runner own no background tasks.

Package 08 drift verification: 147 owned tests passed, one test per process:
management 15, stop 10, title 9, settlement 29, watch 53, PR sync 9, discovery 10,
project 5, deletion/cleanup 3, runtime 4. These ran against the actual source and
canonical dependencies through an external harness while composition bindings
were incomplete; its composition-only test restore is not a production adapter.
The normal all-feature `ports` test target still needs package 09's HTTP fixture
to supply `worktree_restore`. The host must supply the real workspace restore
through `RuntimePorts::worktree_restore`. This is partial source assertion
coverage. Production PR persistence still requires prepared host
identities in the decision inputs; the current engine shim returns unsupported.
The native command/URL policy, linked-reader detector, global change-feed bridge
and shared project-read scope remain integration requirements.

Native orchestration server core over canonical `t3-contracts` types and migrated
`t3-persistence` SQLite. Twelve packages are implemented concurrently; ownership
and acceptance are in `C:/Users/Administrator/t3code-rust-notes/briefs/orchestration-plan.md`.
This README records package 01's store coverage. Other packages' implementation
status must be assessed separately.

## Runtime composition

`composition::OrchestrationRuntime` is the one entry point a host uses. It wires the packages'
services behind the seam traits; TS sources are `runtimeLayer.ts` and the orchestration phases
of `serverRuntimeStartup.ts`.

```rust
pub async fn OrchestrationRuntime::start(ports: RuntimePorts) -> Result<OrchestrationRuntime>;
pub fn RuntimePorts::new(
    database: StoreDatabase,
    providers: Arc<dyn ProviderRegistry>,
    checkpoints: Arc<dyn CheckpointPort>,
    clock: Arc<dyn Clock>,
) -> RuntimePorts;
pub async fn OrchestrationRuntime::shutdown(&self) -> Result<()>;
```

`start` returns only after recovery succeeded; an error means nothing was admitted. Call
`shutdown` exactly once before dropping the runtime and closing the database. It attempts every
step and returns the first failure.

### Required ports

| Port | Production value |
| --- | --- |
| `database` | `StoreDatabase(t3_persistence::Database::open(path)?)`, migrated |
| `providers` | 05's `AdapterRegistry::from_instance_source(source)`; the host implements `ProviderInstanceSource::instances()` over its provider instances, building each native adapter with its own host options (see "Provider adapters") |
| `checkpoints` | 07's `runtime::checkpoints::GitCheckpointBackend` (`git-backend` feature) |
| `clock` | A clock formatting `Date.prototype.toISOString` text |

### Optional ports and their defaults

Every other field of `RuntimePorts` is public and starts as shown. "Unsupported" means calls
reach a stand-in that fails with `Error::Unsupported("<Trait>.<method>")`.

| Field | Default | What the default means for users |
| --- | --- | --- |
| `ids` | `RandomIds` (`<prefix>:<uuid v4>`) | Production-ready |
| `settings` | Unsupported | `thread.create`, `message.dispatch` with a new provider, launches and settlement fail: they read provider enablement. Supply the settings service (`StaticSettings` for fixed settings) |
| `policy` | `BaseRuntimePolicy` | Turns run with the thread's own runtime and interaction modes, with no project root or reasoning effort. Production: `ports::decisions::runtime_policy::ProjectRuntimePolicy::configured(database, Arc::new(SnapshotRuntimeModes::new(provider_snapshots)), None, overrides, reasoning_effort)`. Whatever is supplied, `start` wraps it: a mode the provider does not offer becomes approval-required (`SupportedModesRuntimePolicy`), and `managed_policy`'s ceiling refuses turns above it and sets `max_runtime_mode` (`CeilingRuntimePolicy`) |
| `attachments` | Unsupported | Messages and launches carrying attachments fail before dispatch; messages without attachments work |
| `cleanup` | Unsupported | Terminal, preview, scheduled-task and attachment cleanup effects fail and retry; thread deletion leaves those resources |
| `workspaces` | Unsupported | Worktree launches fail; settlement branch and PR confirmation is unavailable |
| `credentials` | `None` | Provider sessions start without T3 MCP credentials (05's `SessionCredentials`, `prepare_reserved` for managed leases) |
| `worktree_restore` | `NoWorktreeRestore` | A send to a thread whose worktree was deleted is not repaired first; turn start then fails (TS test behaviour). Production: the host's `restoreMissingWorktree` port |
| `agent_scope` | `None` | Force-stop cannot kill thread-owned agent processes; OOM detection is off |
| `telemetry` | `None` | No usage refresh or completed-turn analytics |
| `titles` | `None` | Title-generation effects fail and retry; threads keep their given title. Production: `t3_textgen::orchestration::OrchestrationTitleGenerator` (feature `orchestration-host`); recipe in the server seam brief |
| `message_context` | `None` | `message.dispatch` is decided without composer context (04's `MessageContextPreparer`) |
| `thread_find_text` | `t3-shared` `NativeThreadFindText` | Production-ready; one store, and one find cache, serves the runtime. `None` makes in-thread find fail Unsupported |
| `terminal_output` | `t3-shared` `NativeTerminalOutputFold` | Production-ready: `runtime.command_output` is present. `None` removes it |
| `search_policy` | `t3-shared` `NativeThreadSearchPolicy::default()` | Production-ready: `runtime.thread_search` is present. `None` removes it |
| `launch` | `None` | `launch_thread` and `launch_client_thread` fail Unsupported. Production: `GitLaunchEnvironment::new(backend, host)` over the host's `LaunchHost` |
| `workflow_files` | `None` | `workflow_script` (`orchestration.getWorkflowScript`) fails Unsupported. Production: `WorkflowFiles { backend, projects_root }` with t3-workspace's contained reader |
| `native_child_work` | `None` | `native_child_work` (`orchestration.getNativeChildWork`) fails Unsupported. Production: t3-provider's Antigravity child-work reader |
| `pull_request_identities` | `None` | Projected PR links keep their persisted identity; no host alias or authority normalisation |
| `pull_requests` | `None` | No PR watch reactor runs |
| `rollback_workspaces` | `None` | Provider-thread rollback effects fail Unsupported |
| `finalization_observer` | Unsupported | After each run, VCS status refresh is skipped (the failure is not surfaced) |
| `managed_policy` | `None` | No ceiling. Production: the loaded `ManagedPolicyDocument`; its `limits.maxRuntimeMode` caps thread creation, mode changes and every turn start (TS `serverSettings.managedPolicy`, the one source) |
| `runtime_mode_ceiling` | `None` | Transitional, removed once the host sets `managed_policy`: applies as the document's ceiling; a different value from the document fails `start` |
| `provider_snapshots` | `None` | Each adapter's own `ProviderAdapter::supported_runtime_modes` applies (none declares one yet). Production: the host's provider snapshot watch, whose `supportedRuntimeModes` win |
| `sessions` | `SessionManagerOptions::default()` | TS timeouts (30 min idle, 15 s interrupt) |
| `worker` | `WorkerOptions::default()` | 4 slots, 5 attempts, 30 s liveness poll |

### Host-side callers (MCP, scheduler): who checks what

A caller that is not a WebSocket client submits through `runtime.commands_for(CommandCaller { thread_id, limit, link_fence })`: the one dispatcher, its lanes, receipts and idempotency, behind the same admission gate as client commands. `CallerCommands::threads()` and `intake()` are `runtime.threads` and `runtime.intake` over that caller. Refusals are typed (`Error::CallerRefused`, `CallerRefusal::code()` is TypeScript's MCP failure code).

| Check | Where |
| --- | --- |
| Credential, capability, read-only client, live caller run, `assertNotLinked` (projects, settings, scheduled tasks) | The MCP layer only, as in TypeScript |
| Target already above the caller's modes (`AboveModeLimit`) | Runtime, under the command lock (TypeScript's orchestrator); the MCP layer also prechecks |
| Requested modes broader than the caller (`Escalation`) | Runtime, on the planned events, as a backstop; the MCP layer refuses first with the same text |
| Link fence (`OutsideLink`): a caller fenced below full access changes only threads its link started; its own thread is exempt | Runtime, under the command lock; the MCP layer also prechecks |
| Managed ceiling | Runtime: the caller's limit is narrowed to it, and planned events above it are refused |

### Restoring files: which writers are quiesced (t3-git review H-2)

Git checks paths before it writes, so anything that writes to the folder during a restore can redirect the restore (a junction made after the check). A file restore refuses, naming each writer, while any of these run: a turn of the thread (`activeRunId`), a running command item, background work the provider reports, and the writers the host reports through `RuntimePorts::thread_writers` (terminals, dev servers, setup and preview scripts). The check runs before the provider rolls back its conversation, so a refusal changes nothing, and the host's writers stay held from then until Git returns. The thread's command lane is not held across the provider's rollback (ingesting its events takes the lane); it is taken after it, the thread's own writers are checked again under it, and it is held until the rollback commits (no new turn, send or queue start). A turn started while the provider rolled back is therefore refused after that rollback. TypeScript restores under live writers; refusing is the Rust hardening. Not covered: processes that left the thread's tracking (fully detached daemons the provider does not report), and anything the user started outside T3 in the same folder. Without `thread_writers` the host's terminals and dev servers are not seen.

### Repository bindings (t3-git review H-1)

`GitCheckpointBackend::with_repository_bindings(store)` makes every checkpoint, restore, clean and diff run against the repository saved for its scope folder; a scope seen for the first time is bound and saved. Bindings are taken when a project is created (`project_command`, or `runtime.bind_repository(cwd)` for hosts that create projects elsewhere), when a launch prepares its workspace (before its setup script and first turn), and on the first checkpoint operation of a scope (the baseline, before any turn). A trusted recreation of a worktree rebinds it. They are stored outside the database in `runtime::repository_bindings::FileRepositoryBindings` (a JSON file under the server's state directory), so the migration head stays TypeScript's. A changed repository refuses (`checkpoint.git.repository-changed`, shown to the user); it is never treated as "not a repository" or rebound automatically. Projects that existed before the store are bound on their next checkpoint operation, trusting what is there then.

### Start-up and shutdown order

Start (TS order): legacy V1 shells (`legacy.reconcile_shells`), provider recovery (`RuntimeLayer::start`), continuation delivery (05's `ProviderContinuationService`, through `Dispatcher::prepare_continuations`), the two delegated-task recovery sweeps (`runtime.delegated_recovery()` reports their failures for the host to log), the committed-event consumers feeding 04's run, provider-turn and child-work handlers (`runtime.lifecycle_failures()`), the effect worker, then the settlement and PR watch producers. Commands are admitted once recovery succeeds. The host then drives `runtime.legacy.import_pending_transcripts()` and its `legacyThreadMigration` lifecycle events.

Shutdown (packages 05 and 08's order): close admission; stop the PR watch, settlement and launch producers, the committed-event consumers and continuation delivery; stop the effect worker and join its daemon; stop the title, stop and thread-management services; then run execution, provider sessions and recovery reconciliation; then drain ingestion and pending commits. The session manager ingests session-scoped artifacts that arrive before a run subscribes; run events reach run execution only.

### Which handle serves each RPC

| RPC or route | Call |
| --- | --- |
| `orchestration.dispatchCommand` | `runtime.dispatch_command(command)` |
| `orchestration.launchThread` | `runtime.launch_thread(input)` (needs `launch`) |
| `orchestration.stopWorkflow` | `runtime.stop_workflow(input)`; typed failure `Error::StopWorkflow(OrchestrationV2StopWorkflowError)`, anything else is a defect |
| `orchestration.forceStopThread` | `runtime.force_stop_thread(input)`; typed failure `Error::ForceStopThread(OrchestrationV2ForceStopThreadError)` |
| `orchestration.subscribeThread`, thread snapshot, history, search-in-thread | `runtime.transport` (`subscribe_thread`, `bounded_thread_snapshot`, `history_page`, `search_thread`) |
| `orchestration.getThreadTitles`, `getThreadLinkMetadata`, `getOlderSettledThreads` | `runtime.projections.{thread_titles, thread_link_metadata, older_settled_threads}` (inherent on `ProjectionStore`; trait methods await 02's forwarding) |
| Shell and thread HTTP reads | `OrchestrationHttpRead { service: &runtime.transport }` |
| `orchestration.subscribePromptSuggestion` | `runtime.sessions.subscribe_prompt_suggestion(thread_id)` |
| `orchestration.subscribeShell` | `runtime.transport.subscribe_shell(input, limits)` |
| `orchestration.subscribeCommandOutput` | `runtime.command_output` (`CommandOutputHub::subscribe`), when `terminal_output` is set |
| `orchestration.searchThreads`, `searchThreadStream` | `runtime.thread_search`, when `search_policy` is set |
| Project create, update and delete (`ProjectService`) | `runtime.project_command(ProjectCommand)` |
| MCP `send_to_thread` | `runtime.intake.send_to_thread(input)` |
| Transcript export (HTTP) | `OrchestrationHttpRead { service: &runtime.transport }` |
| `getTurnDiff`, `getFullThreadDiff` | `runtime.checkpoint_diffs.{get_turn_diff, get_full_thread_diff}` (07); typed `CheckpointDiffQueryError` |

Admission: client commands pass the runtime layer's gate (closed before recovery and during
shutdown); the host's `Lifecycle::admit_command` stays in front. `command_dispatcher()` and
`effect_worker()` return the live, ungated dispatcher and worker, for scenario harnesses that
must drain effects; RPC handlers use the calls above.

### Provider adapters

The host builds each adapter and registers it in the `ProviderRegistry`; the composition does not
construct adapters. Replay: `adapters::acp::dev_replay_registration(clock, ids, streaming_delays)`
(`acp-adapter` feature), instance `devReplay`, driver `acpRegistry`, configured in settings as
`providerInstances.devReplay = { driver: "acpRegistry", enabled: true }`. Codex (`codex-adapter`)
takes host bridges through `CodexAdapterOptions`: `attachments`, `background`, `turn_context`,
`mcp_apps`, `generated_image_markdown`. Claude, Antigravity and the other ACP vendors are listed
under "Provider adapter requests" in the integrator's request file.

### Proof

`tests/runtime.rs` (requires `acp-adapter`) runs on package 12's Replay provider (`devReplay`,
model `multi-turn`) behind 05's registry. It creates a project and thread, sends a message
through `dispatch_command`, watches the run reach `completed` through `transport.subscribe_thread`,
then restarts on the same database and finds nothing to recover. With `git-backend` the same turn
runs in an isolated Git repository on 07's `GitCheckpointBackend` and asserts the captured
checkpoint ref resolves to a commit; without it the workspace is a plain folder and capture is
skipped.

## Store API

All synchronous store functions take `&Connection` and accept the dereferenced
`&Transaction` from `StoreDatabase::transaction`. Event, receipt, position,
projection and outbox writes must share that transaction. Publication and worker
signals belong after commit. Dropping an awaiting future cannot cancel SQLite
work already running on the blocking pool. `StoreDatabase::read` runs its
closure without a read transaction, so a closure that needs one snapshot across
several statements must not rely on it (seam request below).

- `event_store::EventStore`: thread/project append, finite transactional reads,
  raw rows, agent/application high waters, pruning cursor and SQL replay stats.
  `read_paged` and `read_application_paged` return lazy `EventPages<T>`; each
  query uses a pooled reader and at most 500 raw rows. Limits and cursors count
  raw rows, including skipped unknown tags. The finite `read` and
  `read_application` also page by 500 rows per statement but collect every
  event; use the paged reads where memory must stay bounded.
- `receipts`: insert-if-absent, explicit upsert, shared/project/thread getters.
- `projects`: typed event apply (absent keeps, null clears), upsert, reads by id,
  id set and exact workspace root, soft deletes and shells.
- `positions`: stable allocation, persisted or supplied run ordinal, million-slot
  run ranges, run-start reservation, and `normalize`. Compact id-table indices are
  refused: positions are keyed by real ids.
- `outbox`: idempotent enqueue, atomic claims, lanes, title cap, retry
  head-of-line blocking, continuation exclusion, compare-owner updates,
  cancellation, process-loss reconciliation (`PROCESS_LOSS` is the one list;
  `after_process_loss` is the exhaustive per-variant decision) and pruning in
  batches of `PRUNE_BATCH_SIZE`.

### Decisions a maintainer would otherwise get wrong

- **Persisted event form.** Every JSON value the stores write is encoded with
  `t3_contracts::stored::to_value` (the stored dialect: a contract field holding
  TypeScript's `undefined`, `Some(None)`, is omitted, where plain serde_json writes
  the RPC dialect's `null`). The event store then applies
  `src/stores/persisted-event-shape.json`, generated from
  `OrchestrationV2DomainEventJson` by the oracle tool, on append and before every
  typed read: dates become Effect's `DateTimeUtcFromString` encoding
  (`canonical_date`), object keys take schema declaration order (this replaces any
  hand-coded discriminator order), keys the schema lacks are dropped, and a null,
  wrong kind (a compact numeric id where an id string belongs) or invalid date that
  the TypeScript decoder would reject is an error. Regenerate the shape whenever the
  TypeScript event schema changes; the staleness test fails until you do.
- **Numbers.** Rows are decoded by giving every number its JavaScript value first
  (integers kept exact while they fit 64 bits, everything else the double
  `JSON.parse` yields). Typed decoding therefore works whether or not serde_json's
  `arbitrary_precision` is unified in (it is whenever `t3-claude-cli` is built,
  for example with `--features claude-adapter`). Append returns the caller's
  events and never re-decodes them, unless the persisted form differs, in which
  case it returns the persisted form so a later read compares equal.
- **Timestamps.** Outbox and receipt timestamps are canonicalised as
  `DateTime.formatIso` text before they are written or compared in SQL; a string
  that is not a date is an error, as it is for the TypeScript reader.
- **Undecodable outbox rows.** The TypeScript claim autocommits before decoding,
  so an undecodable row stays `running` under the claiming worker and blocks only
  its own lane. `claim_next` runs inside the caller's transaction, where an error
  would roll the claim back and select the same row on every poll; instead the
  row keeps its claim and the next candidate is claimed in the same call.
  `claim_next_reporting` also returns the skipped rows so the worker can log them.
- **Statements.** Fixed statements go through `event_store::statement`
  (`prepare_cached`, cache raised to 64 per connection). Optional read filters add
  predicates, so each variant keeps its index and is cached by its SQL text.

## Compatibility evidence

`tools/verify-event-encoding.ts` runs the real TypeScript stores (event store,
effect outbox, command receipts, project store, turn-item positions) on fresh
in-memory databases with a fixed test clock. No live T3 data is used. From the
repository root:

```powershell
node rust/crates/t3-orchestration/tools/verify-event-encoding.ts            # check
node rust/crates/t3-orchestration/tools/verify-event-encoding.ts --refresh  # regenerate
```

It writes or checks `tests/fixtures/event-compatibility.json`,
`project-event-compatibility.json`, `store-oracle.json` and
`src/stores/persisted-event-shape.json`. `store-oracle.json` records an FNV-1a
hash of the TypeScript sources it depends on and of the generated shape; the Rust
test `checked_in_oracle_output_matches_the_typescript_sources_and_the_generated_shape`
fails when either is stale.

What the oracle compares, with every expected value produced by TypeScript:

- 693 event rows (all 46 event tags, all 25 turn-item tags, 526 JavaScript number
  encodings), byte for byte in all 14 columns. The TypeScript read of each row is
  asserted to encode back to its input, and the Rust test reads the TypeScript rows
  (inserted directly) and compares with that input.
- 77 raw event inputs the TypeScript store re-encodes: seven date spellings,
  non-canonical nested dates, reversed key order, all five metadata keys and a null
  command id. Rows are compared byte for byte, and Rust reads (of its own rows and
  of the TypeScript rows) are compared with the TypeScript reads.
- Rejections: 1,204 corpus candidates fail the TypeScript decoder (707 only for an
  explicit null on an optional key, which is the RPC dialect's `undefined`). Two
  per event and turn-item tag are kept, plus eight raw cases. An explicit-null input
  must produce exactly the TypeScript row of the same event without the nulls (41
  compared byte for byte); any other must be refused or be readable by the store.
- Rust to TypeScript: the Rust test `rows_written_by_rust_for_the_typescript_decoder`
  writes 3,038 rows (the corpus, the raw and rejected inputs, and every labelled event
  sample in `rust/fixtures/stored` and `rust/fixtures/samples`). With
  `T3_STORE_ROWS_OUT` set it dumps them, and `--verify-rust-rows` reads each back
  through the real TypeScript store and compares its re-encoding with the Rust bytes:
  ```powershell
  $env:T3_STORE_ROWS_OUT = "$env:TEMP/store-rows.jsonl"; cargo test -p t3-orchestration rows_written_by_rust
  node rust/crates/t3-orchestration/tools/verify-event-encoding.ts --verify-rust-rows $env:TEMP/store-rows.jsonl
  ```

- 75 date spellings through `DateTimeUtcFromString`. Rust matches every result
  except two forms only V8's legacy parser accepts (`LEGACY_ONLY_DATES`), which
  it rejects; forms V8 reads as local time are left out.
- The outbox: 19 payloads covering all 16 effect variants and both nested unions,
  through enqueue, claims with each option, an undecodable row, next-claimable,
  succeed, fail, retry, cancel, get, list and prune. Process loss covers 16
  variants × 5 statuses with the TypeScript constants. Receipts, project rows,
  shells and reads, and turn-item positions are covered too. Rust replays the same
  operation log (timestamps respelled) and compares every result and table snapshot.

Known divergence, not fixable in this crate: `t3_contracts::provider_policy::
ProviderUserInputAnswers` is a `BTreeMap`, so record keys are written sorted where
TypeScript keeps insertion order (ignored test
`user_input_answers_keep_their_insertion_order`). The same applies to any contract
record typed as a sorted map.

## Coverage ledger

Paths below are relative to `apps/server/src`; Rust paths are relative to this
crate.

| TypeScript authority | Rust implementation / tests | Tests |
| --- | --- | --- |
| persistence/OrchestrationEventStore.ts; orchestration-v2/EventStore.ts | src/stores/event_store.rs, event_json.rs; tests/ports/persistence__orchestration_event_store_test.rs | 18 |
| persistence/OrchestrationEventStore.sequence.test.ts | tests/ports/persistence__orchestration_event_store_sequence_test.rs | 4 |
| orchestration-v2/FoundationPersistence.test.ts; EffectOutbox.ts; CommandReceiptStore.ts; persistence/OrchestrationCommandReceipts.ts | src/stores/outbox.rs, receipts.rs; tests/ports/foundation_persistence_test.rs | 21 (+1 ignored, contracts) |
| orchestration-v2/ProjectStore.ts and .test.ts | src/stores/projects.rs; tests/ports/project_store_test.rs | 3 |
| orchestration-v2/TurnItemPositionStore.ts and .test.ts | src/stores/positions.rs; tests/ports/turn_item_position_store_test.rs | 5 |
| persistence/RepositoryErrorCorrelation.test.ts | tests/ports/persistence__repository_error_correlation_test.rs | **0, blocked** (below) |
| Package fixture scaffold | tests/scaffold.rs | 1 |

Event-store live/projected stream and buffer-budget assertions belong to package
09; Foundation worker/signal, sink publication, maintenance and projection
assertions belong to packages 02/03/09.

`RepositoryErrorCorrelation.test.ts` targets auth-session, pairing-link and
provider-runtime repositories, which are outside this crate's frozen store API and
dependencies (`t3-auth` is not a dependency). It needs reassignment or public
sibling repository and error APIs.

## Verification

From `rust/`, with the assigned build directory. The store must be correct with serde_json's
`arbitrary_precision` off (the default) and must not break with it on (building
`t3-claude-cli`, for example with `--features claude-adapter`, unifies it in until
that manifest changes), so run the tests both ways:

```powershell
$env:CARGO_TARGET_DIR = 'C:/Users/Administrator/t3code-rust/rust/target/agents/orch-b'
cargo clippy -p t3-orchestration --all-targets -- -D warnings
cargo test -p t3-orchestration
cargo test -p t3-orchestration --features serde_json/arbitrary_precision
```

No dev server, paid provider, commit or push is involved.

## Seam changes — 2026-11-13

These are additive contracts. Existing struct literals and required trait signatures are unchanged. New operations default to typed `Unsupported`, except empty commit/ingestion options and retry-aware executor/thread wrappers, which delegate to the original operation. Production implementations are **not** supplied by adding a default method. Package owners must override the methods below in their leaf files. Signatures omit `&self`; `CT` means `CancellationToken`, and all listed service operations are async unless marked sync.

| Addition / signature | Requested by | Implementer / consumer |
| --- | --- | --- |
| `EventCommitter::commit_with_options(CommitRequest, CommitOptions) -> Result<CommitResult>`; `CommitOptions` re-exports existing `runtime::commit::WriteOptions` unchanged | 06, overlapping 03/05 | 03 `runtime/commit.rs`; 06 ingestion and 05 recovery consume |
| `EventCommitter::commit_prepared(CommitRequest, CommitOptions, CommitPreparation) -> Result<CommitResult>`; `CommitPreparation = Box<dyn FnOnce(&Transaction<'_>, CommitRequest) -> Result<CommitRequest> + Send + 'static>` | 03 + 05 merged | 03 sink; 05 legacy importer consumes |
| `EffectExecutionOptions { will_retry: bool }`; `EffectExecutor::execute_with_options(EffectRow, CT, EffectExecutionOptions) -> Result<()>`; `ThreadOperations::{start_with_options, continue_run_with_options}(ThreadId, RunId, EffectExecutionOptions) -> Result<()>` | 03 | 03 worker/executor forwards budget; 06 start and 08 continuation override |
| `ProviderTurnControl::{interrupt(InterruptTurn, CT), interrupt_and_await_terminal(RestartTurn, CT), steer(SteerTurn, CT), apply_selection(ApplyModelSelection, CT)} -> Result<()>` | 03 | 06 control service; 03 executor routes typed requests |
| `CheckpointRollback::rollback(RollbackRequest, CT) -> Result<()>`; `RollbackRequest` re-exports 07's existing request | 03 | 07 rollback service; 03 executor consumes |
| `RuntimeRequestResponder::respond(ThreadId, ProviderSessionId, RuntimeResponse, CT) -> Result<()>` | 03 | 06 response service; 03 executor consumes; preserve the source response error as a cause when adapting to the shared error |
| Sync `DecisionContext::with_inputs(DecisionInputs) -> PreparedDecisionContext`; sync `DecisionEngine::decide_prepared(&PreparedDecisionContext, &OrchestrationV2ServerCommand) -> Result<Decision>` | 04 | 04 dispatcher preloads, decision engine consumes |
| `DecisionInputs` carries configured runtime policy, selection plan, provider-thread background-work observations, related projections, project rows, per-thread/run item ordinals and shell completion timestamps | 04 | 04; no new field on `DecisionContext` |
| `ProjectionReader::query_records(ProjectionRecordQuery) -> Result<ProjectionRecords>`; `message_count(ThreadId, MessageFilter) -> Result<u64>` | 04 (+ 08 title filtering) | 02 optimized SQL; 04/08 consume. Query combines existing `RecordFilter`, message IDs/roles/run IDs/monitor-only filtering, run statuses and include-latest-run |
| `ProjectionReader::runtime_recovery(ThreadId) -> Result<OrchestrationV2ThreadProjection>` | 05 | 02 selective recovery read, omitted collections empty; 05 consumes |
| `ProjectionReader::provider_target(ProviderTargetQuery) -> Result<Option<ControlContext>>` with exact thread/provider-thread/provider-turn and optional attempt/message IDs | 06 | 02 exact historical control read; 06 consumes. No new `ControlQuery` variant, preserving exhaustive leaf matches |
| `RuntimePolicyOptions { reasoning_effort: Option<String> }`, `ConfiguredRuntimePolicy { policy: RuntimePolicy, options: RuntimePolicyOptions }`; sync `RuntimePolicy::with_reasoning_effort(Option<String>) -> ConfiguredRuntimePolicy` | 04 | 04 policy resolver/preloading; 10/11/12 adapters consume |
| `ProviderAdapter::open_session_with_policy(OpenSession, RuntimePolicyOptions, CT) -> ProviderResult<Arc<dyn SessionRuntime>>`; `SessionRuntime::{ensure_thread_with_policy(EnsureThread, RuntimePolicyOptions), resume_thread_with_policy(ResumeThread, RuntimePolicyOptions), fork_with_policy(ForkInput, RuntimePolicyOptions)} -> ProviderResult<OrchestrationV2ProviderThread>`; `start_turn_with_policy(TurnInput, RuntimePolicyOptions) -> ProviderResult<()>` | 04 | 05 session wrappers must forward; 10/11/12 native adapters implement supported reasoning options. Defaults forward only absent reasoning effort |
| `ProviderAdapter::supported_runtime_modes() -> Option<Vec<RuntimeMode>>` (default `None`) | security M2 | 12 declares Kiro, Grok, FX, Muse and Pi lists (TS `supportedRuntimeModes`); composition's `SupportedModesRuntimePolicy` runs any other thread mode as approval-required unless the host snapshot lists it |
| `ProviderRegistry::metadata(ProviderInstanceId) -> Result<AdapterMetadata>`; `list() -> Result<Vec<ProviderInstanceId>>`; metadata re-exports 05's existing type | 05 | 05 registry; 04/05 consumers |
| `ProviderError::Setup(Box<t3_contracts::provider_setup::ProviderSetupError>)` | 05 | 05 registry/credential admission; canonical payload reused |
| `SessionDirectory::detach_thread(DetachSession) -> Result<()>`; `DetachSession { provider_session_id, thread_id, detail, revoke_credential }` | 05 (+ 08) | 05 pooled session manager; 03/08 consume. Existing whole-session `detach` unchanged |
| `SessionDirectory::other_working_threads(ProviderSessionId, ThreadId) -> Result<Vec<ThreadId>>`; `release_unless_busy(ReleaseSession) -> Result<bool>`; `SessionRetentionCheck::keep() -> Result<bool>` | 08 | 05 session manager, 08 management consumes. `ReleaseSession` includes own provider-thread IDs, through-run ordinal, reason/detail and optional `Arc<dyn SessionRetentionCheck>` |
| `TitleGenerator::generate_with_input(TitleGenerationInput, CT) -> Result<String>` | 08 | Host text-generation backend; 08 title service builds thread/project IDs, cwd, project title, message, attachments, previous title, fork title/history, instructions, model selection |
| `PullRequestBackend::{detail(PullRequestRef) -> Result<PullRequestDetail>, activity(PullRequestRef) -> Result<PullRequestActivity>, thread_comments(PullRequestThreadCommentsInput) -> Result<PullRequestThreadCommentsResult>, watch_fingerprint(PullRequestRef) -> Result<Option<PullRequestWatchFingerprint>>}` | 08 | Host forge backend; 08 watch/sync services consume. Contract request/results reused; server-only fingerprint has `status` and `remarks` strings |
| `WorkspaceBackend::repository_identity(PathBuf) -> Result<Option<RepositoryIdentity>>`; `default_branch(PathBuf) -> Result<Option<String>>`; `checked_out_pull_request(PathBuf, String, String) -> Result<bool>` (cwd, branch, URL) | 08 (+ 06 finalization) | Host workspace/VCS backend, 07 runtime adapter; 08/06 consume |
| `SettingsService::get() -> Result<ServerSettings>`; sync `subscribe() -> Result<watch::Receiver<Arc<ServerSettings>>>`; `SettleScriptRunner::run(SettleScriptInput, CT) -> Result<SettleScriptRun>` | 08 | Host settings/project-script services; 08 settlement consumes. Script input is thread/project/worktree; result is `Skipped` or `Started { completion: oneshot::Receiver<Result<()>> }` |
| `AgentScope::{kill(ThreadId), oom_killed(ThreadId)} -> Result<bool>` | 08 + 06 merged | Host thread-owned agent scope; 08 management / 06 ingestion consume |
| `ProviderTelemetry::{refresh_usage(ProviderInstanceId, CT), turn_completed(CompletedProviderTurn, CT)} -> Result<()>`; `EventIngestor::ingest_with_context(ProviderSessionId, ProviderInstanceId, ThreadId, ProviderEvent, IngestionContext) -> Result<()>` | 06 | Host usage/analytics hooks; 06 owns dedupe, context forwarding and task lifetime. Context carries optional command provenance and analytics; completed-turn context mirrors TS origin/segment/model/policy fields |
| `AttachmentBackend::claim_pending(ThreadId, Vec<ChatAttachment>) -> Result<ClaimedAttachments>`; `release_claimed_paths(Vec<PathBuf>) -> Result<()>`; `ClaimedAttachments { attachments, claimed_paths }` | 06 | Host attachment store; 06 intake consumes, including document sidecars and ambiguous-delivery retention |
| Sync `CommandDispatcher::command_lanes() -> Result<Arc<KeyedLanes>>` | 06 | 04 dispatcher exposes its actual lane Arc and adds an injectable constructor/builder in its owned leaf; 06 ingestor receives the identical Arc. Accessor default does not create a second map |
| `unicode-segmentation = "1"` (currently lockfile 1.13.3), matching `t3-client` | 08 | 08 monogram validation |
| `ProjectionReader::turn_start_context(ThreadId, RunId) -> Result<TurnStartContext>`; `api::TurnStartContext` re-exports 02's `projections::control::TurnStartContext` | 02 | 02 `projections/store.rs`: override it in `impl ProjectionReader for ProjectionStore`, forwarding to the existing inherent method of the same name; 06 turn start consumes |
| `ProjectionReader::subscriber_shell_snapshot(Option<ShellIndexSelection>) -> Result<SubscriberShellRead>`; `ShellIndexSelection = Box<dyn FnOnce(&[ActiveShellIndexEntry]) -> Vec<ThreadId> + Send + 'static>`; `SubscriberShellRead { snapshot: OrchestrationV2ShellSnapshot, index: Option<Vec<ActiveShellIndexEntry>> }`; `ActiveShellIndexEntry { id, project_id, lineage, settled_override, settled_at, pinned_at, snoozed_until, may_be_live, has_remote_tasks }` | 09 | 02 store: TS `listActiveShellIndex`, shells by ID, projects and latest sequence in **one** read transaction; the selection runs inside it and must do no I/O; `None` reads every active shell and returns `index: None`; archived shells are not included. 09 `ShellStream` supplies the selection (`shellSubscriberThreadIds`) and applies the trims |
| Sync `EventCommitter::subscribe_shared() -> Result<broadcast::Receiver<Arc<ApplicationStoredEvent>>>` | 09 | 03 sink publishes each committed event once, as an `Arc`, at the same post-commit point and in the same order as `subscribe`; 09 transport subscribers consume. A lagged receiver resnapshots (`snapshot-required`), never skips silently |
| `CheckpointBackend::checkpoint_ref(&CheckpointScopeId, u64) -> Result<CheckpointRef>` (sync); `is_git_repository(PathBuf) -> Result<bool>`; `delete_checkpoint_refs(OrchestrationV2CheckpointScope, Vec<OrchestrationV2Checkpoint>) -> Result<()>` | 07 | 07 `runtime/checkpoints.rs` `GitCheckpointBackend`: move the bodies of its `CheckpointMetadata` impl here, then drop `CheckpointMetadata` and `CheckpointService`'s second `Arc`. Delete rejects the whole batch, deleting nothing, if any ref is not owned by the scope. Names follow TS `CheckpointStore` (`isGitRepository`, `deleteCheckpointRefs`) and differ from 07's local trait so concrete calls stay unambiguous while both exist |
| `RuntimePolicyResolver::resolve(OrchestrationV2AppThread, ModelSelection) -> Result<ConfiguredRuntimePolicy>` (new trait, required method) | 07 (+ 04, 06 use the same TS service) | 04 `ports/decisions/runtime_policy.rs`: registry and project-backed resolver, TS `RuntimePolicyV2` including `layerWithOverride`. 07 rollback replaces its private `RollbackPolicy` with it; 04 orchestrator preloading and 06 turn start consume. Above the managed ceiling it fails as `Error::Service { operation: "runtime-policy.above-ceiling", detail }` (07's `POLICY_CEILING_OPERATION`) |
| `sha2 = "0.10"` (lockfile 0.10.9, the version 12 sibling crates use) | 09 | 09 `ShellStream` hashing (TS `createHash`) |
| `testing::{FakeCheckpointBackend, FakeRuntimePolicyResolver}`; `FakeProjectionReader` scripts both new reads; `FakeEventCommitter::shared_publication` and `publish(ApplicationStoredEvent)`, which delivers to both subscriptions | Integrator requirement | Implemented in `testing.rs` |
| `DecisionInputs::provider_metadata: Vec<(ProviderInstanceId, AdapterMetadata)>`, `provider_session_ids: Vec<(ProviderInstanceId, ProviderSessionId)>`, `live_sessions: Vec<(ProviderSessionId, bool)>` (third batch, 15:09) | 04 (critical path: first message, run interrupt) | 04 loads them in `runtime/dispatch.rs` `CommandInputs::prepare`, before deciding: adapter identity from `ProviderRegistry::metadata(instance)` (TS `providerAdapters.get(instanceId)`), live presence from `SessionDirectory::get(session)` (TS `providerSessions.get(id)`). A session absent from `live_sessions` was not observed; `false` means observed missing. `ProjectionCommandInputs` needs a `sessions: Arc<dyn SessionDirectory>` field, which the composition passes. The decision engine stays pure |
| `DecisionInputs::provider_enabled: Vec<(ProviderInstanceId, bool)>`, `supported_runtime_modes: Vec<(ProviderInstanceId, Vec<RuntimeMode>)>`, `restore_isolated: Option<bool>`, `attachment_ids: Vec<(ThreadId, Vec<String>)>` | 04 | 04 loader (enablement via 08's `provider_instance_enabled`, restore safety via 07's `checkpoint_restore_safety`, inventories via `ProjectionReader::attachment_ids`) |
| `ThreadOperations::resume_queue(ThreadId, Option<RunId>) -> Result<()>` | 04 | Default `Unsupported`. 04 dispatcher calls it after commit and on receipt replay of `queue.resume` / `queued-run.start`; implementer to be named by 04 (TS orchestrator queue start) |
| `ThreadOperations::recover_delegated_task(ThreadId, source_run_id: Option<RunId>, completed_run_id: Option<RunId>) -> Result<()>` | 03 | Default `Unsupported`. 04 implements (TS `Orchestrator.recoverDelegatedTask`); 03 executor calls it after a final failed restart continuation and after successful finalization |
| `ThreadOperations::stop_delegated_for_effect(ThreadId, CommandId, Option<String>)`, `generate_title_for_effect(ThreadId, CommandId, TitleKind)` | Integrator (composition) | Defaults forward to the old methods. The composition implements them with the effect's command id as receipt identity (TS). 03 `ServiceExecutor` must route `delegated-tasks.stop` and `thread-title.generate` through them with `effect.command_id`; until then both effects fail `Unsupported` |
| `ProviderTurnControl::interrupt_with_outcome(InterruptTurn, CT) -> Result<Option<SubagentOutcome>>` | 03 + 06 | Default calls `interrupt` and returns `None`. 06 overrides; 03 dispatches the confirmed subagent settlement |
| `Error::ProviderTurnControl(Box<ProviderTurnControlError>)` | 03 + 06 | 06's `impl ProviderTurnControl` returns it instead of a string `Error::Service`, keeping `turn_completed`, `steer_declined`, `unresponsive` and the cause |
| `icu_collator = "2"` (as `t3-usage`) | 02 | 02 `localeCompare` ordering |
| `uuid = { version = "1", features = ["v4"] }` (as siblings) | Integrator | `composition::RandomIds` |
| `AdapterMetadata` derives `Debug` (mechanical: `DecisionInputs` derives `Debug`) | Integrator | Done in 05's `provider_adapter_registry.rs` |
| `SessionDirectory::attach_with_policy(ProviderInstanceId, OpenSession, RuntimePolicyOptions) -> Result<Arc<dyn SessionRuntime>>` (fourth batch, 16:20) | 05 | Default forwards to `attach` when no reasoning effort is set, else `Unsupported`. 05's manager overrides it with its inherent method; 06 turn start must call it with the configured options (today `ProviderTurnStartService::start` takes a bare `RuntimePolicy`, so the composition drops reasoning effort) |
| `PullRequestBackend::{summary(PullRequestRef) -> Result<PullRequestSummary>, stack(PullRequestRef, bool) -> Result<Option<ThreadPullRequestStack>>, batches_reads(PullRequestRef) -> Result<bool>, invalidate(PullRequestInvalidateInput, bool) -> Result<()>, changes(PullRequestRef) -> Result<watch::Receiver<u64>>, subscribe_merges() -> Result<broadcast::Receiver<PullRequestRef>>, notify_linked_threads_changed() -> Result<()>}` | 08 | Host forge backend implements; 08 sync, discovery, watch and settlement consume. `changes` is a change counter (coalescing); `subscribe_merges` carries merged PR refs. `summary` returns transient failures instead of retrying (TS `recoverTransientFailure: false`) |
| `Error::PullRequestHost(PullRequestHostError)`; `PullRequestHostError::{Failed { transient, detail }, RateLimited { retry_at: Option<IsoDateTime>, detail }, NotFound { detail }, Permanent { detail }}` | 08 | Host forge backend returns these; 08 host pause, missing-PR backoff and the watch's eight-failure ending policy match on them |
| `WorkspaceBackend::{branch_pull_request(BranchPullRequestQuery) -> Result<Option<GitBranchPullRequest>>, repository_identity_with_refresh(PathBuf, bool), invalidate_status(PathBuf) -> Result<()>}`; `BranchPullRequestQuery { cwd, branch, refresh, confirm_terminal, fresh_git_state }`; `GitBranchPullRequest { pull_request: VcsStatusChangeRequest, repository_key, updated_at, closed_at, merged_at, head_sha }` (TS `GitBranchPullRequest`) | 08 | Host VCS/workspace backend implements; 08 discovery and settlement branch confirmation consume |
| `ResourceCleanup::close_on_settle(ThreadId) -> Result<()>` | 08 | Host resource cleanup; distinct from `cleanup(Terminals)` |
| `ProjectionReader::{thread_titles(Vec<ThreadId>) -> Result<OrchestrationV2ThreadTitles>, thread_link_metadata(Vec<ThreadId>) -> Result<BTreeMap<ThreadId, OrchestrationV2ThreadLinkMetadata>>, older_settled_threads(OrchestrationV2GetOlderSettledThreadsInput) -> Result<OrchestrationV2OlderSettledThreadsPage>, runtime_response_context(ThreadId, RuntimeRequestId) -> Result<Option<ControlContext>>, checkpoint_context(ThreadId) -> Result<CheckpointContext>}` (fifth batch, 17:10); `api::CheckpointContext` re-exports 02's `projections::control::CheckpointContext` | 02 (first three block server RPCs) | 02 forwards each from `impl ProjectionReader for ProjectionStore` to its existing inherent method; the server can call the inherent methods on `runtime.projections` meanwhile |
| `RecordFilter::{turn_item_run_id: Option<Option<RunId>>, turn_item_run_ids: Option<Vec<Option<RunId>>>, turn_item_node_ids: Option<Vec<NodeId>>, provider_turn_attempt_ids: Option<Vec<RunAttemptId>>}` | 02 | 02 read SQL. Empty list matches nothing; a `None` entry matches runless items. Every existing literal uses struct-update syntax |
| `StoreDatabase::read` runs its closure in one deferred read transaction | Store owner (L8) | Done. Forced adaptations: 02's `ProjectionStore::read_snapshot` and 12's `ProjectionReplay::verify` no longer open their own (now nested) transaction |
| `OrchestrationRuntime::{command_dispatcher() -> Arc<dyn CommandDispatcher>, effect_worker() -> Arc<dyn EffectWorker>}`; `RuntimePorts::pull_request_identities: Option<Arc<dyn PullRequestIdentityRegistry>>` | 10, 12 (scenario harnesses); 02 (registry) | Composition. The registry, when given, makes the sink use 02's `RegistryProjectionWriter` |
| `Error::StopWorkflow(Box<OrchestrationV2StopWorkflowError>)`, `Error::ForceStopThread(Box<OrchestrationV2ForceStopThreadError>)`; `OrchestrationRuntime::{stop_workflow(OrchestrationV2StopWorkflowInput) -> Result<()>, force_stop_thread(OrchestrationV2ForceStopThreadInput) -> Result<OrchestrationV2ForceStopThreadResult>}` (sixth batch, 19:50) | Server (typed RPC failures) | Composition translates 08's operation codes until 08 returns the variants from `ThreadStopService` |
| `RuntimePorts::credentials: Option<Arc<dyn SessionCredentials>>`; manager receives the ingestor; recovery `with_restart_settings`; layer `with_run_execution` and staged shutdown | 05 | Composition |
| `tests/runtime.rs` requires `acp-adapter` (`[[test]] required-features`) and runs on Replay | Coordinator | Integrator |
| `ThreadWorktreeRestore::restore(OrchestrationV2AppThread) -> Result<()>` (new trait); `testing::FakeThreadWorktreeRestore` (seventh batch, 22:45, upstream drift) | Upstream `runtimeLayer.ts` | 08 adds `worktree_restore` to `ThreadManagementServices`; host supplies the restore; composition binds it once 08's field exists |
| `TurnInput::inline_context_length: Option<usize>` (upstream `inlineContextLength`) | Upstream `ProviderAdapter.ts` | 06 fills it in turn start; 11 uses it for skill-mention dispatch. Mechanical `None` added to the five literals (05, 06, 10, 11, 12) |
| `OrchestrationRuntime::{thread_lanes(), clock(), ids()}`; `checkpoint_diffs: Arc<CheckpointDiffQuery>` | Server integrator (kept); 07 | Composition |
| `ProviderError::EventStream { provider_session_id, detached_thread_id: Option<ThreadId>, detail }` (eighth batch, 00:30, upstream drift) | 05 + 06 (upstream `detachedThreadId`) | 05's manager raises it with `detached_thread_id` when T3 closes a session because a thread detached; 06 consumes it and retires its `ProviderStreamContext` shim |
| `RuntimePorts::worktree_restore: Option<Arc<dyn ThreadWorktreeRestore>>` (stand-in `NoWorktreeRestore`); run execution `with_baseline_checkpoints` | 08, 07 | Composition |
| `testing::Script<I,O>::push(Result<O>)`, `next_call().await -> Option<I>`; `Fake*` implementations for every new service, policy methods on existing fake adapter/runtime, `FakeEventCommitter::new(StoreDatabase)` | Integrator requirement | Implemented in `testing.rs`. Unscripted calls fail with typed Unsupported; prepared fake commits run callbacks on real migrated SQLite and roll back on callback/script errors |

Transaction implementation contract for 03: perform receipt dedupe and guards before invoking preparation; run normalization, callback mutations, marker/position writes, events, projections, receipt and outbox on the **same** writer transaction. Preparation must not replace the receipt identity or weaken the supplied guard; validate any newly prepared guard before append. A guard rejection after callback mutation must roll those mutations back. Publish and signal only after durable commit, retaining the publication lane when the caller drops. The callback is synchronous and must not perform external I/O. The fake demonstrates rollback, not production dedupe, guard enforcement, event append or publication.

Session implementation contract for 05: inspect/remove under the same busy-turn gate used to admit starts. Evaluate `keep_if` while retaining that admission gate; no SQLite connection guard may cross its await. Shared-thread inspection alone never authorizes a subsequent unconditional release. Keep the thread-specific credential revocation scope and exact reason/detail.

Merged/declined requests: 03 and 05 share one transaction callback; 06 commit flags reuse `WriteOptions`; 05 metadata and 07 rollback requests reuse existing types. The requested direct `RuntimePolicy.reasoning_effort` field and new `DecisionContext` fields use wrappers because current leaf literals lack struct-update syntax. The requested `ControlQuery::ProviderTarget` variant uses a companion method to avoid breaking exhaustive matches. Repository-error correlation (01) remains outside this crate: auth/pairing/provider-runtime repository APIs must be supplied/reassigned by their owners; no duplicate repositories or auth dependency was added. Shared command-lane injection (06) requires 04's leaf constructor change; no integrator edit to `runtime/dispatch.rs`. Workflow file-identity work remains 06's future consumer of the existing `t3-workspace` capability; unused `file-id`/`same-file` dependencies were not added here.

Second batch (later on 2026-11-13), packages 02, 07 and 09. Answered by existing seams rather than new ones: 02's provider-control targeting is `provider_target(ProviderTargetQuery)` (06 asked for the same capability); 02's runtime-response thread scoping is `RuntimeRequestResponder::respond(ThreadId, ...)`; 02's missing record filters are `query_records(ProjectionRecordQuery)`, and any filter still missing must be named field by field. 07's shared dispatcher lane for final rollback commits is `CommandDispatcher::command_lanes()`: `CheckpointRollbackService` already takes `Arc<KeyedLanes>`, so composition passes the dispatcher's Arc. 07's reservation and lease helper adoption is internal to 07. Not done here: moving 09's `ShellSnapshotAnswer` (`ports/transport/http.rs`) into `t3-contracts` belongs to the contracts owner.

Third batch (2026-11-13 afternoon): not yet applied. 04's provider-switch / context-handoff plans need an exact type; 04's canonical pull-request identity context needs the host source-control registry, so it belongs with the host ports, not `DecisionInputs`. 06's `ProviderEventNormalizeError` / `ProviderEventPublishError` variants wait until 06 defines those structs. 02's exact signatures (thread-scoped runtime response, checkpoint metadata, two `RecordFilter` filters, registry adapter for the write path) wait for its new heading in `orchestration-seam-requests.md`; for checkpoint metadata, the 2026-11-13 `CheckpointBackend::{checkpoint_ref, is_git_repository, delete_checkpoint_refs}` may already be the answer.

Fourth batch (16:20): 03's pending-delegation repair has no implementation to forward to; TS `Orchestrator.recoverDelegatedTask` offers delegated-completion deliveries and finalizes the app-owned subagent under the thread lock, which is 04's completion-delivery work. The composition forwards `recover_delegated_task` once 04 exposes it (owner 04). 08's source-control registry port and 05's usage auto-arm (`LimitRecoveryArmer`, needs a guarded dispatcher method from 04) and continuation worker are not composed yet.

Fifth batch (17:10): not applied. 02's `CheckpointRollbackContext::target_attempt` element-type change is not additive (02 builds the context; 07 and 10 consume the full attempt) and needs those three owners to agree on one change. 02's reader snapshot token needs a stable reader identity from `t3-persistence` (its owner). Native adapter factories (Cursor, OpenCode, OpenCode2, Pi, Muse, Grok, Kiro, fx) belong to the provider packages outside this crate; the Codex host bridges, ACP flavor hook, diff formatter, runtime-instruction builder, command-output fold and pending-work helper belong to the host or shared owners. Routing is in `orchestration-seam-requests.md`.



## Package 07 host integration

Package 07 current coverage: `CheckpointDiffQuery.ts` maps to
`checkpoint_diff_query.rs` (4 SQLite cases); `ThreadLaunchService.ts` maps to
`thread_launch_service.rs` (19 cases). Checkpoint/fork/safety/branch coordination
has 72 default integration cases and 9 additional Git-feature cases. The normal crate's
checkpoint selection runs 42 cases; native fork/fallback is covered in package
06's existing provider-start cases. Git has 97 tests, including 5 checkpoint drift
cases and the 1,600-file aggregate query. A real current-TypeScript/Rust probe
confirmed both writers' checkpoints can be read by both readers.

`runtime.checkpoint_diffs` serves turn and full-thread diff queries from thin checkpoint
metadata. The injected `CheckpointMetadata::query_diff` must preserve authored-file
filtering, Windows path batching and the shared 10 MB UTF-8 limit; the Git adapter
uses `t3-git::filtered_checkpoint_diff`. Forward `CheckpointDiffQueryError::cause()`
when building the RPC error envelope so range and missing-ref details survive.

The Git adapter passes the preceding scope ref when capturing ordinal > 0. Capture
and diff use TypeScript's `T3-Head-Moved-At` trailer and synthetic second-parent
format; unreadable/older metadata retains the literal baseline. Do not strip these
parents or replace the trailer with wall-clock capture time.


Construct `WorkspaceCoordination::global()` once and pass it to
`CoordinatedTurnStart::new(turn_start, projections, database, coordination.clone())`.
Use that wrapper for every provider start (including retries); it reserves the project
root or worktree through startup and returns the execution handle before settlement.
(Composition does not use the wrapper yet: its turn starts take no workspace reservation.)
Turn start always resolves the policy itself: `start_resolving` (or `CoordinatedTurnStart::start_resolving`)
takes the policy resolver, keeps reasoning effort and settles a ceiling refusal as a failed run
(TS `runtime-mode-above-ceiling`). There is no entry that takes a caller-resolved policy, so a host
cannot skip the ceiling and offered-modes wrap; tests use `testing::FixedRuntimePolicy`.
Wrap the host `WorkspaceBackend` in `CoordinatedWorkspaces::new(backend, coordination)`
for launch cleanup and storage cleanup; direct host removals must instead call
`coordination.cleanup(path, token, removal)`. Rollback uses these same global
reservations and workspace leases. Use `ResolvedRollbackPolicy(policy_resolver)` so rollback
forwards configured reasoning effort through `SessionDirectory::attach_with_policy`.
Give `CheckpointService::new` a **separate**
`Arc::new(WorkspaceLocks::default())` for snapshot locks.

Keep `CheckpointCaptureService::execute` in run finalization after the provider turn
settles; a start reservation must not cover turn execution or finalization. Route
thread launch and prepared-run retry through `ThreadLaunchService`. With `git-backend`,
`GitLaunchEnvironment::new(checkpoint_backend, launch_host)` supplies the Git queries
and branch rename behavior. `LaunchHost` injects config/workspace folder policy,
provider enablement, naming and terminal setup (`t3-config`, `t3-workspace`,
`t3-terminal` remain host dependencies). Use `ThreadLaunchService::with_preparation`
with a `LaunchWorkspacePreparation` and `LaunchProgress` to stream fetch, checkout,
submodule, setup and agent stages. Preparation must call `observer.claimed(workspace)`
immediately after claiming a new checkout path; failures then remove that path.
The legacy constructor delegates to `WorkspaceBackend::prepare`, whose host must
clean its own partial failures.

Subscribe to the host's VCS branch stream before admitting commands and feed its
`BranchChange` receiver to `CheckpointSubscriptions::start(sync, receiver, launches)`.
On shutdown: stop external command and effect admission, shut down these subscriptions
(joins foreground launches, cancels preparation and cleans abandoned worktrees, then
stops branch input and awaits in-flight reconciliation), stop provider execution,
drain finalization/checkpoint captures, then drain event commits before closing SQLite.
Do not close the command dispatcher or event sink before launch cleanup and branch
reconciliation have completed. The host owns broadcaster-to-channel adaptation and
must resnapshot on lag rather than silently drop branch changes.

## Claude adapter composition (package 11)

Enable `claude-adapter` and call `adapters::claude::claude_registration(options, continuation_key, enabled, credential_access)`. Add the returned `AdapterRegistration` to `runtime::sessions::AdapterRegistry::new`, then inject that registry as `RuntimePorts.providers`, exactly as with Replay. A dynamic host can return the registration from `ProviderInstanceSource`. Use driver `claudeAgent` in the configured instance settings, and the same instance ID in `options.instance_id` and model selections. The continuation key identifies the configured provider/credential context; the host supplies its enabled state and shared credential-access gates. Construction and registration do not spawn a process; starting native work does. The registry remains authoritative for admission.

Inject the configured instance id and `LaunchOptions` (resolved executable and complete environment), `ProcessConnector` for production or a pipe connector for replay, shared `Clock`/`IdGenerator`, and a fresh native-session UUID generator. `options.mcp_servers` accepts stateless SDK servers shared across sessions. For thread-bound T3 tools, inject `ClaudeHostOptions::mcp_server_factory`: its `ClaudeMcpServerFactory::create(&OpenSession, CancellationToken)` returns the named SDK servers for that app thread/provider session. The adapter calls it once per opened session, registers its servers before native initialization, and reuses them across native query replacements. Bind the host's thread context and permissions there and stop any owned host work when its token is cancelled; closing one session does not cancel another's bindings. Static and factory names must be distinct. Supply `ClaudeAttachments` for saved attachment paths and document text sidecars (`DirectoryClaudeAttachments` handles the standard attachment directory). `transcript_root` is the explicitly configured Claude projects directory used for native transcript operations. The adapter never discovers account directories implicitly.

`ClaudeHostOptions` supplies the host home/settings directories, attachment directory for native access, current auto-compact override, and authoritative runtime/MCP/device instructions. Inject `compile_model` from the provider model catalog to preserve catalog defaults, aliases, effort maps, and model suffixes; the same compiler handles launch and live effort/fast-mode updates; the direct-model fallback only applies explicit native options. `managed_settings` must include the native OS/server effective policy snapshots for this instance, including restrictions resolved outside ordinary settings files. The adapter combines these restrictions monotonically with custom-home, project/local and launch-file restrictions; launch settings cannot re-enable a disabled mode. No JavaScript SDK sidecar is used.

Name the T3 MCP server `t3-code`. The adapter adds the exact upstream `send_to_user` guidance and the annotated read-only allowlist. Its host server must expose `send_to_user` as read-only with `anthropic/alwaysLoad: true`, preserving the exact Markdown in `sentMessage`; theme reads are allowed, while theme writes remain governed by the MCP host's access checks.

For native background wakes, inject `ClaudeHostOptions::continuation_requests` with the sender consumed by the shared `ProviderContinuationService`. The adapter buffers the native wake until the allocated provider-created run calls `start_turn`; that call consumes native frames without sending another prompt. Keep the same session alive while its pending-background predicate is true. Continuation callbacks retire cancelled, unstarted wakes and guard against stale delivery. Preserve Monitor `updated` notifications through continuation admission so progress does not become a delegated parent follow-up.

Known Claude differences:

- Outside a managed ceiling, explicitly accepting a native `setMode` suggestion for the session can raise Claude's permission mode above the thread's displayed mode. This behavior is inherited from TypeScript (review L4); the thread mode is not changed by that native session grant. With `max_runtime_mode` present, every `setMode` suggestion is filtered.
- Empty or entirely ceiling-filtered session suggestions grant only the current call. TypeScript creates a whole-tool rule when no suggestions remain; this stricter behavior is intentional. Other native suggestion objects retain every field, with destination set to session.

## Package 09 transport — server contract (2026-11-13)

The composed shell and thread services are implemented. Every subscription below owns its cancellation: drop cancels work; shell/command `close().await` also joins the worker. Pull shared live item values through the RPC encoder without cloning their whole payloads.

| Endpoint | Single service call |
| --- | --- |
| `orchestration.subscribeShell` | `runtime.transport.subscribe_shell(input, LiveStreamLimits::default()).await?` |
| `orchestration.subscribeThread` | `runtime.transport.subscribe_thread(input, limits).await?` |
| `orchestration.subscribeCommandOutput` | `runtime.command_output.as_ref().ok_or(/* unavailable backend */)?.subscribe(input).await?` |
| `orchestration.subscribePromptSuggestion` | `runtime.sessions.subscribe_prompt_suggestion(input.thread_id).await?`; send the watch's current value, then changes |
| `orchestration.searchThread` | `runtime.transport.search_thread(input).await?` |
| `orchestration.searchThreadStream` | Requested store accessor: `runtime.projections.search_thread_stream(input).await?`; forwards its existing cached `ThreadFind::search_thread_stream`, never creates another cache |
| `orchestration.searchThreads` (client) | `runtime.thread_search.as_ref().ok_or(/* unavailable backend */)?.search_with_older_settled_rows(input).await?` |
| Agent title/message search | `ThreadSearch::search_threads(input, project_id).await?` |
| HTTP thread transcript | `OrchestrationHttpRead { service: &runtime.transport }.thread_transcript(scopes, &runtime.threads, thread_id, environment_id, max_bytes).await?` |

HTTP transcript export uses the existing management service, including scope enforcement before reads, full persisted history, the supplied environment identity, a 400 response for over-budget exports and 404 for missing threads. The export preserves tool output rather than applying wire previews. The server owner still needs to connect its HTTP route to this call.

The ANSI terminal fold, rendered find text and search policy come from `t3-shared` (`terminal_output`, `find_text`, `search_policy`), shared with the client. `ports/transport/shared_helpers.rs` implements this crate's three ports for them, and `RuntimePorts::new` installs them, so `runtime.command_output` and `runtime.thread_search` exist for every host. The hub retains 64 live and 16 finished tails, bounds subscriber appends at 16 Ki units, throttles append/replacement frames, observes committed item snapshots, normalizes legacy Bash results, and fails on missing/non-command rows or event loss. Provider ingestion must deliver ephemeral chunks to this exact hub's `append(target, chunk)` and call `finish(target)` before settled item publication. Its current `CommandOutput` branch still discards chunks; that hookup belongs to 06/composition.

`ThreadFind` retains the snapshot/revision-bound scan, progressive early/complete frames, relative selection/wrapping, folded question answers, bounded navigation, 8,192/32 parsed-text caches, eight completed scans and one-minute idle expiry. The store supplies its real visible-index/candidate payload snapshot in one transaction. `ThreadSearch` uses bounded real SQLite FTS/scan reads, excludes placeholders/notices/streams/hidden threads and async answers, counts the best-quality hits, scopes projects and returns older settled shell rows coherently. `message_search_index.rs` supplies create/recreate/rebuild and trigger predicates, including legacy databases without `search_text`.

Source mapping and checks:

| TypeScript | Rust | Focused test coverage |
| --- | --- | --- |
| `orchestration-v2/ShellStream.ts`, shell part of `ws.ts` | `shell_stream.rs`, `transport/service.rs` | 14 baseline shell tests |
| `orchestration-v2/CommandOutputHub.ts`; `shared/terminalOutput.ts` | `command_output_hub.rs`, `t3-shared` `terminal_output` | 7 tests including one 1,204-case executed-TS oracle; all five current external hub tests pass |
| `orchestration-v2/ThreadFind.ts`; shared find/count/context/directive helpers | `thread_find.rs`, `t3-shared` `find_text` | 5 baseline service/cache tests; 2 new native tests covering 888 rendered cases, 480 embed cases and 80 occurrence cases |
| `orchestration-v2/ThreadSearch.ts`; `shared/paletteSearch.ts`, PR helpers | `thread_search.rs`, `t3-shared` `search_policy` | 3 baseline SQL/snippet tests; 3 native tests covering 196 message, 14 title and 8 message-ranking cases |
| `orchestration-v2/messageSearchIndex.ts` | `message_search_index.rs` | Real migrated SQLite FTS and scan coverage in the search tests |
| `orchestration-v2/http.ts` | `http.rs` | 4 tests, including authorized full export, denied scope, missing thread and size bound |

The latest isolated native-source run passes **6 tests / 2,870 TypeScript-computed cases**, and scoped Clippy with `-D warnings` passes. It imports the actual helper/fold source against actual contracts and copies only seam signatures/accessors into an external scaffold; it is not a production composition test. The current standard crate passes all **82 package-09 transport tests** (77 integration tests and five library tests), including the expanded hub/HTTP/Find assertions. The staged helper tests still require the manifest changes before they can join that production module tree. The latest normal hub test build succeeds; the earlier visit-event exhaustiveness, absent Grok replay fixture and ingestion borrow errors no longer block it. Current all-target Clippy stops outside package 09 at `pull_request_sync_reactor.rs:605` (`too_many_arguments`) and `thread_settlement_service.rs:843` (`unnecessary_fold`). Crate format-check reports only unowned files: `checkpoint_diff_query.rs`, `pull_request_sync_reactor.rs`, `thread_settlement_service.rs`, `runtime/threads.rs` and `thread_settlement_service_test.rs`. No other owner's source was changed.

Known differences and remaining integration limits:

- Project enrichment is not connected: snapshots use stored project shells and emit no asynchronous identity-resolution refresh. Digest/root-selection helpers match executed TypeScript values.
- The handler maps crate errors into RPC envelopes; original event-feed causes are not retained by the hub. Full TypeScript error-cause parity needs that bridge.
- Find checks its completed-scan cache after reading snapshot candidates; TypeScript checks a cheaper revision query first. Composition now retains the single cache across calls.
- Native Markdown uses CommonMark events and a lightweight raw-HTML traversal rather than the TypeScript HTML5 tree-repair engine. The recorded visibility/directive/context cases match; arbitrary malformed-HTML repair, all editor-link schemes and all custom Markdown plugin syntax still need full source-suite conformance before claiming renderer-wide parity.
- Rust public strings replace unpaired UTF-16 surrogates with U+FFFD; JavaScript can retain them when cursor edits split a surrogate pair. Internal terminal runs retain original units so later edits can repair the pair.
- Older title candidates read full thread JSON rather than reduced SQL fields. Candidate ordering is explicitly rowid-based, replacing unspecified SQL order.
- Numeric cursors contain no environment identity. Ahead/pruned/unsafe cursors reset; an in-range cursor from another environment is indistinguishable under the existing TypeScript contract too.
- `ShellSnapshotAnswer` remains a local untagged type pending its requested move to `t3-contracts::orchestration_v2::OrchestrationV2ShellSnapshotAnswer`.
- Contracts now expose `ThreadVisitRecorded`; package 09 only extracts its thread ID for the generic reference path. Read-state fan-out/awareness drift is not ported before the requested upstream merge.
- Inherited from TypeScript, kept on purpose (to raise upstream): a pull-request watch wake carries forge text (comment snippets, check names, merge-queue removal reasons) followed by "Look into each item and act on it as your task requires", with nothing marking that text as written by other people. Check names and the removal reason are not bounded like comment snippets (`ports/thread_project/pull_request_watch.rs:410, 481-575`; TS `pullRequestWatch.ts:256-345`). Review `t3-orchestration-ceiling-rereview.md` W1.
- Inherited from TypeScript, kept on purpose (to raise upstream): a watch whose wake is refused for a reason that persists (the thread is above the managed ceiling, its provider instance is disabled) never commits its advance, so every two-minute sweep re-reads the forge for it, without end and without a visible notice (`pull_request_watch_reactor.rs:1049-1053`, `:187-194`; TS `PullRequestWatchReactor.ts:551-559`). Review W2.

The investigated image-preview bound is a TypeScript test bug: executing the source with 10,000 image blocks yields **314,913 UTF-16 units / 314,915 UTF-8 bytes**, including eight retained image markers. Its 262,144-byte raw preview expands under JSON escaping, so `<270,000` is incorrect. `wire_projection_test.rs` pins the executed source result; Rust preserves that behavior.
