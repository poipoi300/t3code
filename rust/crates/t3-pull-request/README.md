# t3-pull-request

Native, transport-neutral pull-request application services. The default build
uses contracts, injected project/host services, a clock, and an optional durable
cache store. `forge` adds the `t3-forge::Forge` adapter; `sqlite` adds existing
SQLite viewed-file, guide, and linked-thread repositories through `t3-persistence`.
`orchestration` adds the concrete adapter for `t3-orchestration::api::PullRequestBackend`.
No live home directory or credentials are read by this crate.

## Integration API

`PullRequestService::with_records(projects, clock, read_cache, records)` is the
production constructor. `new` uses explicitly ephemeral `MemoryRecords` for
tests/embedders. `ProjectSource::projects` must return current authorized project
records and resolved repository identities; it must not accept client cwd grants.
Construct bindings with one shared `ForgeHost::new(Arc<dyn Forge>)` per driver.
Project settings supply `remove_agent_credits_on_merge`. Unknown-host refinement,
Azure/mounted-host canonical identity, secret refresh, and environment permissions
belong to the project/host composition, not a second registry in this crate.
`ProjectSource::canonical_repository_key` permits that registry's canonical
identity mapping. The Forge adapter permits unbound public reads only on
`github.com`; recognized private hosts still require authorized checkout context.

The service exposes the application methods needed by all 35 `pullRequests.*`
RPCs in the package F handoff. Reads include list/stats/filter options, routing
identity, summary/stack/detail/preview/checks/fingerprint, activity, paginated
thread comments, diff, changed-file expansion, viewed marks, and breakdown.
Writes include action, update, comments, reviews, replies, resolution, reactions,
reviewers, labels, viewed marks, state reporting and invalidation. Linked-thread
queries use `Records::linked_threads`; subscriptions use Tokio watch/broadcast.
Review-guide get/generate are on `PullRequestReviewGuideService`.

`ForgeHost::create` writes an application-owned temporary body file and delegates
creation; `discover_branch` supplies checkout branch lookup. Thread discovery,
link commands and post-create workflow dispatch belong to orchestration/workspace.
`diff` is also the one application method for the paginated HTTP diff endpoint;
auth, HTTP compression and transport flow control remain the server's concern.

Cache TTLs are 30 seconds for lists, 60 seconds for normal summaries/details,
10 minutes for merged summaries/details and commit diffs, and 15 seconds for
checks/fingerprints/viewed marks. Stale detail/summary/diff reads may serve
held values for up to 10 minutes while a scoped background read revalidates.
Strict reads and watch preflight bypass that recovery as requested. Successful
summary and stack reads also have a 60-second persisted cache with striped scope
invalidation and same-key in-flight sharing. Cache filenames hash keys and use a
separate Rust disposable-cache namespace; they do not overwrite Effect cache
files. A `FileStore` can be opened in the configured PR cache directory; callers
may fall back to `MemoryStore` on I/O failure and schedule `prune` hourly.
Viewer identities and batched stats are cached for ten minutes and one minute
respectively. Detail reads seed summary/checks/stats; lists seed up to 64 previews.
File revisions retain bounded partial answers, refresh held views in the background,
and refresh stale revisions before saving marks. Viewed writes and viewed-only
invalidation advance a separate revision so an older pending read cannot restore
superseded marks. A minute maintenance task drops
diffs older than ten minutes; it is joined by shutdown and does not block `drain`.

Repositories are read at most 12 at a time. Cross-repository searches group by
host/cursor boundary and chunk at 100, with per-repository fallback for unresolved
coverage or unsupported batch reads. Filtered single-repository reads cap at five
pages; timestamp boundary exclusions preserve same-instant neighbors. Host errors
retain missing-tool/auth/forbidden/rate-limit/not-found/transient categories and
retry times. A generation lease prevents old successes clearing newer host pauses.
Writes recheck permissions; author edits and reader reactions retain the host's
own authorization rules. Pinned accounts use a verified owned Forge session and
credential fingerprint namespace, including on cache hits.

Call `shutdown` before dropping dependencies. It cancels/join revalidations and
admitted PR reads/writes; `drain` awaits current work without stopping admission.
Review-guide generation has its own joined shutdown and explicit cleanup seam.

## Ownership with t3-orchestration

`t3-orchestration/src/ports/thread_project/pull_request_watch_reactor.rs` owns
polling, quiet/fingerprint cadence, persisted watch baselines and remark IDs,
checks/conflict/terminal wake decisions, wake deduplication, and dispatch. Its
sync reactor and thread PR service own discovery/link projection synchronization;
its settlement service owns settle decisions. Those algorithms are intentionally
not copied here. This crate provides their host reads and notifications.

`PullRequestWatchService::set_watching` ports the separate application registration
service from `apps/server/src/pullRequest/PullRequestWatchService.ts`. Its injected
`WatchOrchestration` reads thread state, resolves targets through the authoritative
host registry, and persists a watch command. It rejects starting watches from
subagents and merged PRs, permits recoverable preflight host failures, and rereads
the actual committed watch. The orchestration adapter must preserve ended-watch
baselines and recheck stopped-run admission in its dispatch path.

With the `orchestration` feature, construct
`OrchestrationPullRequestBackend::new(service, checkouts)` and place an `Arc` of it
in `RuntimePorts::pull_requests`. The dependency is one-way and optional; the
default application crate still builds without orchestration or Forge.

The adapter implements summary, stack, batches_reads, detail, activity,
thread_comments, watch_fingerprint, invalidate, changes, subscribe_merges, and
notify_linked_threads_changed. Summary disables transient recovery; native stack
reads become the persisted `ThreadPullRequestStack` contract. Scoped host event
streams become shared, coalescing `watch::Receiver<u64>` counters. Confirmed merge
events become `broadcast::Receiver<PullRequestRef>`, as the port requires.
Native error categories map to `PullRequestHostError::{Failed { transient },
RateLimited { retry_at }, NotFound, Permanent}`; retry milliseconds become UTC
wire dates. Local validation failures remain non-transient; host transport
failures remain transient, and auth/tool/permission failures remain permanent.

The older required `refresh(cwd, links)` and `discover(cwd, branch)` methods
delegate to the constructor's `CheckoutPullRequests`. That host-owned callback
must use authoritative worktree-to-project and repository identity services.
This adapter does not infer project grants from client paths or reimplement
orchestration's discovery/link/snapshot decisions. The host can reuse
`ForgeHost::discover_branch` for its checkout driver read. No silent empty fallback
is installed when checkout services are unavailable.

Stop orchestration watch/settlement producers first, then call adapter `shutdown`
to cancel and join feeds and admitted calls, then shut down guide/application
services before closing their provider and persistence dependencies. Adapter
shutdown closes its change and merge receivers and refuses late calls.

## Port status and concrete remaining integrations

Sources: `apps/server/src/pullRequest/{PullRequestService,PullRequestReadCache,
PullRequestWatchService,pullRequestViewedFiles,linkedThreads,
pullRequestReviewGuideNormalize,PullRequestReviewGuideService}.ts`, and
`packages/shared/src/changedFileKind.ts`. Tests map to those source suites and
use scripted hosts/Forge implementations, temporary filesystem/SQLite state,
explicit clocks and synchronization events. No external forge/provider call is
used. This is not a claim that every assertion in the large TS suite is ported.

Review-guide orchestration, atomic claims, stored results, normalization,
cancellation and cleanup are present. Production generation is blocked on an
implementation of `GuideGenerator`: project/server enablement and model policy,
the provider text-generation backend (not exposed by `t3-provider`), and an
isolated Git checkout/fetch/ref-cleanup service. Its `prepare` contract requires
validated refs, exact head identity and cleanup; this crate never invents shell
commands from PR branch names or model output. The backend must also construct
the full stack-aware generation input from activity/settings.

`apps/server/src/review/ReviewService.ts` is already served by package F's
authorized Git adapter (`t3-server`/`t3-git`); it is not duplicated here.
`apps/server/src/triage/` is not self-contained: it needs server-derived paths,
runtime-state/liveness, boot-service and relay metadata, version/source-clone
management, filesystem session writing, and CLI/app prompt integration. Those
dependencies are owned by server/config/workspace, not the PR application seam;
no substitute production triage implementation or prompt is installed here.

The disposable Rust cache intentionally has its own byte format and filename
namespace, rather than sharing Effect persisted-cache entries. Filter options use
ICU base-strength collation and the source's 100-item limit. Driver-provided login
requirements and provider identity survive unavailable-error mapping. Non-GitHub
forge behavior remains constrained by the upstream drivers' supported operations.
This port has focused behavior coverage, not every assertion from the full TS
suite. POSIX execution is unverified.

All wire inputs/results use `t3-contracts`. Context, clocks, native host pagination,
cache entries, generator work and watch dispatch structs are integration types;
there are no replacement wire DTOs to move to contracts.

## Verification

Use `CARGO_TARGET_DIR=D:\t3code-rust-target\server-k` and the workspace's configured
four Cargo jobs without overriding them. Run scoped `cargo fmt -p t3-pull-request -- --check`,
`cargo clippy -p t3-pull-request --all-targets --all-features -- -D warnings`,
and `cargo test -p t3-pull-request --all-features`. Final verification results and
remaining host integration requirements are recorded in `HANDOFF.md`.
The final scoped format, default Clippy and 39 default tests passed. All-feature
Clippy with `--no-deps` and all 55 tests passed before concurrent Forge GitLab
changes introduced compile errors; the later dependency-wide lint run is blocked
in `t3-forge/src/gitlab/activity.rs` and `gitlab/read.rs`. See the hand-off for
exact diagnostics. No Forge files were changed here.

Takeover: no predecessor crate or files existed. No predecessor implementation
was replaced. Only this crate is owned; no commits, git-state mutations or
edits to orchestration, server handlers, contracts, workspace manifest or Cargo
job configuration are made.
