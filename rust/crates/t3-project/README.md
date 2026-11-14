# t3-project

Native transport-neutral services for `apps/server/src/project`. Audit performed
before implementation against all four sibling crates, their READMEs, the
server RPC/HTTP ledger, per-filename searches of orchestration-plan.md, and
reports/orchestration-08-takeover.txt. Package 08 ports orchestration-v2 files,
not the similarly named project-directory services.

## Current upstream round (817a9186f9)

Specification was re-read after the head-91 upstream merge. The server now uses
this service for ensureScratch/mutate/createNew. Remaining server construction
and exact RPC calls are recorded in the t3-project next-round section of
C:\Users\Administrator\t3code-rust-notes\briefs\server-seam-requests.md.
ForgeCloneRepository supplies the real Git backend; the server forge adapter now
exposes its registry for injection. Promotion's scheduler adapter also exists.

Review M1/M2/L5 are fixed with observed red-to-green tests: create and changed-root
update refuse metadata/worktree containers before commit; automatic bootstrap has
a refusal-only skip API; imported linked worktrees retain branch/path/runtime cwd;
failed settings cleanup logs the project ID without undoing deletion. The new
t3-git container probe and real-repository test matrix were added under explicit
coordinator authorization. Server composition must call bootstrap_automatic and
share one ProjectService and environment Git handle (see the seam file).
The native session inventory (`inventory.rs`) and the import store over the
runtime table (`import_store.rs`) now exist; the server serves agentSessions.*.

The concrete clone test now uses a temp file:// remote, asserts progress and
checks partial-repository cleanup, then verifies foreign contents are preserved.
Session fixtures now live inside an explicit temp .codex/sessions tree. No real
provider home is read. scratch_folder_root is exported for cleanup/recovery;
the existing scratch-root behavior is unchanged. Current build output is
D:\t3code-rust-target\project with two configured jobs and the configured
rustc-gate wrapper. No wrapper bypass or memory-allocation retry was needed.
The review regressions and previous dependency-blocked tests now execute.

## Coverage audit (initial state)

Every path below is relative to `apps/server/src/project/`. Status refers to
behavioral completeness, not the mere presence of a similarly named type.

| TypeScript file | Existing Rust home before this task | Complete? |
| --- | --- | --- |
| ProjectService.ts | t3-orchestration stores::projects and ports::thread_project::project_commands provide persistence/planning only | No service |
| ProjectMutation.ts | None | No |
| ManagedProjectFolders.ts | None; launch environment has an injected folder_for_thread seam | No |
| ProjectCloneTracker.ts | None; ledger's proposed package 08 API does not exist | No |
| gitCloneProgress.ts | None (t3-git progress handles commit output) | No |
| ProjectEnrichmentService.ts | None; WorkspaceBackend::repository_identity is injected | No |
| ProjectFaviconResolver.ts | t3-workspace::project::FaviconResolver | Partial; documented sequential probes and bounded reads |
| ProjectSetupScriptRunner.ts | t3-workspace::scripts::ScriptRunner; project script helpers | Partial; settings/projection composition external, observer differences in its README |
| WorktreeSetupTracker.ts | None | No |
| RepositoryIdentityResolver.ts | None; t3-git supplies Git execution and remote_repository_key only | No |
| T3ProjectFileLoader.ts | t3-workspace::project::{load_project_file,parse_project_file} | Partial; bounds/JSONC delegated to existing implementation |
| ScratchThreadPromotion.ts | None; adopt-folder command belongs to orchestration | No |
| AgentSessionScanner.ts | None | No |
| AgentSessionJson.ts | None | No |
| AgentSessionImporter.ts | None; legacy_v1_thread_importer handles a different format | No |
| http.ts | None; ports::transport::http handles thread routes, not project routes | No; service targets snapshot/mutate |
| threadWorkspaceRoot.ts | Equivalent workspace lookup inside thread launch/title services, no exported helper | Partial |
| worktreeContainer.ts (upstream addition) | No home at initial audit | Additive Git module now recorded in current coverage below |

Tests alongside these TypeScript files are the specification. Existing sibling
implementations are reused; this review round explicitly permits the additive
t3-git container module, declaration and tests.

## Current coverage and handler API

This is an incomplete port: native session inventory/runtime adapters remain
blocked on sibling public APIs. Managed filesystem operations now have the
server's native HostFolders backend. All backend seams are
required constructor inputs; no empty-success default exists.

The forge owner added clone services/progress during this task. The final code
uses those APIs; the initially necessary local progress parser was removed.

| TypeScript file | Current Rust home | Complete? |
| --- | --- | --- |
| ProjectService.ts | t3-project::service; orchestration project stores/planners/EventSink | Core service implemented; metadata differences below |
| ProjectMutation.ts | t3-project::service::mutate | Implemented, shielded delete/clone discard |
| ManagedProjectFolders.ts | t3-project::managed; t3-server::adapters::project::HostFolders | Service and native backend implemented; cleanup refusal tests include an outside Windows junction |
| ProjectCloneTracker.ts | t3-project::clones | Tracker + concrete ForgeCloneRepository adapter implemented; full parity unverified |
| gitCloneProgress.ts | t3-forge::clone_progress; t3-project forwarding adapter | Existing sibling port reused |
| ProjectEnrichmentService.ts | t3-project::enrichment | Bounded lanes/caches, options, remembered identities, single-flight and redacted publication; tests pass |
| ProjectFaviconResolver.ts | t3-workspace::project::FaviconResolver | Partial sibling port reused; differences below |
| ProjectSetupScriptRunner.ts | t3-workspace::scripts/project; t3-project::scripts | Execution reused; settings/projection/trust composition added |
| WorktreeSetupTracker.ts | t3-project::setup; t3-git::setup_tracker | Legacy tracker implemented with redacted output; server must use one authoritative tracker; Git owner's redaction request recorded |
| RepositoryIdentityResolver.ts | t3-project::identity; t3-git keys/execution; t3-forge remote recognition | Cwd/root caches, coalescing, optional refiner and credential redaction verified; host normalization gaps remain |
| T3ProjectFileLoader.ts | t3-workspace::project::{load_project_file,parse_project_file}, t3-contracts | Partial sibling port reused; bounds/parser differences below |
| ScratchThreadPromotion.ts | t3-project::promotion; t3-scheduler task-movement adapter | Service with joined shutdown implemented; real SQLite lifecycle regression passes |
| AgentSessionScanner.ts | t3-project::sessions::AgentSessionScanner, transcript and inventory::NativeSessionInventory | Discovery, budgets, Git identities and recent threads ported; differences below |
| AgentSessionJson.ts | t3-project::session_json | Streaming bounded projection; accounting/scalar differences below |
| AgentSessionImporter.ts | t3-project::sessions::AgentSessionImporter | Recent history and linked-worktree metadata/runtime cwd verified; native runtime adapter/history remain missing |
| http.ts | t3-project::service::{snapshot,mutate} | Service targets implemented; transport wiring remains with handler owner |
| threadWorkspaceRoot.ts | t3-project::promotion::thread_workspace_root | Implemented |
| worktreeContainer.ts | t3-git::worktree_container; t3-project::service | Implemented and verified against real repositories and Windows aliases; inconclusive probes accept, matching TS |

Implemented does not mean every original acceptance test has been ported.

| Ledger row | Service method |
| --- | --- |
| GET /api/projects | ProjectService::snapshot |
| POST /api/projects/mutate; projects.mutate | ProjectService::mutate |
| projects.ensureScratch / createNew | ManagedProjectFolders::ensure_scratch / create_new |
| projects.createFromThread | ScratchThreadPromotion::create_from_thread |
| projectClone.start | ProjectService::start_clone (authoritative hooks), or ProjectCloneTracker::start |
| projectClone.cancel / retry | ProjectCloneTracker::cancel / retry |
| subscribeProjectClones | ProjectCloneTracker::subscribe_project_clones |
| agentSessions.scan / import | AgentSessionScanner::scan / AgentSessionImporter::import |
| subscribeWorktreeSetup / worktreeSetup.cancel | WorktreeSetupTracker::subscribe_worktree_setup / cancel |

Construct ProjectService with ProjectServices: live StoreDatabase, EventSink,
projection reader, legacy importer, clock/IDs, settings, enrichment, clone tracker
and account home. Thread lanes MUST be the dispatcher's own KeyedLanes. Watch
receivers deliver the current borrowed value first, then changed/newest values.
Cancel/retry return the inner boolean for the handler envelope. All wire payloads
use t3-contracts; no local type needs moving there.

Use ProjectService::new_with_git and RepositoryIdentityResolver::with_git with
clones of the environment's single configured Git root. Managed folders derive
checkout handles from that service. bootstrap_automatic returns None only for a
positively identified container; explicit bootstrap retains its existing Result.

EnrichmentOptions supplies cache_capacity/max_pending/concurrency (512/512/4;
each clamped to at least one). with_backend accepts a required ProjectMetadata
adapter; NativeProjectMetadata delegates to the existing identity/favicon
services. Tests gate those real services rather than fabricate repository data.
RepositoryIdentityResolver also exposes with_cache_capacity and with_refiner;
failed optional refinement preserves the unrefined identity. Construct promotion
with ScratchThreadPromotion::new and join its shutdown before closing SQLite.

ForgeCloneRepository uses the forge registry/Git/cwd/home, prepared private URL,
no clone deadline, progress and an owned cancellation token. Git/SSH fallback/
discard behavior stays in t3-forge. Project deletion is soft deletion: children
commit newest-first under locks, then project, then best-effort settings cleanup.
The workspace directory is never deleted. Resources clean up via the outbox.

Stop clone/setup producers before ProjectService shutdown. Join enrichment,
managed tasks, effect worker and sink while Tokio is alive, then close SQLite.
ProjectService shutdown joins admitted mutate calls; hosts must stop invoking
low-level create/update/delete after closing admission.

## Remaining sibling dependencies (blocking completion)

| Blocked work | Named owner / requested home |
| --- | --- |
| Descriptor-bound streaming/listing outside registered workspaces | t3-workspace security-fixes owner; t3-workspace public API |
| Native-history import session mutation locks, registry/runtime policy composition | Orchestration 05 sessions/recovery, with t3-provider and orchestration 04 policy owners |
| Canonical provider identity/refinement and post-clone authoritative status notification | t3-forge owner and server D/F composition owners; t3-git owner for remote-key normalization |
| Redaction in the authoritative Git setup tracker | t3-git setup owner; current tracker clamps raw submodule tails/errors; precise seam request recorded |

Scanner discovery and budget enforcement are in `inventory.rs`, over std
filesystem reads with their own link, identity and bound checks.

- ManagedFolderIo now has server HostFolders: symlink_metadata refuses link or
  non-directory roots and std::fs::remove_dir_all does not traverse nested links.
  Crate tests exercise the service with a temp backend and verify the Windows
  junction FileType predicate used by HostFolders; they do not call the private
  server adapter. No ancestor-swap descriptor guarantee is claimed.
- SessionInventory: `inventory::NativeSessionInventory` reads each enabled
  instance's store (TypeScript's homePath/environment/home order), with all of
  AgentSessionScanner.ts's bounds. Below a store root only real directories and
  regular files are read; transcript identity is checked from the open handle
  before and after a read; paths from content (cwd, Codex roots, gitdir,
  commondir) must be local-disk absolute paths before any filesystem call.
- SessionImportStore: `import_store::RuntimeImportStore` runs TypeScript's SQL
  (list, getByThreadId, insert-ignore upsert, recordImportedTranscript) on the
  existing table and resolves the runtime mode from the policy ceiling and
  project settings. No schema change.
- Registry-aware identity keys/refinement and authoritative Git-status refresh
  after cloning remain. Concrete forge clone and scheduler move_thread_tasks
  adapters now exist and are no longer gaps.

Git files are read in-process (regular files only, 1 MiB, two-second deadline,
16 at once, five-minute backoff for at most 1,024 blocked paths) instead of TS's
POSIX `cat`; a FIFO or other non-regular file is refused rather than timed out.

## Test-source mapping

| TypeScript test | Rust evidence | Coverage |
| --- | --- | --- |
| ProjectService.test.ts | tests/project_service.rs | CRUD/bootstrap/page/receipt replay/conflicts/races/rollback/script IDs/shutdown |
| ProjectService.deletion.test.ts | tests/project_service.rs | Nonempty refusal/archived children/newest-first partial cascade/retry/directory preservation |
| ProjectMutation.test.ts | project_service.rs and trackers.rs | Mutation/discard independently covered; disconnect-during-delete integration remains |
| ManagedProjectFolders.test.ts | managed_identity.rs | Real Git scaffolds/concurrent claims/scratch/names/ownership/cancel cleanup/outside junction |
| ProjectCloneTracker.test.ts | trackers.rs | Real clones/project-first/private URLs/cancel/retry/late success/failure/duplicate/order/concrete adapter/stall/retention; full takeover matrix remains |
| WorktreeSetupTracker.test.ts | trackers.rs and setup unit test | Stages/tails/deadline retention/null/restart subscription/cancel acknowledgement/uncancellable/abandoned subscription reclamation |
| RepositoryIdentityResolver.test.ts | managed_identity.rs | Real upstream/origin/marks/cache/bare/local URLs; full host/mark matrix remains |
| ProjectEnrichmentService.test.ts | enrichment.rs and ProjectService integration | Real Git eviction, saturated lanes, pending bound/dedupe, invalidation, disconnected caller, null/failure/expiry and private URL redaction pass |
| ScratchThreadPromotion.ts lifecycle | promotion.rs | Real SQLite/project creation with gated task-store failure; shutdown joins admitted work |
| ProjectFaviconResolver.test.ts | Existing t3-workspace tests/project.rs | Sibling tests, not rerun here |
| ProjectSetupScriptRunner.test.ts | Existing t3-workspace tests/scripts.rs and project.rs | Execution reused; composition wrapper lacks PTY integration test |
| T3ProjectFileLoader.test.ts | Existing t3-workspace tests/project.rs | Sibling JSONC/schema/trimming/bounds/malformed tests |
| AgentSessionJson.test.ts | parsing.rs | Original duplicate-key examples/five chunk sizes/projection/malformed/depth/allocation/large skipped payload |
| AgentSessionScanner.test.ts | parsing.rs and session_import.rs | Claude/Codex/Pi/message window/identity/known-project scan; discovery matrix blocked |
| AgentSessionImporter.test.ts | session_import.rs | Real projections/runtime/retry/repair/root check; policy/ownership/native-history matrices remain |
| worktreeContainer.test.ts | t3-git tests/worktree_container.rs; project_service.rs | Bare/nonbare containers, checkout/index/unborn/outside refusals, Windows aliases, automatic-only skip and exact user text |

## Behavior differences / remaining parity

- Transport auth/startup/error mapping and metrics stay with owners. Settings
  cleanup now logs the source warning with project ID. Native errors are
  thiserror enums, not exact TS tagged serialization
  or all source user-facing explanations.
- Mutate shields all mutations after caller drop; TS explicitly shields deletion.
  Clone start shields preparation too; TS begins after preparation/claim. Direct
  project methods lack this wrapper. Ordered hydration now uses 8/16 concurrency.
- Enrichment now has separate caches/pending limits, configurable options,
  remembered identities independent of eviction, five-second failure caching,
  single-flight lookups and cancellation-safe owned resolution. Source-equivalent
  warning logs remain absent. Background requests use bounded tracked tasks
  waiting on permits instead of Effect queues. Behavioral tests pass.
- Identity now caches cwd/root separately, coalesces misses and preserves origin
  display case when the sibling parser recovers it. t3-git normalization lacks some host rewrites and
  distinct HTTP ports. Registry refinement is missing. Remote/path tie sorting
  is ordinal, not localeCompare. Clone hooks
  update project/enrichment but do not notify authoritative Git-status readers.
- Managed cleanup additionally refuses paths resolving outside their immediate
  managed parent; removal must reject junctions/links. Canceled scaffolding
  cleans unowned claims; project creation may finish once started. No
  ancestor-swap identity guarantee is claimed. Promotion preserves move order/
  unfinished-work checks and now exposes joined shutdown for admitted validation
  and shielded post-validation mutations. Its full acceptance matrix remains.
- Setup uses cancellation token/completion acknowledgement rather than an Effect
  fiber. Subscribed channels survive retention. Empty unsubscribed entries are
  reclaimed on later access/retention, not a separate sweeper.
- Identity/enrichment and this crate's setup snapshots deliberately remove URL
  userinfo, query and fragment before publication. Current TS identity returns
  raw remotes; this is an intentional security improvement, not exact parity.
  Private Git remotes and executable scripts are unchanged. Git's independent
  setup tracker still requires its owner's redaction fix before subscription.
- Container probes add a 64-KiB regular-file read bound and reject truncated or
  non-UTF-8 Git output as inconclusive. The source has no equivalent byte bound.
- Existing workspace differences are inherited untouched: project files over
  1 MiB, non-UTF-8 and escaped t3.json links are rejected; TS has no whole-file
  byte cap. Contract trimming/enums/optional-key validation, iconPath's 512
  UTF-16 units and 50 scripts are delegated to that loader; its regex JSONC
  parser/post-trim length checks were not replaced. Favicon probes are sequential
  and source-file reads bounded.
- Script composition reads the authoritative thread first, propagates read
  failures before opening, skips fenced linked origins, then resolves supplied
  project / ID / cwd and settings inheritance. Loading untrusted t3.json does
  not itself execute scripts. Worktree cwd, T3CODE_PROJECT_ROOT and
  T3CODE_WORKTREE_PATH, COLORTERM="", NO_COLOR=1, FORCE_COLOR=0 match TS. Both
  impose no script execution timeout. Limits are 400 reported characters and
  4,096 partial-line characters. The sibling runner's startup flushing,
  successful prompt-redraw wait and scalar/UTF-16 slicing differences remain.
- JSON projection conservatively accounts values/keys rather than identical
  stream-json token/chunk allocations; selected lone surrogates/overflowing
  numbers are rejected, discarded numeric text consumes budget. Skipped strings
  stream without retaining payload. Byte/deadline/regular-file protection is in
  `inventory.rs`.
- Pure transcript parsing takes a bounded supplied string and limits Claude to
  100,000 records too; streaming Claude's rolling allocation release is missing.
  Dates use RFC3339 rather than the full JS Date grammar. Text/title trimming,
  scalar slicing and preamble attribute handling differ at JS whitespace/
  UTF-16/regex edge cases.
- Recent threads are a stream, one transcript's history held at a time.
  Candidate aliases use canonical paths, not device/inode identity. Native history and
  pending-native-history import (provider-session lock/two-minute deadline) are
  not implemented.
- Scalar-safe clamping can produce one fewer UTF-16 unit where TS would split an
  astral character. Windows real files/junctions/Git/SQLite/processes are tested;
  POSIX execution was not verified.

## Checks

From rust, use CARGO_TARGET_DIR=D:\t3code-rust-target\project and retain the
configured two-job limit and rustc-gate wrapper (30-minute command deadline):

    cargo fmt -p t3-project
    cargo clippy -p t3-project --all-targets -- -D warnings
    cargo test -p t3-project

Current Windows full test run passed 50 tests: one unit; integration enrichment
6, managed/identity 9, parsing 9, project service 9, promotion 1, session import 4,
settings cleanup 1 and trackers 10; zero ignored/doc tests. The additive Git
container matrix passed separately (one test with real repository variants and
Windows junction aliases). M1, M2, L5, identity redaction and setup redaction were
each observed failing before their behavior fix, then passing. Final tests also
pin exact text, retained sanitized values, unchanged-root legacy updates and
nested-junction target preservation.

Format, exact all-target warnings-denied Clippy and the final full project suite
pass through the configured gate. The additive Git matrix also passes after the
final first-line parser parity adjustment. No verification command remains running.
No actual provider home or live T3 database was read, copied or modified.
