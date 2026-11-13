# PR application port integration hand-off — 2026-11-13

## Authoritative location and build rules

The complete crate is at
`C:\Users\Administrator\t3code-rust\rust\crates\t3-pull-request`.
It was built outside the workspace before its complete directory was moved in.
The staging copy at
`D:\t3code-rust-target\server-k\staging-t3-pull-request` is stale.
Use `CARGO_TARGET_DIR=D:\t3code-rust-target\server-k` for every Cargo command.
Respect the workspace's configured four Cargo jobs; do not override them.

## Completed work

The crate supplies application service methods for all 35 PR RPCs from the server
F handoff, optional Forge and SQLite adapters, authorized repository/account
context, persistent and memory read caches, mutations, linked-thread records,
viewed revisions, watch registration, and injected review-guide lifecycle.
Wire data comes from `t3-contracts`; native integration types are not substitute
wire contracts. README describes public API, differences and composition.

The optional orchestration adapter implements `PullRequestBackend`: strict
summaries, persisted stacks, reads and batches, invalidation, shared change feeds,
merge notifications, linked-thread notifications, typed errors and joined
shutdown. Checkout refresh/discovery delegates to `CheckoutPullRequests`.
Watch polling, fingerprint baselines, wake deduplication, sync and settlement
remain owned by `t3-orchestration`; their algorithms were not duplicated.

The final parity work includes ICU filter ordering/100-item cap, list preview
seeding, diff invalidation on updated summaries, diff expiry, driver login
metadata, environment viewed-response caching, viewed-write/read race protection,
and restricting unbound GitHub reads to the public hostname. Focused regressions
were observed failing before their fixes. Error metadata is boxed to keep the
native result error small; the classifier compiles its constant patterns once.

## Remaining integration blockers

- The host must supply authoritative `ProjectSource`, records/cache locations,
  driver/account bindings, `WatchOrchestration`, and `CheckoutPullRequests`, then
  install `OrchestrationPullRequestBackend` in `RuntimePorts::pull_requests`.
  These are owned by server/project/worktree composition outside this crate.
- Production guide generation requires a `GuideGenerator` implementation backed
  by provider text generation, model/enablement policy, stack-aware input and an
  isolated Git checkout/fetch/ref-cleanup service. No production text-generation
  backend was found in the current provider/project/server crates. The lifecycle,
  atomic claims, persistence, normalization, cancellation and cleanup seam here
  are implemented and tested.
- Review Git commands already belong to server F's authorized Git adapter.
  Triage additionally requires server paths, runtime/liveness, boot relay,
  source/version management and session/CLI prompt integration. README records
  those dependencies; no substitute backend or prompt was invented.
- Non-GitHub production behavior depends on upstream Forge driver support.
  POSIX execution and the entire original TS assertion suite are not claimed.

## Verification

Final scoped format check and all-feature Clippy with `--no-deps -D warnings`
passed. The final all-feature suite passed **55 tests**: 1 classifier, 20
application, 7 cache, 7 Forge, 6 guide, 5 orchestration adapter, 4 SQLite, 1 viewed
race and 4 watch tests. Default Clippy with `-D warnings` and all **39 default
tests** also passed. A subsequent dependency-wide all-feature Clippy run met new
concurrent compile errors in `t3-forge/src/gitlab/activity.rs:43,287` (E0716) and
`t3-forge/src/gitlab/read.rs:594` (E0373); those files are outside this task's
ownership. The 55-test pass above predates those new upstream changes. Retry the
all-feature lint/tests after the Forge owner finishes. Do not patch those files
from this task.
No live host/provider calls, credentials, dev servers or user state were used.

First integration step: wire the host-owned contexts/checkout adapter, then the
production guide generator when its upstream services exist. Keep orchestration
watch/sync/settlement ownership as described in README.

Already present at original takeover: no predecessor crate or files.
Predecessor implementation replaced: none.
Only this crate was edited; no commits, git-state mutations, delegates, workspace
manifest changes, foreign source edits or Cargo job overrides were made.
