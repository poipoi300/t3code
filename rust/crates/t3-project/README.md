# t3-project

Native transport-neutral services for `apps/server/src/project`. Audit performed
before implementation against all four sibling crates, their READMEs, the
server RPC/HTTP ledger, per-filename searches of orchestration-plan.md, and
reports/orchestration-08-takeover.txt. Package 08 ports orchestration-v2 files,
not the similarly named project-directory services.

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

Tests alongside these TypeScript files are the specification. Existing sibling
implementations will be reused and never edited here.

## Current coverage and handler API

This is an incomplete port: managed filesystem and native session
inventory/runtime adapters remain blocked on sibling public APIs. All are
required constructor inputs; no empty-success default exists.

The forge owner added clone services/progress during this task. The final code
uses those APIs; the initially necessary local progress parser was removed.

| TypeScript file | Current Rust home | Complete? |
| --- | --- | --- |
| ProjectService.ts | t3-project::service; orchestration project stores/planners/EventSink | Core service implemented; metadata differences below |
| ProjectMutation.ts | t3-project::service::mutate | Implemented, shielded delete/clone discard |
| ManagedProjectFolders.ts | t3-project::managed | Service implemented; native ManagedFolderIo missing |
| ProjectCloneTracker.ts | t3-project::clones | Tracker + concrete ForgeCloneRepository adapter implemented; full parity unverified |
| gitCloneProgress.ts | t3-forge::clone_progress; t3-project forwarding adapter | Existing sibling port reused |
| ProjectEnrichmentService.ts | t3-project::enrichment | Partial: independent bounded lanes/cache/invalidation/publication |
| ProjectFaviconResolver.ts | t3-workspace::project::FaviconResolver | Partial sibling port reused; differences below |
| ProjectSetupScriptRunner.ts | t3-workspace::scripts/project; t3-project::scripts | Execution reused; settings/projection/trust composition added |
| WorktreeSetupTracker.ts | t3-project::setup | Implemented: sliding subscriptions/cancel acknowledgement/retention |
| RepositoryIdentityResolver.ts | t3-project::identity; t3-git keys/execution; t3-forge remote recognition | Partial: ordinary/bare repos/fetch remotes/gh marks/cache |
| T3ProjectFileLoader.ts | t3-workspace::project::{load_project_file,parse_project_file}, t3-contracts | Partial sibling port reused; bounds/parser differences below |
| ScratchThreadPromotion.ts | t3-project::promotion; t3-scheduler task-movement adapter | Service implemented; managed folders still required |
| AgentSessionScanner.ts | t3-project::sessions::AgentSessionScanner and transcript | Partial: merge/exclusions/import flags and interpretation; discovery/streaming missing |
| AgentSessionJson.ts | t3-project::session_json | Streaming bounded projection; accounting/scalar differences below |
| AgentSessionImporter.ts | t3-project::sessions::AgentSessionImporter | Recent history import; runtime adapter/native history missing |
| http.ts | t3-project::service::{snapshot,mutate} | Service targets implemented; transport wiring remains with handler owner |
| threadWorkspaceRoot.ts | t3-project::promotion::thread_workspace_root | Implemented |

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

- ManagedFolderIo: t3-workspace needs atomic nonrecursive mkdir/claim, recursive
  mkdir, managed-root writes and no-follow removal. Known-workspace preview/write
  APIs cannot claim/remove unmanaged directories. Tests implement this backend
  only for temporary fixtures, including an actual external Windows junction.
- SessionInventory: workspace/provider composition needs bounded home discovery,
  multi-instance fairness, Codex state_5.sqlite roots/index tail, metadata scans,
  linked-worktree folding, descriptor-bound streaming/restat, and inherited
  rollout prefixes. Current workspace reads require a known workspace and cap
  previews to 1 MiB; no suitable public streaming reader exists.
- SessionImportStore: persistence/provider-runtime owner needs completed/per-thread
  identities, insert-ignore stopped runtimes preserving importedTranscripts,
  atomic transcript recording and project/policy runtime-mode resolution.
  The table exists; a public runtime repository service does not. Test adapters
  use the actual temporary table through t3-persistence.
- Registry-aware identity keys/refinement and authoritative Git-status refresh
  after cloning remain. Concrete forge clone and scheduler move_thread_tasks
  adapters now exist and are no longer gaps.

Inventory bounds still to port: 5,000 transcripts/source; 20,000 discovery and
metadata operations/source; 64 MiB metadata/source; 100,000 metadata records/
source, 1,000/transcript; 8 KiB metadata/32 KiB transcript chunks; 1 MiB cwd scan/
index tail; 30-day recency; 4 GiB/transcript and total import; 100 imports;
100,000 import records except streaming Claude; 200 messages and 32 MiB selected
history. POSIX Git-file reads require two-second subprocess deadlines, 16-wide
reads, five-minute blocked-path backoff and a 1,024-path cap. Import reads must
serialize across callers. These are missing adapter obligations, not guarantees
of the pure transcript helper.

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
| ProjectEnrichmentService.test.ts | ProjectService integration | Dedicated saturation/in-flight invalidation matrix remains |
| ProjectFaviconResolver.test.ts | Existing t3-workspace tests/project.rs | Sibling tests, not rerun here |
| ProjectSetupScriptRunner.test.ts | Existing t3-workspace tests/scripts.rs and project.rs | Execution reused; composition wrapper lacks PTY integration test |
| T3ProjectFileLoader.test.ts | Existing t3-workspace tests/project.rs | Sibling JSONC/schema/trimming/bounds/malformed tests |
| AgentSessionJson.test.ts | parsing.rs | Original duplicate-key examples/five chunk sizes/projection/malformed/depth/allocation/large skipped payload |
| AgentSessionScanner.test.ts | parsing.rs and session_import.rs | Claude/Codex/Pi/message window/identity/known-project scan; discovery matrix blocked |
| AgentSessionImporter.test.ts | session_import.rs | Real projections/runtime/retry/repair/root check; policy/ownership/native-history matrices remain |

## Behavior differences / remaining parity

- Transport auth/startup/error mapping, metrics/tracing/warning logs stay with
  owners. Native errors are thiserror enums, not exact TS tagged serialization
  or all source user-facing explanations.
- Mutate shields all mutations after caller drop; TS explicitly shields deletion.
  Clone start shields preparation too; TS begins after preparation/claim. Direct
  project methods lack this wrapper. Hydration is sequential, not 8/16-wide.
- Enrichment uses fixed defaults (512 entries/four workers per independent lane),
  not configurable options. Last identity lives in its cache entry, so eviction
  loses it. The shared entry limit also couples admission when either lane fills
  all entries, unlike the source's separate caches/queues. Resolver failures
  become cached null. Source-equivalent optional metadata logging and dedicated
  race/saturation acceptance tests remain.
- Identity probes before caching instead of separate cwd/root caching; concurrent
  misses are not coalesced. t3-git normalization lacks some host rewrites and
  distinct HTTP ports. Registry refinement is missing. Remote/path tie sorting
  is ordinal, not localeCompare. Origin display paths are lowercase; primary
  display case is preserved when the sibling parser recovers it. Clone hooks
  update project/enrichment but do not notify authoritative Git-status readers.
- Managed cleanup additionally refuses paths resolving outside their immediate
  managed parent; removal must reject junctions/links. Canceled scaffolding
  cleans unowned claims; project creation may finish once started. No
  ancestor-swap identity guarantee is claimed. Promotion preserves move order/
  unfinished-work checks but lacks an exposed joined post-validation task
  lifecycle and full acceptance matrix.
- Setup uses cancellation token/completion acknowledgement rather than an Effect
  fiber. Subscribed channels survive retention. Empty unsubscribed entries are
  reclaimed on later access/retention, not a separate sweeper.
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
  stream without retaining payload. Byte/deadline/regular-file protection belongs
  to the missing inventory adapter.
- Pure transcript parsing takes a bounded supplied string and limits Claude to
  100,000 records too; streaming Claude's rolling allocation release is missing.
  Dates use RFC3339 rather than the full JS Date grammar. Text/title trimming,
  scalar slicing and preamble attribute handling differ at JS whitespace/
  UTF-16/regex edge cases.
- Recent inventory returns a Vec, not a lazy stream. Discovery/read budgets,
  descriptor identity and policy remain required backend obligations. Candidate
  aliases use canonical paths, not device/inode identity. Native history and
  pending-native-history import (provider-session lock/two-minute deadline) are
  not implemented.
- Scalar-safe clamping can produce one fewer UTF-16 unit where TS would split an
  astral character. Windows real files/junctions/Git/SQLite/processes are tested;
  POSIX execution was not verified.

## Checks

From rust, use CARGO_TARGET_DIR=D:\t3code-rust-target\server-j and retain the
configured Cargo job limit (currently four):

    cargo fmt -p t3-project
    cargo clippy -p t3-project --all-targets -- -D warnings
    cargo test -p t3-project

Latest Windows verification: format passes; cargo test passes 38 tests (one unit,
37 integration, zero ignored; zero doc tests). Integration counts: managed/identity
8, parsing 9, project service 8, session import 3, trackers 9. This includes real
Git/SQLite/processes, an outside-pointing Windows junction, ownership-query
failure refusal and actual scratch recreation. The retention regression was
observed failing before the captured-deadline fix and passing afterwards; the
absent-inode versus literal-string regression was also observed red then green.

The exact required clippy command is blocked by seven sibling dead-code errors
in t3-forge/src/support.rs:4-10 (decode, encode, text, rows, nodes, sha,
quote_patch_path). No sibling files were changed. A --no-deps check can diagnose
owned lints but does not replace this required gate.
No actual provider home or live T3 database was read, copied or modified.
