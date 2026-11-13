# t3-project hand-off

The usage pause was lifted. The crate is implemented but completion is blocked
on the sibling APIs listed below. No subagents, commits, pushes, sibling edits,
dev servers, live T3 reads or real provider-home reads were performed.

## Authoritative location and build rules

- Installed crate: C:\Users\Administrator\t3code-rust\rust\crates\t3-project.
  It built in external staging before installation with Cargo.toml and src/lib.rs.
- D:\t3code-rust-target\server-j\staging-t3-project is a STALE pre-install
  snapshot. Never copy it over the installed crate.
- Run scoped Cargo commands from C:\Users\Administrator\t3code-rust\rust with
  CARGO_TARGET_DIR=D:\t3code-rust-target\server-j. Retain .cargo/config.toml's
  configured limit (currently four jobs); never override it.
- Only t3-project is owned. Shared brief: t3code-rust-notes\briefs\server-port.md.
  The orchestration-08-takeover report confirms ownership of project-directory
  services here and orchestration-v2 project planning in orchestration.

## Implemented

README.md contains the initial file-by-file audit, current coverage, ledger
mapping, public constructors, test-source mapping and all known parity gaps.
ProjectService snapshot/mutate, receipt-aware CRUD and guarded child deletion;
managed project orchestration; clone/setup trackers; a concrete forge clone
adapter; identity/enrichment; settings/projection script composition; scratch
promotion with scheduler adapter; bounded JSON projection/transcript parsing;
recent-history scanner/import service composition are present.

## Latest verification

- cargo fmt -p t3-project passes.
- Earlier resumed full execution passed 38 tests (1 unit, 37 integration).
- Inode absent versus literal "null" regression was observed red, fixed and
  then passed in that full execution. Empty setup subscriptions are reclaimed;
  their private regression also passed.
- Tightened the existing setup retention test: the old version could pass after
  a delayed timer. Observed red for that exact defect, captured the deadline
  before task spawn, then observed green. Clone retention uses the same correction.
- Extended real-directory refusal coverage: an unavailable ownership query must
  preserve the directory; this passed. The actual outside-pointing Windows
  junction and its sentinel also passed.
- Extended the scratch test to actually delete/recreate scratch. First execution
  failed because optional metadata probes pinned it on Windows. That fixture
  now shuts enrichment down before creating projects, avoiding background pins.
- Final full test execution after these edits PASSED all 38 tests: 1 unit;
  managed/identity 8, parsing 9, project service 8, session import 3, trackers 9.
  Zero failed/ignored; zero doc tests. Scratch recreation now passes as well.
- Required clippy reached owned diagnostics; all owned errors found were fixed.
  Latest exact command is blocked by seven foreign dead-code errors at
  t3-forge/src/support.rs:4-10 (decode, encode, text, rows, nodes, sha,
  quote_patch_path). Do not edit sibling files or claim exact clippy passed.

## Blocking sibling APIs

1. ManagedFolderIo: t3-workspace exports no unmanaged atomic mkdir/claim,
   recursive mkdir, managed-root writes and no-follow directory removal API.
   Its ScopedDir helper remains private. Constructor requires a backend;
   tests implement one only over temporary fixtures.
2. SessionInventory: no native provider-home discovery and descriptor-bound
   bounded streaming/restat API outside registered workspaces. It must supply
   all README scanner budgets, Codex index/state roots, worktree folding,
   inherited prefixes, POSIX deadlines/backoff and global read serialization.
3. SessionImportStore: no public provider-runtime repository for completed and
   per-thread transcript identities, insert-ignore stopped runtimes preserving
   importedTranscripts, atomic recording and runtime-policy resolution.
4. Registry canonical identity/refinement and authoritative post-clone Git-status
   notification remain integration gaps. Forge cloning/progress and scheduler
   task movement now have concrete adapters and are no longer blockers.

Native/pending-native-history import and the remaining acceptance matrices are
incomplete; README documents this explicitly. Do not substitute empty success,
copy sibling filesystem/runtime services, or inspect real provider homes.

## Next

Finish scoped verification and record actual results. When sibling APIs exist,
implement their production adapters, native-history import and remaining parity
matrices. No need to recreate the crate, re-audit its original homes, or restore
staging. Stop service producers/join tasks before closing temporary databases.
