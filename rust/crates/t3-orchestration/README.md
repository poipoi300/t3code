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
- `options.mcp_apps: CodexMcpAppHost` for validated/CSP-filtered stored snapshot leases, and `options.generated_image_markdown` for the shared Markdown image formatter.

These optional host traits and their inputs live in `adapters::codex`; omitted integrations remain
unavailable. The adapter owns native protocol translation, process supervision/stderr draining,
initialization and awaited cleanup. A session serves multiple workspaces: process cwd is cleared,
and cwd/app-thread identity travel on each thread RPC. `CodexAdapterV2::open_connected` accepts
an externally owned native transport; the recorded `CodexReplayAdapter` uses it without Codex turns.

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
The composition's `ProviderSessionManager::with_options` must pass
`Some(ingestor.clone())`: `None` leaves idle requests/recovered commands without
a consumer before run execution subscribes. This wiring is still required in
the integrator-owned `src/composition.rs`; no second event pump is needed.

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
dispatcher, committer, database, settings, hydrator, signals, clock })`. Clone this
service to share its export admission limit and shutdown scope. Construct
`ThreadStopService::new(ThreadStopServices { management, sessions, agent_scope,
gate })` with the dispatcher's **same** `Arc<KeyedLanes>` as `gate`; alternatively,
`with_dispatcher_gate(management, sessions, agent_scope)` obtains it from the
management dispatcher's `command_lanes()`. The stop service exposes
`stop_workflow`, `force_stop_thread`, and `await_background_tasks` to handlers.

Use `ThreadTitleRegenerationService::new(management, database, settings, generator)`
instead of a struct literal. Execute titles with the effect's original command ID
and the runtime cancellation token. Construct settlement with
`ThreadSettlementService::new(ThreadSettlementServices { projections, dispatcher,
committer, settings, clock, ids })`, wrap it in `Arc`, then call `start().await`.
Its current sweeps cover inactivity and persisted links; branch lookup, merge
feeds and settle cleanup remain blocked on the backend ports listed in the
package-08 takeover report.

Construct `PullRequestWatchReactor::new(PullRequestWatchServices { projections,
commands, pull_requests, clock, ids })`, wrap it in `Arc`, and call `start().await`
for immediate and two-minute sweeps. `sweep()` is also directly awaitable. Inject
the same dispatcher and projection store used by the runtime. Its current host
port supports grouped detail/activity/fingerprint reads and paginated comments;
typed host failure policies and host change/invalidation feeds still require
seam additions. PR sync and branch discovery are not yet available to compose.

Shutdown order: close external admission; await watch and settlement `shutdown()`
to stop their producers; stop and join the effect worker; await title `shutdown()`;
await stop `shutdown()`; await management `shutdown()`; then shut down provider
sessions and drain the event sink before closing SQLite. Each service shutdown is
idempotent. Management shutdown closes transcript admission and cancels/awaits
active exports, waits, settlement requests and dispatches. Stop shutdown first
cancels/awaits foreground calls, then cancels/joins its owned release/detach tasks.
Title shutdown cancels its backend token and awaits active generations. An
accepted SQLite commit can outlive its canceled caller; the sink must still drain.
Settlement shutdown also cancels/joins direct sweeps, stops subscriptions, then
joins both coalescing workers. Project and deletion planners are pure functions;
resource cleanup has an injected constructor and owns no background tasks.

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
| `policy` | `BaseRuntimePolicy` | Turns run with the thread's own runtime and interaction modes; no project override, managed ceiling or reasoning effort (04 owns the real resolver) |
| `attachments` | Unsupported | Messages and launches carrying attachments fail before dispatch; messages without attachments work |
| `cleanup` | Unsupported | Terminal, preview, scheduled-task and attachment cleanup effects fail and retry; thread deletion leaves those resources |
| `workspaces` | Unsupported | Worktree launches fail; settlement branch and PR confirmation is unavailable |
| `agent_scope` | `None` | Force-stop cannot kill thread-owned agent processes; OOM detection is off |
| `telemetry` | `None` | No usage refresh or completed-turn analytics |
| `titles` | `None` | Title-generation effects fail and retry; threads keep their given title |
| `message_context` | `None` | `message.dispatch` is decided without composer context (04's `MessageContextPreparer`) |
| `thread_find_text` | `None` | In-thread find (`ProjectionReader::search`) fails Unsupported; one store, and one find cache, serves the runtime when given |
| `terminal_output` | `None` | `runtime.command_output` is absent: no command-output subscription |
| `search_policy` | `None` | `runtime.thread_search` is absent: no cross-thread search |
| `launch` | `None` | `launch_thread` fails Unsupported |
| `pull_request_identities` | `None` | Projected PR links keep their persisted identity; no host alias or authority normalisation |
| `pull_requests` | `None` | No PR watch reactor runs |
| `rollback_workspaces` | `None` | Provider-thread rollback effects fail Unsupported |
| `finalization_observer` | Unsupported | After each run, VCS status refresh is skipped (the failure is not surfaced) |
| `sessions` | `SessionManagerOptions::default()` | TS timeouts (30 min idle, 15 s interrupt) |
| `worker` | `WorkerOptions::default()` | 4 slots, 5 attempts, 30 s liveness poll |

### Start-up and shutdown order

Start (TS order): legacy V1 shells (`legacy.reconcile_shells`), provider recovery
(`RuntimeLayer::start`), effect worker daemon, then the settlement and PR watch producers.
Commands are admitted once recovery succeeds. The host then drives
`runtime.legacy.import_pending_transcripts()` and its `legacyThreadMigration` lifecycle
events. Delegated-task recovery is not composed yet.

Shutdown: PR watch and settlement producers; launch, title, stop and thread management
services; the runtime layer (closes admission, prepares providers, stops the effect worker and
provider sessions, reconciles); then run execution, the event ingestor and pending commits.
Package 08's documented order joins the worker before title, stop and management. The runtime
layer stops the worker and sessions together, so those three close first and in-flight effects
retry after restart.

### Which handle serves each RPC

| RPC or route | Call |
| --- | --- |
| `orchestration.dispatchCommand` | `runtime.dispatch_command(command)` |
| `orchestration.launchThread` | `runtime.launch_thread(input)` (needs `launch`) |
| `orchestration.stopWorkflow` | `runtime.stop.stop_workflow(input, None)` |
| `orchestration.forceStopThread` | `runtime.stop.force_stop_thread(input)` |
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
| `getTurnDiff`, `getFullThreadDiff` | not available yet (no diff query service) |

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

`tests/runtime.rs` creates a project and thread, sends a message through `dispatch_command`,
watches the run reach `completed` through `transport.subscribe_thread`, then restarts on the same
database and finds nothing to recover. With `git-backend` the same turn runs in an isolated Git
repository on 07's `GitCheckpointBackend` and asserts the captured checkpoint ref resolves to a
commit. Without `git-backend` the workspace is a plain folder and capture is skipped. The provider
is a scripted adapter behind 05's registry. The Replay case (`acp-adapter`) is ignored until 12's
ACP adapter accepts the not-loaded provider thread the first-message decision creates: it fails
`ensure_thread` and the start effect then retries indefinitely.

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
| `testing::Script<I,O>::push(Result<O>)`, `next_call().await -> Option<I>`; `Fake*` implementations for every new service, policy methods on existing fake adapter/runtime, `FakeEventCommitter::new(StoreDatabase)` | Integrator requirement | Implemented in `testing.rs`. Unscripted calls fail with typed Unsupported; prepared fake commits run callbacks on real migrated SQLite and roll back on callback/script errors |

Transaction implementation contract for 03: perform receipt dedupe and guards before invoking preparation; run normalization, callback mutations, marker/position writes, events, projections, receipt and outbox on the **same** writer transaction. Preparation must not replace the receipt identity or weaken the supplied guard; validate any newly prepared guard before append. A guard rejection after callback mutation must roll those mutations back. Publish and signal only after durable commit, retaining the publication lane when the caller drops. The callback is synchronous and must not perform external I/O. The fake demonstrates rollback, not production dedupe, guard enforcement, event append or publication.

Session implementation contract for 05: inspect/remove under the same busy-turn gate used to admit starts. Evaluate `keep_if` while retaining that admission gate; no SQLite connection guard may cross its await. Shared-thread inspection alone never authorizes a subsequent unconditional release. Keep the thread-specific credential revocation scope and exact reason/detail.

Merged/declined requests: 03 and 05 share one transaction callback; 06 commit flags reuse `WriteOptions`; 05 metadata and 07 rollback requests reuse existing types. The requested direct `RuntimePolicy.reasoning_effort` field and new `DecisionContext` fields use wrappers because current leaf literals lack struct-update syntax. The requested `ControlQuery::ProviderTarget` variant uses a companion method to avoid breaking exhaustive matches. Repository-error correlation (01) remains outside this crate: auth/pairing/provider-runtime repository APIs must be supplied/reassigned by their owners; no duplicate repositories or auth dependency was added. Shared command-lane injection (06) requires 04's leaf constructor change; no integrator edit to `runtime/dispatch.rs`. Workflow file-identity work remains 06's future consumer of the existing `t3-workspace` capability; unused `file-id`/`same-file` dependencies were not added here.

Second batch (later on 2026-11-13), packages 02, 07 and 09. Answered by existing seams rather than new ones: 02's provider-control targeting is `provider_target(ProviderTargetQuery)` (06 asked for the same capability); 02's runtime-response thread scoping is `RuntimeRequestResponder::respond(ThreadId, ...)`; 02's missing record filters are `query_records(ProjectionRecordQuery)`, and any filter still missing must be named field by field. 07's shared dispatcher lane for final rollback commits is `CommandDispatcher::command_lanes()`: `CheckpointRollbackService` already takes `Arc<KeyedLanes>`, so composition passes the dispatcher's Arc. 07's reservation and lease helper adoption is internal to 07. Not done here: moving 09's `ShellSnapshotAnswer` (`ports/transport/http.rs`) into `t3-contracts` belongs to the contracts owner.

Third batch (2026-11-13 afternoon): not yet applied. 04's provider-switch / context-handoff plans need an exact type; 04's canonical pull-request identity context needs the host source-control registry, so it belongs with the host ports, not `DecisionInputs`. 06's `ProviderEventNormalizeError` / `ProviderEventPublishError` variants wait until 06 defines those structs. 02's exact signatures (thread-scoped runtime response, checkpoint metadata, two `RecordFilter` filters, registry adapter for the write path) wait for its new heading in `orchestration-seam-requests.md`; for checkpoint metadata, the 2026-11-13 `CheckpointBackend::{checkpoint_ref, is_git_repository, delete_checkpoint_refs}` may already be the answer.

Fourth batch (16:20): 03's pending-delegation repair has no implementation to forward to; TS `Orchestrator.recoverDelegatedTask` offers delegated-completion deliveries and finalizes the app-owned subagent under the thread lock, which is 04's completion-delivery work. The composition forwards `recover_delegated_task` once 04 exposes it (owner 04). 08's source-control registry port and 05's usage auto-arm (`LimitRecoveryArmer`, needs a guarded dispatcher method from 04) and continuation worker are not composed yet.

Fifth batch (17:10): not applied. 02's `CheckpointRollbackContext::target_attempt` element-type change is not additive (02 builds the context; 07 and 10 consume the full attempt) and needs those three owners to agree on one change. 02's reader snapshot token needs a stable reader identity from `t3-persistence` (its owner). Native adapter factories (Cursor, OpenCode, OpenCode2, Pi, Muse, Grok, Kiro, fx) belong to the provider packages outside this crate; the Codex host bridges, ACP flavor hook, diff formatter, runtime-instruction builder, command-output fold and pending-work helper belong to the host or shared owners. Routing is in `orchestration-seam-requests.md`.



## Package 07 host integration

Construct `WorkspaceCoordination::global()` once and pass it to
`CoordinatedTurnStart::new(turn_start, projections, database, coordination.clone())`.
Use that wrapper for every provider start (including retries); it reserves the project
root or worktree through startup and returns the execution handle before settlement.
Call `start_configured` with the full resolver result to preserve reasoning effort.
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

Inject the configured instance id and `LaunchOptions` (resolved executable and complete environment), `ProcessConnector` for production or a pipe connector for replay, shared `Clock`/`IdGenerator`, a fresh native-session UUID generator, and the per-session T3 MCP servers. Supply `ClaudeAttachments` for saved attachment paths and document text sidecars (`DirectoryClaudeAttachments` handles the standard attachment directory). `transcript_root` is the explicitly configured Claude projects directory used for native transcript operations. The adapter never discovers account directories implicitly.

`ClaudeHostOptions` supplies the host home/settings directories, attachment directory for native access, current auto-compact override, and authoritative runtime/MCP/device instructions. Inject `compile_model` from the provider model catalog to preserve catalog defaults, aliases, effort maps, and model suffixes; the same compiler handles launch and live effort/fast-mode updates; the direct-model fallback only applies explicit native options. `managed_settings` must include the native OS/server effective policy snapshots for this instance, including restrictions resolved outside ordinary settings files. The adapter combines these restrictions monotonically with custom-home, project/local and launch-file restrictions; launch settings cannot re-enable a disabled mode. No JavaScript SDK sidecar is used.

For native background wakes, inject `ClaudeHostOptions::continuation_requests` with the sender consumed by the shared `ProviderContinuationService`. The adapter buffers the native wake until the allocated provider-created run calls `start_turn`; that call consumes native frames without sending another prompt. Keep the same session alive while its pending-background predicate is true. Continuation callbacks retire cancelled, unstarted wakes and guard against stale delivery.

## Package 09 transport continuation — server contract (2026-11-13)

The shell RPC is ready to connect to the server handler:

```rust
let mut subscription = runtime.transport.subscribe_shell(input, LiveStreamLimits::default()).await?;
while let Some(batch) = subscription.pull().await? {
    // Send each SharedLiveItem.value through the existing RPC encoder.
}
subscription.close().await?;
```

`subscribe_shell(OrchestrationV2SubscribeShellInput, LiveStreamLimits) -> Result<ShellSubscription>` subscribes before any database await. Its authoritative snapshot precedes `synchronized`; resume deltas precede that marker and live deltas follow it. Invalid/ahead/pruned/over-budget resume cursors return a snapshot, or one terminal `snapshot-required` frame with `httpSnapshotFallback`. Broadcast loss also sends a terminal `snapshot-required`; buffer overflow fails the stream. Drop cancels both workers; `close` additionally joins them. Lightweight references group for 50 ms / 512 events, shell reads batch, queued full-state deltas replace by identity, and delivered states remain charged. Subscriber snapshots use the single-transaction indexed projection seam and preserve idle-subagent/older-settled exceptions and counts. HTTP snapshots use the same loader.

| Endpoint | Single service call |
| --- | --- |
| `orchestration.subscribeShell` | `TransportService::subscribe_shell(input, limits).await?` |
| `orchestration.subscribeCommandOutput` | `CommandOutputHub::subscribe(input).await?`; frames from async `pull`, cancel on drop, awaited `close` |
| `orchestration.subscribePromptSuggestion` | Existing `SessionManager::subscribe_prompt_suggestion(input.thread_id).await?`; send the watch's current value, then changes, drop on cancellation |
| `orchestration.searchThread` | `ThreadFind::search_thread(input).await?` |
| `orchestration.searchThreadStream` | `ThreadFind::search_thread_stream(input).await?`; async `pull` yields the early match then the complete snapshot; drop releases the scan |
| `orchestration.searchThreads` (client) | `ThreadSearch::search_with_older_settled_rows(input).await?` |
| Agent title/message search | `ThreadSearch::search_threads(input, project_id).await?` |
| HTTP thread transcript | `OrchestrationHttpRead::thread_transcript(scopes, management, thread_id, environment_id, max_bytes).await?` |

Construct one `CommandOutputHub::new(projections, events, terminal_fold)` and route ephemeral provider output to its `append(target, chunk)` / `finish(target)`. It retains 64 live and 16 finished tails, accumulates at most 16 Ki UTF-16 units per subscriber, throttles appends at 100 ms and overflow replacements at 500 ms, reconciles persisted settlement/legacy Bash output, and errors on missing/non-command rows or event loss. The mandatory marked `TerminalOutputFold` host shim represents `packages/shared/src/terminalOutput.ts`; ANSI/cursor folding is **not implemented here**. The hub is not production-complete until the shared owner supplies that fold and ingestion connects it. Plain-text test doubles test the hub only; they do not establish ANSI parity.

`ThreadFind::new(snapshot_reader, rendered_text)` requires the marked `ThreadFindSnapshotReader` and `ThreadFindText` host shims. Package 02 now implements the visible-index/candidate-payload read on ProjectionStore in one transaction, including fork sources/revisions and empty-reply tool exceptions. The text helper must supply canonical rendered Markdown/context/skill segments, occurrence counting and HTML/MCP result detection. Selection, relative wrapping, bounded navigation, question-answer folding and progressive frames are ported. `ProjectionReader::search` and `TransportService::search_thread` now work when the store is constructed with `ProjectionStore::with_thread_find_text(text)`; without that required backend they fail Unsupported. Supply the same text backend through composition (integrator-owned). Find now has 8192/32 parsed-text caches, eight completed scans and a one-minute idle expiry with active scan leases.

`ThreadSearch::new(database, policy)` has the real SQLite bounded FTS/scan query, placeholder/notice/stream/subagent/side-chat/async-answer exclusions, message selection/hit counts, project scoping, older-settled selection and coherent ranked shell reads. `ThreadSearchPolicy` is a marked shim for the authoritative shared palette tokenizer/matcher/ranking/displayed-link helpers; no approximate policy is installed. `message_search_index.rs` ports the trigger predicates, readable-text hook, create/recreate and transactional rebuild, including old databases without `search_text`. Citation conversion is injected into `readable_search_text`; it is not duplicated. Full source search suites await these shared implementations.

Remaining differences/integration work:

- No project-enrichment host seam exists. Shell reads expose stored project shells; asynchronous identity-resolution refreshes and `knownProjectIdentities` suppression are not connected. SHA-256 identity digests, absent-identity root selection and HTTP digests match TypeScript golden values.
- New subscriptions return the crate's typed `Error`; the server handler owns mapping it to each canonical RPC error envelope. The hub preserves error messages; event-feed loss currently retains no original cause, so full TypeScript cause parity needs an error-envelope bridge.
- Find still reads snapshot candidates before checking its completed-scan cache; TypeScript checks a cheaper revision query first. Keep a ThreadFind service alive across calls to retain caches; the current store forwarding constructs one per call.
- Older title candidates currently read raw thread JSON rather than TypeScript's reduced SQL fields. Title/ranking validation belongs to the supplied policy. Candidate order is explicitly rowid order instead of relying on unspecified SQL ordering.
- Numeric cursors carry no environment identity in the existing wire contract. Ahead, pruned and unsafe cursors reset correctly; an in-range cursor copied from another environment cannot be distinguished by either implementation without a contract change.
- `ShellSnapshotAnswer` remains local pending the contracts owner's move. No manifest/module-root/seam edits were made.

TypeScript source mapping for this continuation: `ShellStream.ts` plus the shell portion of `ws.ts` -> `src/ports/transport/shell_stream.rs`; `CommandOutputHub.ts` -> `command_output_hub.rs`; `ThreadFind.ts` -> `thread_find.rs`; `ThreadSearch.ts` -> `thread_search.rs`; `messageSearchIndex.ts` -> `message_search_index.rs`; `http.ts` -> `http.rs`. `WireProjection.test.ts` -> `tests/ports/wire_projection_test.rs` now pins the independently executed TypeScript result: **314,913 UTF-16 units / 314,915 UTF-8 bytes** for 10,000 images, including eight retained image markers. The raw preview is 262,144 bytes before JSON escaping, so TypeScript's `<270,000` assertion is incorrect. Rust preserves the source behavior.

Verification: **81 transport tests pass** (76 in the isolated package-09 harness plus five transport lib tests), including real migrated SQLite and the real package-02 Find snapshot adapter. This continuation adds 22 tests. Shell has 14 tests (12 external + 2 lib), command output 6 (4 + 2), Find 5 (4 + 1), HTTP 4, and cross-thread search/index 3; existing wire/history/thread tests also pass. The integrator has declared the previously missing hub and Find test modules. Full crate verification remains red outside transport: the ports run had 746 passed, two stop-related failures and one ignored; Clippy reports five collapsible_if findings in decision/engine.rs; format-check reports unowned test files only. This remains partial TypeScript source-suite coverage: remaining rendering, ANSI, ranking and enrichment assertions require the named shared host bridges. Exact signatures, owners and failure names are recorded under the dated package-09 headings in the external seam-request brief.