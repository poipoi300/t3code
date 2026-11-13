# Paused at coordinator request

Account weekly usage is nearly exhausted. Stop requested; do not resume until
the coordinator asks. No subagents, commits, pushes, sibling edits or live T3/
provider-home reads were performed.

## Latest pause checkpoint (second pause; read this first)

The coordinator resumed this task, then requested another approximately one-hour
pause because the usage reset has not actually happened. No command remains
running. Do not resume until instructed. The authoritative installed crate and
D: target rules below are unchanged; never restore the stale staging snapshot.

Work completed during the resumed interval:

- Rechecked sibling APIs: native ManagedFolderIo, SessionInventory and
  SessionImportStore adapters are still unavailable. README lists the exact
  requirements; do not bypass them with duplicated native I/O or empty success.
- Required format command passes, including after the latest source edits.
- The full test run reached 36 passing integration tests; the last new timing
  test hung. Direct diagnostic reproduction proved that it paused Tokio before
  launching real Git, causing a virtual 30-second process timeout. Moved pause
  after the real Git readiness signal and bounded retention observation using
  virtual time. The focused timing test then PASSED (1 passed, 8 filtered).
  It has not yet been included in a completed final full-suite run.
- Ran clippy with --no-deps to reach owned diagnostics despite sibling lint
  failures. It found five owned lint errors: redundant sequence field,
  matches!-style availability match, filter/next_back, and two clone-on-Copy
  runtime modes. All five were manually fixed. Final clippy rerun remains.
- Found a real identity bug: absent inode collided with string "null".
  Extended the existing parsing test and observed it FAIL for that exact
  collision. Fixed sessions.rs inode normalization to Option<Option<String>>,
  preserving a separate absent tag. The post-fix test has NOT yet executed.
- Found abandoned empty setup subscription entries were retained. Added a
  private unit test, but its attempted run was BLOCKED before reaching this
  crate by t3-orchestration::ports::transport::thread_search.rs:117 (E0282,
  insufficient type information for best[index]). At the coordinator's stop,
  finished the edit in hand: prune_idle now removes empty entries with zero
  receivers on begin/get/subscribe, preserving live subscriptions; moved the
  unit test to the end of setup.rs. This latest change is formatted but NOT
  typechecked/tested due the sibling build failure and pause instruction.

First on next resume:

1. Run the existing transcript-identity regression and the new private
   setup::tests::dropped_idle_subscriptions_are_reclaimed test once siblings
   compile. Expected total is now 38 tests (37 integration + 1 private unit).
2. Run cargo fmt -p t3-project; cargo clippy -p t3-project --all-targets --
   -D warnings; cargo test -p t3-project, with the assigned D: target and two
   configured jobs. --no-deps may help diagnose owned lints, but must not be
   reported as passing the exact required clippy command.
3. Do not classify slow/quiet RTK output as a test hang without direct output.
   rtk proxy cargo prints live build/test output. The initial resumed focused
   run was stopped during dependency rebuilding, not proven hung; the later
   full suite established the timing test's actual failure, fixed above.
4. Keep README coverage/limitations accurate, replace stale check counts with
   actual final results, and complete or concretely report the missing sibling
   APIs. No new native adapter/public sibling API appeared during this interval.

The older checkpoint below is retained for provenance; this section supersedes
its test counts, last-error locations and immediate next steps.

## Location and build rules

- Authoritative crate: C:\Users\Administrator\t3code-rust\rust\crates\t3-project.
  It was successfully checked in staging before its complete directory was
  installed. Cargo.toml and src/lib.rs are present.
- The old D:\t3code-rust-target\server-j\staging-t3-project directory is a STALE
  pre-install snapshot. Do not copy it over the installed crate.
- All Cargo commands from C:\Users\Administrator\t3code-rust\rust with
  CARGO_TARGET_DIR=D:\t3code-rust-target\server-j. Keep configured jobs=2.
- The source brief is t3code-rust-notes\briefs\server-port.md; also read
  reports\orchestration-08-takeover.txt. Only t3-project is owned.

## Completed

README contains the before/after file coverage audit, handler ledger mapping,
required sibling APIs, test-source mapping and explicit parity differences.
Core ProjectService uses existing orchestration planners/stores/sink, including
receipt replay, conflicts, rollback, soft deletion and newest-first partial
child cascade. Added clone/setup trackers, managed-folder services, identity/
enrichment, settings/projection script composition, scratch promotion and
scheduler adapter, streaming JSON projection, transcript interpretation and
recent-session import.

t3-forge acquired SourceControlRepositoryService and clone_progress during this
task. The current crate now reuses those, with a concrete ForgeCloneRepository
adapter; do not restore the removed duplicate parser. The scheduler already
provides move_thread_tasks, and its adapter is implemented.

## Verification checkpoint

- cargo fmt -p t3-project passed repeatedly; format again after latest test.
- A full cargo test execution passed 35 tests. Later the concrete forge backend
  test passed independently (1 passed, 7 filtered). Do not claim the final source
  is fully verified: small source/test edits followed those executions.
- A setup subscription regression was observed failing after retention; fixed
  by retaining subscribed channels, then observed passing.
- Later checks repeatedly encountered concurrent sibling failures in
  t3-orchestration. Last clippy errors were engine.rs redundant run_id field and
  provider_turn_start_service.rs moving source_run_id inside a find closure.
  Earlier dispatcher/projection errors were repaired by their owners; do not
  edit sibling files yourself. Own final clippy diagnostics have not yet been
  obtained from a clean dependency build.
- The last new test is
  trackers::clone_marks_stall_and_expires_success_only_after_retention. Its
  focused Cargo run was started immediately before the stop request. When its
  captured exec session was stopped/read, it had already failed in the sibling
  decision engine: usage_limit_holds_queue at engine.rs:4070 lacks the new second
  argument. This test did not execute. No build or test remains running.
- All filesystem/session tests use temp fixtures. An actual outside-pointing
  Windows junction preserved its sentinel. Git tests use isolated configuration;
  latest edit adds the same environment injection to identity resolver probes.

## First on resume

1. Inspect the latest source and README. There should be 37 tests after the last
   two tracker additions; verify by actual execution, not this estimate.
2. Run format, focused last tracker test, then full crate tests and clippy once
   sibling dependencies compile. Fix owned diagnostics only. If retention
   starts late because its spawned task begins sleeping after scheduling, use
   an Instant deadline captured when finishing, not elapsed-time assertions.
3. Review clone insertion-order, cancellation during post-clone hooks, selective
   projection helper and managed-scaffold cancellation edits against tests.
4. Finish the actual missing adapters only when sibling public APIs exist.
   ManagedFolderIo needs unmanaged atomic mkdir/write/no-follow removal.
   SessionInventory needs native discovery and descriptor-bound bounded streaming
   outside registered workspaces; SessionImportStore needs the public provider
   runtime repository/policy integration. Do not replace them with empty success
   or duplicate sibling filesystem/persistence services. README enumerates all
   scanner budgets and other incomplete parity/acceptance matrices.
5. Update README with final actual check counts/results and report the required
   coverage table, ledger methods, dependencies and all differences. No commit.
