# Second coordinator pause — 2026-11-13

Pause requested because the usage reset has not landed. Resume in this thread
only after the coordinator lifts it. No commits, delegates, live data/credentials,
dev servers, workspace-manifest edits or foreign source edits were made.

## Authoritative location and build rules

Crate: `C:\Users\Administrator\t3code-rust\rust\crates\t3-pull-request`.
It was built outside the workspace before its complete directory was moved in
atomically during the first turn. The old staging copy at
`D:\t3code-rust-target\server-k\staging-t3-pull-request` is stale; do not use it.
Set `CARGO_TARGET_DIR=D:\t3code-rust-target\server-k` on every Cargo command.
Keep `rust/.cargo/config.toml` at **two jobs**; never override it.

## Completed implementation

PR application methods, optional Forge/SQLite adapters, authoritative project and
account context, read/cache/mutation policy, linked-thread records, watch
registration, viewed revisions and injected review-guide lifecycle are present.
All wire data uses contracts. README describes public API and ownership.

This resume added optional `orchestration` feature and
`OrchestrationPullRequestBackend`, implementing strict summaries, persisted native
stacks, reads, batching, invalidation, shared change counters, merge refs and link
notifications. It preserves typed error policy and retry dates and joins feeds
and admitted calls on shutdown. Legacy checkout refresh/discovery uses injected
`CheckoutPullRequests`; the host supplies authoritative worktree/project/identity
services. Orchestration polling, wake, sync and settlement were not duplicated.

Also completed ICU base filter ordering, the 100-option limit, diff keys following
new summary revisions, minute diff maintenance, list preview seeding, and driver
login guidance/provider mapping. README now explains the expanded seam.

## Verification and exact stopping point

- Resumed full suite before new edits: **44 passed**.
- New orchestration adapter suite: **5 passed**, including typed policy, strict
  summaries, shared feeds, merge forwarding and shutdown cancellation.
- Latest application suite with `--features forge,sqlite`: **20 passed**. Includes
  viewed revision refresh, diff expiry, updated-head invalidation and preview
  reuse. Collation, updated diff and preview cases were observed red then green.
- Current test count should be **52** (20 application, 7 cache, 6 Forge, 6 guide,
  4 SQLite, 4 watch, 5 adapter). The sixth Forge test for unavailable guidance
  has not yet run. Do not claim a 52-test green suite.
- Last all-feature run failed in another owner's file:
  `t3-orchestration/src/ports/transport/thread_search.rs:117`, E0282 for
  `let current = &mut best[index]`. Leave that file alone. Earlier adapter tests
  compiled/passed before this concurrent change.
- Orchestration also emitted a deprecation warning at
  `src/ports/sessions_recovery/provider_session_manager.rs:130` (`fetch_update`).
  It may block all-feature Clippy with `-D warnings`; leave it to its owner.
- Latest formatting check passed. Earlier default/all-feature Clippy passed before
  this resume; final Clippy remains due. No test or build is intentionally left
  running. The final non-orchestration compile result is appended below.

## First on resume

1. Read the appended compile result. Run the new unavailable-guidance Forge test,
   then scoped default / Forge+SQLite tests and Clippy. Run adapter/all-feature
   checks once the orchestration owner repairs the inference error. `--no-deps`
   can separate owned lint diagnostics; it does not prove dependencies pass.
2. Remaining source audit: environment viewed-file responses currently read the
   records each call; the TS facade caches them for 15 seconds. Host viewed writes
   clear entries rather than bumping a dedicated viewed epoch: audit a slow read
   overlapping a later untick. This was only identified while planning the next
   change; **no edit or new test was started for it**.
3. Production guide generation is blocked on provider text generation and isolated
   checkout adapters behind `GuideGenerator` (no backend found in current provider,
   project or server source). Triage dependencies are listed in README. Review Git
   operations belong to package F. Do not invent substitutes or edit those owners.
4. Finish scoped verification and update status. Do not claim the entire TS suite
   is ported or POSIX is verified.

Inherited before the first turn: no predecessor crate/files. Replaced predecessor
implementation: none. Authoritative work is in the crate above.

Final compile result: `cargo check --offline -p t3-pull-request --features forge,sqlite`
**PASSED** on the final owned source (25.51 seconds, two configured jobs, target
directory `D:\t3code-rust-target\server-k`). All commands have finished.
