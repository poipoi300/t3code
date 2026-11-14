# t3-forge

Native, transport-neutral forge layer on Tokio, `t3-contracts`, `t3-process`,
`t3-git`, and reqwest. GitHub operations and the source-control application
services, GitLab, and Bitbucket are implemented. Azure DevOps, GitCafe, Forgejo,
and Cursor Origin are being ported; URL recognition does not imply backend support.

## Public API

- `Forge`: object-safe asynchronous checkout, repository, PR read/write, review,
  diff, identity, discovery, media and sign-in operations. Requests live in
  `forge`; provider-local results live in `model`. Optional/unimplemented
  operations return `ForgeError::Unsupported`, rather than empty answers.
- `ForgeRegistry`: long-lived adapter registration, explicit context binding,
  remote selection and 2,048-entry/five-second successful detection cache.
  Unknown providers return an empty checkout PR listing; other operations fail.
- `remote`: SSH/SCP/HTTP remote parsing, built-in recognition priority, PR link
  parsing, owner/ref parsing and transport-safe diagnostic values.
- `RateLimits`: credential-scoped generation leases, cooldowns and Retry-After.
  Time is an explicit millisecond argument, making race tests deterministic.
- `github::GitHub`: HTTP operations with CLI credential lookup through
  `t3-process` and repository work through `t3-git`. Construct `GitHubHttp` with
  an application-owned `Credentials` implementation or `GitHubCredentials`.
  The latter takes **literal**, already-resolved settings and an explicit
  environment snapshot; it does not load application secrets or live state.
  `with_repository_environment` supplies GH_HOST/GH_REPO equivalents.
- `with_verified_credential` returns a `VerifiedForge`: identity plus an adapter
  pinned to that credential and host. Use its adapter for dependent operations.
  This replaces TypeScript's scoped callback with an owned Rust session.
  `transport::with_interactive(future)` allows a request to draw reserved quota.
- `services::SourceControlRepositoryService`: typed wire-input/output lookup,
  prepare/clone/discard and publish workflows. Pass the server's shared `Git`,
  registry and working directory, with an explicit home directory for `~`
  expansion. Clone deadlines default to 120 seconds; `CloneOptions::timeout =
  None` supports tracked/cancellable clones. `on_progress` receives typed,
  bounded, credential-redacted progress, including bare-CR redraws.
- `services::SourceControlSignIn`: start/wait/cancel/access/starred/star dispatch
  using the same long-lived registered adapters and their application sessions.
  `RepositoryError::wire` and `SignInError::wire` are the existing contracts;
  raw process stderr and credentials do not cross the service boundary.
- `discovery::SourceControlDiscovery`: Git/Jujutsu version probes through
  `t3-process`, plus registered forge discovery. Supply the refreshed process
  environment through `with_environment` when the platform refreshes PATH.
- `review_host::ForgeReviewHost`: implements `t3_git::ReviewHost` from a resolved
  `ForgeHandle`. Its required `ReviewSetup` dependency is supplied by the project
  setup owner; PR resolution does not invoke it. It does not grant workspace
  access or supply orchestration postconditions.

One registry and adapter set should live for the server lifetime. `supports`
reports implemented methods; capabilities describe the exposed feature set.
Do not infer viewer permissions from capabilities. GitHub's event feed is
opt-in through app sign-in, falls back to a personal credential on refusal,
and ends quietly when unavailable; consumers must retain polling.

## Implemented and tested portions

Sources are `packages/source-control-core/src/{server,shared}`,
`packages/source-control-definitions`, `apps/server/src/sourceControl`, and
`packages/source-control-github/src/{server,shared}`.

GitHub includes repository creation/clone URL/default branch lookup; PR creation,
number/URL/branch lookup and owner-qualified REST listing; single/cross-repository
search with repository-list fallback and unresolved repositories; stats,
preview/summary/detail/checks/watch fingerprints; issue/review comments,
review threads and scoped tail paging; review submission, reviewer and label
candidates/mutations; reactions and resolution; viewed-file reads/bulk writes;
raw/paged diffs and revision contents; ready/draft/close/reopen, merge/auto-merge,
update branch, revert, guarded branch deletion and workflow approval. Git checkout,
committed template selection, gh branch-base config and repository stars are
also implemented. Several of these paths still have the differences below.

GitHub additionally includes device-code app sign-in, refresh and rejected-token
tracking; installation coverage and personal-token fallback; CLI account
selection, discovery and settings revision invalidation; media range requests,
safe redirects and SSO signed-URL resolution; SSE renewal/backoff/subscription
cleanup; atomic stack merges and isolated leased cascade rebases; credential-keyed
GraphQL batching and routing/search/node caches; conditional checks and workflow
ETag revalidation; partial GraphQL reviewer/pull-ref recovery; fork remote setup;
and activity actor, commit and dismissal enrichment. Refresh failure ahead of
expiry preserves a usable token, but never restores a rejected one.

HTTP uses the source REST/GraphQL endpoints and copied operation documents under
`src/github/graphql`. Limits include eight shared HTTP slots, a ten-percent
background quota reserve, 30-second requests, 60-second diffs, bounded response
bodies, check/thread pagination and five-page viewed-file reads. Redirects are
refused so credentials cannot move to another origin.

Tests use a real loopback HTTP server, copied TypeScript fixture-builder output,
and real Git repositories in temporary directories with isolated Git config.
`tests/fixtures/provenance.json` records the fixture sources. No test contacts a
forge, invokes gh, uses developer credentials, or reads T3 home. The full
TypeScript suite has **not** been ported. POSIX execution remains unverified.

GitLab uses `glab` through the injectable `cli::ForgeCli` boundary and `t3-git`
for committed templates. It implements checkout/repository workflows, API
discovery and authenticated self-hosted identity refinement; REST/GraphQL MR
paging, detail/checks/stats/permissions, discussions/reactions/commits, diff and
blob reads, review ranges and verdicts, reviewers/labels/references, merge and
auto-merge, guarded source deletion, and revert with tracked cleanup. Queries
follow `packages/source-control-gitlab/src/server/GitLabPullRequestCli.ts`.
Its optional surface follows TypeScript: no app login, credential pinning, SSE,
stack operations, workflow approval, host viewed marks, or cross-repository list.
`GitLab::with_environment` takes the refreshed environment; `with_cli` permits
recorded request/response fixtures. Neither reads a provider home itself.
Eleven GitLab fixture tests passed on Windows. The all-target/all-feature Clippy
run passes with warnings denied (two Cargo jobs; dedicated D: build output).

Bitbucket implements the TypeScript Cloud backend: checkout/repository creation
and clone/default-target lookup, listings and filters, detail/preview/checks,
conversation threads and votes, diff revisions, reviewer UUID preservation,
partial edits, comments/replies/resolution, and review verdicts. API credentials
are explicit `http::ApiCredential` literals; the application resolves settings
and environment precedence. `ApiHttp::update_credential` atomically updates a
registered adapter and changes its settings revision. Calls and redirects pin
one credential snapshot; cooldowns are scoped to its fingerprint. Foreign
origins are refused, response bodies are bounded, and raw diagnostics stay local.
Ten Bitbucket/HTTP fixture tests and a quoted-path revision parser test pass.
Cloud-compatible custom endpoints are explicit; Server/Data Center APIs differ.
Unlike TypeScript's unbounded recursion, non-conversation pagination has a
1,000-page safety ceiling and fails rather than reporting incomplete results.

## Remaining work and known differences

- The server composition root must supply authoritative workspace grants before
  calling any path operation, register adapters, persist `AppSessionStore`,
  resolve setting secrets to literals, refresh PATH, and supply `ReviewSetup`.
  No service reads T3 home, provider homes, application settings or live secrets
  implicitly. Updating credentials uses `GitHubCredentials::update_settings`;
  host-secret resolution remains in the settings/application layer.
- Core definitions are native recognition/link helpers, not the complete
  browser-safe `shared/definition.ts` API. Azure SSH/web canonical keys and
  mounted-host identity refinement remain with their backend ports. Identity
  hooks expose Rust values rather than TypeScript resolver callbacks.
- Native errors preserve classification, retry time, HTTP status, access hints
  and sanitized upstream diagnostic values; exact TypeScript cause classes and
  every error sentence are not reproduced. Diagnostic strings and clone progress
  details are bounded and redact URL credentials. Oversized progress lines are
  discarded rather than buffered without a limit. Discovery accepts an explicit
  refreshed environment; it does not launch a login shell itself.
- The full TypeScript test suite has not been ported. Recorded-response and real
  Git integration coverage is listed below; POSIX execution remains unverified.
- All other backends remain, in the requested order:
  `packages/source-control-gitlab`, `source-control-bitbucket`,
  `source-control-azure-devops`, `source-control-gitcafe`,
  `source-control-forgejo`, `source-control-cursor-origin`. Recognition of their
  URLs is core functionality, not an implementation of those backends.

`Provider*`, selector/context and operation-input structs correspond to native
interfaces in TypeScript source-control-core. Wire actors, capabilities, actions,
checks, reviews and source-control results reuse `t3-contracts`; no missing wire
contract was replaced locally. Therefore there are no substitute wire types to
move to `t3-contracts`.

The PR watch service in `apps/server/src/pullRequest/` is intentionally untouched.
`Forge` exposes summary, detail/checks, activity, thread tails, watch fingerprints,
stack reads and optional events for its future Rust implementation. Its polling,
persistence, notification and agent-wake policy belong outside this crate.

## Server handoff: all 54 deferred RPCs

The dependency direction is `t3-orchestration`'s `PullRequestBackend` port,
implemented by `t3-pull-request::OrchestrationPullRequestBackend`, through that
crate's `PullRequestService` and `ForgeHost`, to this crate's `Forge`. This crate
does not depend on either application crate or duplicate their policy.

`ForgeHost` uses summary/detail/activity/thread-comments/watch-fingerprint/stack
reads, typed list pagination, mutations, capabilities, settings revisions,
credential-pinned sessions and change streams from the same long-lived adapter.
Its application layer augments native forge results with authorized project
identity and converts them to existing wire contracts. Unsupported optional
fingerprints, stacks, cross-repository search or events must be detected through
`supports`; they are never inferred from empty placeholder data.

The orchestration adapter owns counter streams, merge broadcasts and linked-thread
notifications. Its required `CheckoutPullRequests::{refresh,discover}` callback
remains host-owned: resolve authorized worktree/project/repository context, then
reuse `ForgeHost::discover_branch` or checkout `Forge` methods here. The
`ForgeReviewHost` adapter serves the separate `t3-git` workflow port.

Stop orchestration producers and their backend adapter first, then drain/shut
down PR application work, then call `ForgeRegistry::shutdown` before dropping
HTTP/credential dependencies. The registry joins GitHub event-feed cleanup;
dropping one event stream cancels only that subscription. Registry shutdown
closes all its event feeds, including streams held by verified adapter clones.
Normal requests belong to the calling application task's cancellation scope.

Each row names the forge operation available here and the application work that
still belongs to its owner. Forge primitives do not replace `PullRequestService`
policy, authorization, routing, cache invalidation, persisted state or streams.
This crate does not remove registrations from `t3-server/handlers/unsupported.rs`.

| RPC | Service/method in this crate | Application integration |
| --- | --- | --- |
| `sourceControl.lookupRepository` | `SourceControlRepositoryService::lookup_repository` | Authorize cwd |
| `sourceControl.cloneRepository` | `SourceControlRepositoryService::clone_repository` | Authorize destination; choose clone tracking/deadline |
| `sourceControl.publishRepository` | `SourceControlRepositoryService::publish_repository` | Authorize cwd |
| `sourceControl.signInStart` | `SourceControlSignIn::start` | Supply persistent app sessions |
| `sourceControl.signInWait` | `SourceControlSignIn::wait` | Reuse the same adapter/session |
| `sourceControl.signInCancel` | `SourceControlSignIn::cancel` | Reuse the same adapter/session |
| `sourceControl.appAccess` | `SourceControlSignIn::access` | Same registered credential context |
| `sourceControl.starred` | `SourceControlSignIn::starred` | Same registered credential context |
| `sourceControl.star` | `SourceControlSignIn::star` | Same registered credential context |
| `pullRequests.list` | `Forge::list_change_requests`, `list_change_requests_across` | PR service repository/routing/cache policy |
| `pullRequests.listStats` | `Forge::list_change_request_stats` | PR service routing/cache policy |
| `pullRequests.filterOptions` | `Forge::list_repository_labels`, `search_repository_authors`, `search_repository_references` | PR service filter aggregation |
| `pullRequests.summary` | `Forge::get_change_request_summary` | Resolve authoritative PR context |
| `pullRequests.routing` | `Forge::with_verified_credential` | PR service environment/checkout selection |
| `pullRequests.routingIdentity` | `Forge::get_routing_identity` | PR service routing selection |
| `pullRequests.stack` | `Forge::get_change_request_stack` | Resolve authoritative PR context |
| `pullRequests.detail` | `Forge::get_change_request` | Resolve authoritative PR context |
| `pullRequests.preview` | `Forge::get_change_request_preview` | Resolve authoritative PR context |
| `pullRequests.checks` | `Forge::get_change_request_checks` | Preserve verified credential for conditional checks |
| `pullRequests.activity` | `Forge::get_change_request_activity` | PR service discussion/cache policy |
| `pullRequests.threadComments` | `Forge::get_review_thread_comments` | Resolve authoritative PR context |
| `pullRequests.diffFileContents` | `Forge::get_diff_file_contents` | PR service cache/policy |
| `pullRequests.filesViewed` | `Forge::get_files_viewed` | PR service environment-viewed fallback for other backends |
| `pullRequests.changeBreakdown` | `Forge::get_diff`, `get_file_stats` | PR service change aggregation |
| `pullRequests.setFilesViewed` | `Forge::set_files_viewed` | PR service mutation policy/refresh |
| `pullRequests.update` | `Forge::update_change_request` | PR service mutation policy/refresh |
| `pullRequests.comment` | `Forge::comment` | PR service mutation policy/refresh |
| `pullRequests.updateComment` | `Forge::update_comment` | PR service mutation policy/refresh |
| `pullRequests.submitReview` | `Forge::submit_review` | PR service mutation policy/refresh |
| `pullRequests.replyToThread` | `Forge::reply_to_thread` | PR service mutation policy/refresh |
| `pullRequests.setThreadResolution` | `Forge::set_thread_resolution` | PR service mutation policy/refresh |
| `pullRequests.setReaction` | `Forge::set_reaction` | PR service mutation policy/refresh |
| `pullRequests.reportState` | `Forge::get_change_request_summary`, `get_change_request_checks` | PR service report-state policy/formatting |
| `pullRequests.reviewerCandidates` | `Forge::list_reviewer_candidates` | PR service candidate policy |
| `pullRequests.requestReviewers` | `Forge::set_reviewer_request` | PR service mutation policy/refresh |
| `pullRequests.labelCandidates` | `Forge::list_label_candidates` | PR service candidate policy |
| `pullRequests.setLabels` | `Forge::set_labels` | PR service mutation policy/refresh |
| `pullRequests.linkedThreads` | No forge I/O | Orchestration sync-key resolution and linked-thread query |
| `pullRequests.subscribeLinkedThreadRefreshes` | No forge I/O | Orchestration refresh stream |
| `pullRequests.reviewGuide` | No forge I/O | `PullRequestReviewGuideService::get` |
| `pullRequests.generateReviewGuide` | No forge I/O | `PullRequestReviewGuideService::generate` |
| `pullRequests.runAction` | `Forge::run_action` | PR service mutation policy, linking and sync |
| `pullRequests.invalidate` | `Forge::forget_change_request_reads` | PR service cache invalidation and `PullRequestSync::request_sync` |
| `pullRequests.subscribeRefreshes` | Optional `Forge::change_request_events` supplies changes | PR service refresh broadcaster, polling and persistence |
| `subscribeVcsStatus` | No forge I/O | `VcsStatusBroadcaster::stream_status` |
| `subscribeVcsBranch` | No forge I/O | `VcsStatusBroadcaster::stream_branch_status` |
| `vcs.refreshBranch` | No forge I/O | `VcsStatusBroadcaster::refresh_branch_status` |
| `vcs.refreshStatus` | No forge I/O | `VcsStatusBroadcaster::refresh_status` |
| `subscribeWorktreeSetup` | No forge I/O | `WorktreeSetupTracker::stream` |
| `worktreeSetup.cancel` | No forge I/O | `WorktreeSetupTracker::cancel` |
| `git.resolvePullRequest` | `ForgeReviewHost` implements `ReviewHost::get` | Existing `GitWorkflowService::resolve_pull_request` |
| `git.preparePullRequestThread` | `ForgeReviewHost` implements get/checkout/clone URLs/default branch and explicit setup | Existing Git workflow plus authorized project/setup context |
| `git.runStackedAction` | `ForgeReviewHost` implements list/create/targets-remote; `Forge::run_action` handles GitHub stack actions | Commit/PR content generation, link-created/refresh-pushed and status postconditions |
| `vcs.createWorktree` | No forge I/O | Trusted worktree settings/destination, submodules, branch-base persistence and setup |

## Verification

Verified on Windows during takeover: all 67 tests pass, together with crate
formatting and exact Clippy with dependencies (`-D warnings`). The inherited
baseline was 55 passing tests. Added coverage includes real clone/publish and SSH
fallback, URL credential redaction, typed progress, sign-in dispatch, CLI
credential caching/settings changes, discovery and real `GitWorkflowService`
integration. Refresh-failure, whitespace-host verification and stack-permission
batching regressions were observed failing before their fixes. No real forge or
developer credential is used. POSIX execution remains unverified.

Runtime evidence from these passing tests:

- `lookup_repository(github, acme/web)` returns typed clone information from the
  recorded GitHub REST response.
- Publishing an unborn repository reports `remote_added`; publishing after a
  commit retries refused SSH as HTTPS, reports `pushed`, updates the stored
  remote, and leaves the expected file in the real bare remote's `main` branch.
- Automatic clone retries refused SSH as HTTPS; explicit SSH never falls back.
- A credential-bearing loopback clone failure returns no userinfo/query secret
  in its snapshot, public error or Debug representation.
- `GitWorkflowService::resolve_pull_request` through `ForgeReviewHost` resolves
  the expected number, URL and repository without invoking project setup.
- Device-code sign-in through `SourceControlSignIn` saves its own app session
  and resolves its own viewer. Missing-provider sign-in returns the existing
  typed `unsupported` error; optional access/star reads retain their fallback.

Inherited work retained: the complete trait/models/registry and substantial
GitHub adapter, plus the interrupted owner's app, batch, conditional-check,
media, event, discovery and stack modules and their 55 passing tests.
Replaced: rejected-token removal and early-refresh failure behavior; sequential
stack permission reads; noncanonical pinned-host comparison; oversized error
detail storage and an inherited test-module placement that failed Clippy. The
stale README's unimplemented-feature list was reconciled against actual code.

From `rust`, set `CARGO_TARGET_DIR` to `D:\t3code-rust-target\server-k`, then
run `cargo fmt -p t3-forge`, `cargo clippy -p t3-forge --all-targets -- -D warnings`
and `cargo test -p t3-forge`. If an in-progress sibling dependency prevents the
Clippy command, `--no-deps` isolates diagnostics for this crate; it does not replace
the required workspace-integrated check. Keep the workspace's two-job Cargo limit.
