# t3-project hand-off — upstream 817a9186f9 round

Status: blocked before changed tests can execute. No subagents, commits, pushes,
foreign-crate edits, network clone tests, real provider-home reads or live T3
reads occurred. The coordinator's shared seam file was edited as explicitly
requested; all source/test changes are inside t3-project.

## Workspace and checks

C:\Users\Administrator\t3code-rust\rust\crates\t3-project is authoritative.
Run scoped Cargo from C:\Users\Administrator\t3code-rust\rust with
CARGO_TARGET_DIR=D:\t3code-rust-target\project. Keep TWO jobs and sccache from
.cargo/config.toml. This round used sccache successfully; no wrapper error or
memory-allocation failure occurred. Never restore the stale server-j staging
snapshot. The installed crate already built before its original installation.

cargo fmt -p t3-project passes. Initial cargo test -p t3-project --no-fail-fast
and a later focused linked-worktree test both stopped in a dependency:
t3-orchestration/src/composition.rs:457 constructs ThreadManagementServices
without its newly required worktree_restore field (E0063). This belongs to the
orchestration composition integrator; do not edit it. No command remains running.
No current test pass or observed-red claim is made. The older 38-test full green
and three enrichment-focused greens predate this wave; 47 cases are expected now.

## Current source changes (changed tests FIRST)

- tests/project_service.rs: real bare and nonbare worktree containers must be
  rejected on create/root-change without project mutation. This is the changed
  upstream refusal case; it has not executed yet, and behavior is not patched.
- sessions.rs: added SessionWorktree { path, branch } and optional worktree on
  RecentSession::Importable. The importer intentionally ignores the new metadata
  until its changed test is observed red. Next fix: pass it to import_events,
  persist branch/worktreePath on the imported app thread, and persist runtime
  cwd as worktree.path instead of project root.
- tests/session_import.rs: added a real Git linked-worktree import test asserting
  stored path, branch and runtime cwd. Existing tests pass worktree=None. Fixture
  transcripts now live under an explicit TEMP .codex/sessions tree, not real home.
- tests/trackers.rs: strengthened the existing concrete ForgeCloneRepository
  test to use a TEMP file:// remote, assert real progress callbacks, remove a
  partial initialized repository, and preserve/refuse foreign non-clone contents.
- managed.rs: exported scratch_folder_root(base_dir), reused by scratch_root;
  source semantics unchanged. Cleanup/recovery owners can reuse that convention.
- README: latest drift state and worktreeContainer coverage row added. The
  earlier full ownership audit/RPC ledger and limitations remain present.

## Shared seam file (explicitly requested by coordinator)

C:\Users\Administrator\t3code-rust-notes\briefs\server-seam-requests.md has a
"t3-project next round" section with exact server construction/calls:
- Replace CloningUnavailable with existing ForgeCloneRepository over one shared
  registry/Git/cwd/account_home; ONE ProjectCloneTracker inside the shared service.
  start through ProjectService::start_clone; cancel/retry through clones(); watch
  the borrowed initial snapshot then changes; shutdown owned work before SQLite.
- Promotion already has MoveThreadTasks for ScheduledTaskService. Server G/A
  must expose/retain its existing scheduler Arc (SchedulerAdapter.service is
  private), then use ScratchThreadPromotion::new and join shutdown.
- ONE Arc-backed ProjectService must be shared by runtime bootstrap, launch,
  handlers, managed folders and promotion. Do not reconstruct an independent one.
- Automatic bootstrap must skip specifically a refused worktree container;
  explicit mutations still fail. Do not swallow unrelated startup errors.
- Native SessionInventory is still missing; explicit provider homes alone do
  not create it. Workspace/provider owners need to expose bounded descriptor-
  bound listing/streaming/restat outside registered workspaces. Scanner discovery,
  zstd decoding/decompressed limits, fairness and selection remain t3-project
  work once the public I/O primitives exist. No empty-success inventory.
- ProviderSessionRuntime domain repository (completed/per-thread identities,
  stopped insert-ignore preserving imports, atomic source recording) needs an
  assigned persistence/provider-runtime owner. Native-history import needs
  orchestration 05's session-mutation lock/history port and policy composition.

## Request to active t3-git owner (in same seam file)

Expose conservative Git::is_worktree_container() -> Result<bool, GitError> using
current apps/server/src/project/worktreeContainer.ts and its real-repo test matrix:
.git must be a gitfile targeting canonical metadata strictly inside root; either
bare, or committed with no index; a linked worktree (skip main) must be strictly
inside root. Normal .git directories, valid linked/fresh/orphan/separate-git-dir
checkouts, outside worktrees and inconclusive failures return false. Include
container aliases through Windows junctions/symlinks. The privileged metadata
reader is not public in t3-workspace; do not duplicate its filesystem internals.
Once supplied, call it before create and only changed update roots, returning
Project Invalid with the upstream explanation before committing.

## First on resume

1. After composition owner supplies worktree_restore, run the changed session
   and container cases and observe failures BEFORE changing behavior.
2. Fix linked import path/branch/runtime cwd; assert red then green.
3. Consume the Git owner's probe, complete project refusal and automatic-bootstrap
   integration without a second service instance; record exact server follow-up.
4. Execute the strengthened local-clone test and all previously pending identity,
   enrichment, promotion and service tests. Fix OWN failures only.
5. Native scanner/importer remain blocked on the named public I/O/runtime APIs;
   never read real ~/.codex or ~/.claude and never duplicate sibling internals.
6. Run scoped fmt, exact all-target clippy -D warnings, and full tests; update
   README/seam results with actual evidence, not historical counts.
