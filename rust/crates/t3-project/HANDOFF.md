# t3-project hand-off — latest resumed wave

Status: incomplete; further verification and native adapter integration are
blocked on the named owners below. No subagents, commits, pushes, sibling edits,
real provider-home reads or live T3 reads were performed.

## Location and rules

Authoritative crate: C:\Users\Administrator\t3code-rust\rust\crates\t3-project.
It was built in external staging before installation. Cargo.toml and src/lib.rs
are present. D:\t3code-rust-target\server-j\staging-t3-project is a STALE
snapshot; never restore it over the installed crate.

Run Cargo from C:\Users\Administrator\t3code-rust\rust, scoped -p t3-project,
with CARGO_TARGET_DIR=D:\t3code-rust-target\server-j. Retain the configured TWO
jobs; do not override them. Only t3-project is owned.

## Work since the previous 38-test report

- Rechecked all blockers and corrected the earlier overly broad claim that
  enrichment could not progress. Its cache/admission gaps were owned work.
- Rewrote enrichment with separate bounded identity/favicon caches and pending
  admission, configurable 512/512/4 defaults, bounded remembered identities,
  coalesced owned lookups, stale-generation protection, cached failures, and
  joined shutdown. Foreground identity reads bypass queue saturation and do not
  launch favicon work. Optional resolver defects are caught as failures.
- Added ProjectMetadata/NativeProjectMetadata constructors. Tests gate the real
  Git/favicon backends; they do not fabricate repository result data.
- Three focused enrichment tests PASSED: remembered identity survives eviction;
  independent saturated lanes/pending bounds/dedupe/foreground reads; invalidated
  lookup cannot replace newer identity and caller disconnect leaves lookup safe.
- Added two more enrichment cases (successful null versus failure/cold; expiry).
  They have not executed because sibling compilation failed afterwards.
- Added a real-repository cwd-cache regression; observed it FAIL before fixing
  RepositoryIdentityResolver. Added separate cwd/root LRU caches, coalesced
  misses, configurable capacity and optional refinement (failure preserves the
  original identity). Preserved origin display casing. Post-fix green awaits
  dependency compilation.
- ProjectService snapshot/page hydration now preserves order with concurrency 8;
  shell hydration uses 16, matching TypeScript.
- ScratchThreadPromotion now has a constructor and shared admission/join scope;
  shutdown waits for admitted foreground validation and shielded mutation tasks.
  Added a real SQLite/project-creation regression with a gated failing task store
  proving a disconnected promotion must be joined. This test has not executed.
- futures-util 0.3 was added to this crate only (already used in the workspace).

## Verification truth

Latest format passes. Earlier complete suite passed 38 tests, BEFORE this wave.
Do not claim those 38 or the older passing --no-deps clippy verify current code.
Current source has 45 expected tests: unit 1; managed/identity 9; parsing 9;
project service 8; session import 3; trackers 9; enrichment 5; promotion 1.
Only three of the seven new cases have passed; cwd cache was observed red before
its fix, and the four remaining new/post-fix cases await a compiling dependency.

Latest full cargo test failed before reaching this crate, in t3-forge:
- bitbucket/checkout.rs:3-4 references absent remote::parse_change_request_url.
- bitbucket/checkout.rs:6 references absent ExecuteOptions::check_exit.
- bitbucket/read.rs:6 borrows a temporary (E0716).
- http.rs:45 calls private github::transport::is_interactive (E0603).
Earlier GitLab and orchestration 06 type errors were repaired by their owners.
No sibling source was changed. Exact clippy must be re-run once dependencies
compile. Do not suppress foreign diagnostics or claim this port complete.

SUBSEQUENT exact clippy attempt failed instead in orchestration 04's
runtime/dispatch.rs:1669: an extra .0 on Arc<Inner>, causing type/lint cascades
at 1701-1707. t3-forge emitted no errors in this later clippy execution; its
older test diagnostics above may already be repaired. The current first gate
is the orchestration 04 decisions/dispatch owner. No commands remain running.

## Named owners / concrete requests

- t3-forge owner: repair the current dependency diagnostics; supply canonical
  registry identity/refinement integration. t3-git owner: remote-key host/port
  normalization gaps. Server D/F owners: authoritative post-clone status wiring.
- t3-workspace security-fixes owner: public unmanaged atomic directory claims,
  writes and no-follow removal; native listing and descriptor-bound bounded
  streaming/restat outside registered workspaces. Scanner business logic remains
  t3-project's work after those primitives exist, not reassigned wholesale.
- Coordinator/persistence owner: supply or explicitly assign the missing
  ProviderSessionRuntime domain repository (completed/per-thread transcript
  identities, insert-ignore stopped runtime preserving importedTranscripts,
  atomic transcript recording). Package 01's report explicitly excludes this
  repository from its store seams; do not invent an accepted package-01 owner.
- Orchestration 05 sessions/recovery, t3-provider and orchestration 04 policy
  owners: native-history session-mutation locking, native history access and
  registry/runtime-policy composition for importing a new thread.
- t3-workspace owns the already-ported t3.json loader/favicon/script executor;
  inherited differences remain listed in README, never patched here.

## First on resume

Run cargo test -p t3-project once orchestration 04 dispatch and forge compile.
Fix OWN diagnostics only.
First observe cwd-cache post-fix green and execute all five enrichment tests and
promotion regression. Then exact cargo fmt, cargo clippy --all-targets --
-D warnings and full cargo test. Preserve real dependencies and temp-only homes.

README contains the initial ownership audit, current file coverage, each RPC/HTTP
mapping, constructors, exact blockers and every recorded TypeScript difference.
Only add production adapters using sibling public APIs; no empty-success backend
or copied native filesystem/runtime repository. Join promotion/metadata/managed
work before closing SQLite. No need to recreate the crate or restore staging.
