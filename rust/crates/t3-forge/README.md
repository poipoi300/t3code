# t3-forge

Native, transport-neutral forge layer on Tokio, `t3-contracts`, `t3-config`, `t3-process`,
`t3-git`, and reqwest. GitHub operations and the source-control application
services, GitLab, Bitbucket, Azure DevOps, GitCafe, Forgejo/Gitea and Cursor Origin
are implemented. Optional operations follow each source backend's supported surface.

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
- `bindings::ForgeBindings`: maps authoritative project records to native driver,
  account, checkout and canonical repository bindings; resolves watch targets and
  refreshes/discovers existing thread-link contracts. The server implements the
  foreign PR traits by delegating forge work here and retaining its project,
  workspace-authority and orchestration-command dependencies.
- `settings::{github,bitbucket,gitcafe}`: settings/environment credential precedence
  from already-resolved host values. Unresolved references and redaction markers
  fail before another environment account can be selected.

One registry and adapter set should live for the server lifetime. `supports`
reports implemented methods; capabilities describe the exposed feature set.
Do not infer viewer permissions from capabilities. GitHub's event feed is
opt-in through app sign-in, falls back to a personal credential on refusal,
and ends quietly when unavailable; consumers must retain polling.

## Implemented and tested portions

The current upstream specification is the TypeScript after merge `817a9186f9`.
The Azure launcher drift recognizes only complete MSI/ZIP layouts and runs their
bundled Python with `-X utf8 -IBm azure.cli` and `AZ_INSTALLER`. Other layouts and
non-Windows platforms keep `az`. Invocation is resolved once per adapter; fake CLI
tests explicitly select the direct invocation. `CliRequest::program` is now an owned
string to carry the resolved interpreter path.

Forgejo branch lookup detects servers ignoring `head=`, shares one list per
serving account/repository, scans only its changed front and fully rescans after
one hour. Candidate matches are reread to reject deletion, renamed forks and stale
state; equal timestamps sort by descending PR number. The cache holds at most 128
repositories. These behaviors are covered by the current upstream drift fixture.

## Credential storage and sign-in

`github::session_store::SettingsAppSessions` is the production `AppSessionStore`.
Construct it from the SAME environment-owned `Arc<t3_config::SettingsService>`
and its matching `SecretStore` at the explicitly supplied `HomeLayout::secrets()`.
It uses the existing `sourceControlHosts.github.signIns[host]` setting: settings
JSON holds its redaction marker, while the matching secret file holds UTF-8 JSON
with `accessToken`, `accessTokenExpiresAt`, `refreshToken` and
`refreshTokenExpiresAt`. The secret filename is the config store's existing
`source-control-<base64url(github)>-<base64url(signIns)>-<base64url(host)>.bin`.
The existing secret store applies POSIX 0700/0600. Its non-Unix permission helper
is a no-op: Windows protection depends on the directory ACL provisioned by the
host and was not verified here. This crate adds no encryption or new token database. `MemoryAppSessions` is ephemeral/test storage.

Refresh compare-and-replace holds the same settings write lock as user updates
and sign-out, compares the actual stored access token, then reloads/publishes the
settings snapshot. A deleted secret never falls back to a cached plaintext value.
Reopen, stale refresh, settings sign-out and concurrent refresh/sign-out fixtures
cover this boundary. Store writes advance the adapter revision. The host must
also update its live credential handles/invalidate PR caches on settings changes.

Device codes and polling results remain in memory; only completed sessions are
stored. Only the current GitHub TS driver exposes app sign-in and starring.
It supports github.com, an injected public app client id and the device-code
browser verification page; the service returns the prompt instead of opening a
server browser. Other forges retain their source-supported CLI/token paths and
canonical unsupported sign-in responses. Verification links must be HTTP(S),
bounded and free of userinfo. Denial, expiry, cancellation and shutdown do not save
sessions. Cancel is idempotent; shutdown closes admission and joins polling tasks.
`AppSession`, credentials and CLI output never expose tokens through Debug;
transport/store failures use fixed error text. Settings secret references are
resolved by the host's existing resolver before `settings::*` conversion.

Repository services preserve source behavior for local clone names, including
`--bare` after Git's option terminator, SSH-to-HTTPS fallback, typed progress,
bounded deadlines and guarded cleanup. Only fixed native repository hints are
forwarded from provider failures. Cursor private-visibility update failures tell
the caller that its repository was already created and remains team-visible.

## Backend coverage

Sources are `packages/source-control-core/src/{server,shared}`,
`packages/source-control-definitions`, `apps/server/src/sourceControl`, and
`packages/source-control-{github,gitlab,bitbucket,azure-devops,gitcafe,forgejo,cursor-origin}/src/{server,shared}`.

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
Eleven GitLab fixture tests cover its native CLI/API behavior.

Bitbucket implements the TypeScript Cloud backend: checkout/repository creation
and clone/default-target lookup, listings and filters, detail/preview/checks,
conversation threads and votes, diff revisions, reviewer UUID preservation,
partial edits, comments/replies/resolution, and review verdicts. API credentials
are explicit `http::ApiCredential` literals; the application resolves settings
and environment precedence. `ApiHttp::update_credential` atomically updates a
registered adapter and changes its settings revision. Calls and redirects pin
one credential snapshot; cooldowns are scoped to its fingerprint. Foreign
origins are refused, response bodies are bounded, and raw diagnostics stay local.
Eleven Bitbucket/HTTP fixture tests and a quoted-path revision parser test cover it.
Cloud-compatible custom endpoints are explicit; Server/Data Center APIs differ.
Unlike TypeScript's unbounded recursion, non-conversation pagination has a
1,000-page safety ceiling and fails rather than reporting incomplete results.

Azure DevOps uses `az` with its DevOps extension through `ForgeCli`. It covers
checkout/repository workflows, discovery and PAT viewer fallback, raw-offset PR
paging, summary/detail/activity, reviewer writes, edits, and supported state and
auto-complete actions. Diffs pin an iteration; whole item reads have eight shared
spawn slots, binary metadata and byte limits, native bounded Myers diffing on
blocking workers, and the source file/edit/byte slice budgets. Blob revisions
preserve unknown paths when change paging is truncated. The native text diff
replaces TypeScript's `diff` dependency; no Node/Python helper is launched.
Eleven Azure CLI fixture tests, launcher refusals and three bounded-diff tests cover it.
Azure review writes, check lists, reactions, and reviewer candidates remain
unsupported exactly as in the source backend. Repository visibility follows
project permissions because `az repos create` has no visibility option.
Values beginning with `@` are refused where Azure could treat them as local-file
indirection; the authorized PR description file is passed intentionally.

GitCafe includes literal/settings/environment/CLI credential precedence, scoped
staging credentials, five-minute helper-token caching and live settings updates;
managed CLI discovery, checkout and asynchronous repository admission; actor-ID
filters and creation-position list cursors; checks, line totals, labels, review
threads, reactions and permissions; versioned writes and head-fenced reviews;
immutable fork source reads and revision-pinned diff pages; and stack landing or
restacking fenced by the reviewed heads. API redirects stay on the configured
origin under `/api/`; non-preserving write redirects are refused. Error text is
fixed rather than echoing a remote problem response. Thirteen GitCafe fixtures
cover it. Optional methods absent from the source (for example inline draft reviews,
viewed-file host marks, and commit diffs) return `Unsupported`.
`gitcafe::credentials::GitCafeCredentials` takes literal settings, an explicit
environment snapshot and the CLI working directory; it never resolves secrets or
reads provider homes. `ApiHttp::ApiCredentials` supplies pinned request-time
authentication, including 401 invalidation; the direct credential API remains
available for Bitbucket. The composition root owns settings-to-adapter wiring.

Forgejo/Gitea covers repository and checkout workflows, authenticated unknown-host
and SSH alias refinement, mounted serving URLs, listings, details and required
check globs, comments/reviews/commits/reactions, diffs and fork file contents,
reviewers/labels, edits and merge/update/close/reopen actions. It prefers configured
`fj` accounts, validates them with `fj whoami`, rereads renewed OAuth credentials,
then uses native HTTP with token authentication. `tea` is the fallback when no
configured fj account serves the target; a rejected configured account cannot
silently change identity to tea. JSON writes use stdin. Redirects remain inside
the serving origin and mounted API path. Eleven Forgejo fixture tests cover these
boundaries. Replies, resolution and draft conversion are unsupported as in the
source backend.

Construct `Forgejo` with `ForgejoConfig`: an authorized CLI working directory,
explicit environment and resolved key-file candidates. `transport::keys_paths`
computes the current platform's modern and legacy locations from caller-supplied
home/data-home/app-data paths; it reads no environment or home itself. Empty
candidates disable native fj credential reads. Authentication files are bounded
to 1 MiB. No provider's credentials appear in Debug output or remote errors.

Cursor Origin covers private repository creation, namespace discovery, clone URLs,
checkout and PR creation/listing, summary/detail/checks/activity, version-pinned
diffs, state/draft/merge actions, edits, comments, inline reviews, replies and
thread resolution. Its explicit `CursorOriginConfig` supplies the authentication
working directory and environment; only `origin auth token/status` and checkout
or decorative avatar reads use the CLI. Native HTTP reuses pooled connections,
serializes token reads, caches successful sessions for five minutes and failures
for ten seconds, and refreshes a refused token once. Late refusals cannot clear
a newer session. Long page tokens stay behind bounded opaque cursors; expired
cursors request a refresh. A changed base triggers mergeability verification;
an open stacked parent prevents an implicit stack merge. Thirteen Cursor fixtures
cover these behaviors. Repository creation is followed by a private-visibility
update; failure identifies the already-created repository and team exposure.
Reviewer requests/candidates, reactions, labels, blob contents and host viewed
marks remain unsupported, matching the source's optional surface.

All checkout listings interpret an absent state as every state. This matters to
`ForgeHost::discover_branch` and `ForgeReviewHost`: a merged branch must still
resolve. Bounded pagination reports truncation; Cursor's incomplete all-green
check list remains pending. Registry shutdown cancels HTTP/CLI work and joins
GitHub event producers and GitLab cleanup tasks.

| Backend | Construction and credentials | Source-supported optional features |
| --- | --- | --- |
| GitHub | `GitHub` + `GitHubHttp` + explicit `Credentials` | App auth, pinned sessions, batches, stacks, events, media, host viewed marks |
| GitLab | `GitLab::new` + refreshed environment/optional `ForgeCli` | Discussions, reactions, labels/reviewers, blob revisions, auto-merge/revert |
| Bitbucket Cloud | `Bitbucket::new(ApiHttp, Git)` + literal/updatable `ApiCredential` | Threads/votes, reviewer UUIDs, patch-derived revisions |
| Azure DevOps | `AzureDevOps::new` + refreshed environment/optional `ForgeCli` | Iteration diffs/revisions, reviewers, auto-complete |
| GitCafe | `GitCafe::new(Git, GitCafeCredentials, RateLimits)` | Versioned writes, fork snapshots, guarded stack land/restack |
| Forgejo/Gitea | `Forgejo::new(Git, ForgejoConfig)` | fj/tea account routing, mounted hosts, required checks, fork contents, reactions |
| Cursor Origin | `CursorOrigin::new(Git, CursorOriginConfig)` | Session refresh, inline threads, pinned patches, guarded merge |

## Remaining work and known differences

- The server composition root must supply authoritative workspace grants before
  calling any path operation, register adapters, construct `SettingsAppSessions`,
  resolve setting secrets to literals, refresh PATH, and supply `ReviewSetup`.
  No service reads T3 home, provider homes, application settings or live secrets
  implicitly. Updating credentials uses `GitHubCredentials::update_settings`;
  host-secret resolution remains in the settings/application layer.
- Core definitions are native recognition/link helpers, not the browser-safe UI
  `shared/definition.ts` API. `remote::azure_{repository_key_from_remote,
  canonical_repository_key,repository_selector}` supplies Azure SSH/legacy web
  normalization and decoded CLI names to the project/host composition. Azure,
  GitLab, Forgejo and Cursor identity hooks expose existing Rust contracts.
- Native errors preserve classification, retry time, HTTP status, access hints
  and sanitized upstream diagnostic values; exact TypeScript cause classes and
  every error sentence are not reproduced. Diagnostic strings and clone progress
  details are bounded and redact URL credentials. Oversized progress lines are
  discarded rather than buffered without a limit. Discovery accepts an explicit
  refreshed environment; it does not launch a login shell itself.
- The full TypeScript test suite has not been ported. Recorded-response and real
  Git integration coverage is listed below; POSIX execution remains unverified.
- Native Cursor reads are performed from a fresh PR basis rather than launching
  speculative reads from the last observed PR. Successful PR/check reads cache
  for one second, repository settings five minutes, base tips two seconds and
  verified immutable patches/mergeability one hour. Caches are bounded and
  generation/revision scoped; some evict all entries at capacity rather than the
  source's FIFO policy. There is no detached connection warmup or avatar task.

`Provider*`, selector/context and operation-input structs correspond to native
interfaces in TypeScript source-control-core. Wire actors, capabilities, actions,
checks, reviews and source-control results reuse `t3-contracts`; no missing wire
contract was replaced locally. Therefore there are no substitute wire types to
move to `t3-contracts`.

`Forge` exposes summary, detail/checks, activity, thread tails, watch fingerprints,
stack reads and optional events to the implemented `t3-pull-request` and
`t3-orchestration` services. Their polling, persistence, notification and agent-wake
policy belong outside this crate.

## Server handoff: discovery and the 54 deferred RPCs

The server construction instructions, exact ten-method dispatch table and native
delegations for `ProjectSource`, driver/account bindings, `WatchOrchestration` and
`CheckoutPullRequests` are in
`C:\Users\Administrator\t3code-rust-notes\briefs\server-seam-requests.md`,
section **t3-forge round 2**. This crate cannot depend on `t3-pull-request`, whose
Forge feature already depends on it. The server's foreign trait implementations
retain authority and command admission; they forward forge work to `ForgeBindings`.

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
| `server.discoverSourceControl` | `SourceControlDiscovery::discover` | Refreshed environment and the same registered adapters |
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
| `git.runStackedAction` | `ForgeReviewHost` implements list/create/targets-remote; `Forge::run_action` handles GitHub/GitCafe stack actions | Commit/PR content generation, link-created/refresh-pushed and status postconditions |
| `vcs.createWorktree` | No forge I/O | Trusted worktree settings/destination, submodules, branch-base persistence and setup |

## Verification

Final round-2 Windows verification passes:

- `cargo fmt -p t3-forge --check`.
- `cargo clippy -p t3-forge --all-targets --all-features -- -D warnings`, including dependencies.
- `cargo test -p t3-forge`: **152 tests across 17 suites**.
- `cargo check -p t3-pull-request --all-targets --features forge,orchestration,sqlite`.

Build output is `D:\t3code-rust-target\forge`, with the unchanged two-job config.
A concurrent Git-owner type/lint edit briefly blocked earlier checks and settled
before final verification. No sccache override was needed; no sibling source was
changed. The server's RPC registrations and authoritative trait wrappers remain
with its integrator, using the concrete instructions in the seam file.

The inherited baseline was 55 passing tests. Added coverage includes real clone/publish and SSH
fallback, URL credential redaction, typed progress, sign-in dispatch, CLI
credential caching/settings changes, discovery and real `GitWorkflowService`
integration, all six additional backend fixtures, paging/version fences and
shutdown. Refresh-failure, whitespace-host verification and stack-permission
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

From `rust`, set `CARGO_TARGET_DIR` to `D:\t3code-rust-target\forge`, then
run `cargo fmt -p t3-forge --check`, `cargo clippy -p t3-forge --all-targets --all-features -- -D warnings`
and `cargo test -p t3-forge`. If an in-progress sibling dependency prevents the
Clippy command, `--no-deps` isolates diagnostics for this crate; it does not replace
the required workspace-integrated check. Keep the workspace's two-job Cargo limit.

## Security review follow-up

GitLab destinations are now bound before glab request construction: the default
or configured server `GITLAB_HOST`, `with_configured_hosts` authorities, and the
remote in an authorized `CheckoutContext`. Ports are part of the authority.
Repository/PR URLs, explicit request hosts, `requested_host` and provider base URLs
do not create trusted hosts. Checkout trust is scoped to the call. Hosts used by
PR API bindings must be configured by the server. The server must authorize cwd
and construct checkout contexts from its project records; see the seam requests.

| Finding | Disposition and evidence |
| --- | --- |
| GitLab client-selected credential destination | Fixed. `tests/gitlab_hosts.rs` refuses lookup, clone, thread preparation and review clone-URL overrides before any fake CLI request; configured/authorized remote acceptance and scope isolation are covered. All four refusal cases reproduced before the guard. |
| Cancel after success overwrites completion | Fixed. Completion is changed only while pending, including cancellation, replacement and shutdown. The persistent sign-in service test waits successfully again after cancel; it failed with Cancelled before the fix. |
| Clone URL and snapshot reserialization | Fixed. Git receives the original operand except TypeScript's pathless-query slash insertion. Snapshot redaction serializes only when removing userinfo or a nonempty query. Unit cases cover drive paths, spaces, dot segments, encoded dot segments and fragments; real clones cover option-like names and spaces. |
| Fresh app token reads wait on refresh gate | Fixed. Fresh reads bypass the refresh gate; refresh re-reads after acquiring it. A test holds the gate while a fresh read completes. |
| Coverage cache locked during installation HTTP | Fixed. Separate gates serialize each credential's population; the global map lock covers cache operations only. A stalled loopback request leaves that lock available. |
| Session marker removal and write snapshot/update race | Confirmed, blocked on an atomic conditional mutation API in `t3-config`. `with_snapshot` holds its write lock but cannot update settings; `update` reacquires it. The precise request is in `server-seam-requests.md`. No claim of completion for this finding. |
| Tests that never exercise Git operands | Fixed. Real repositories named `--bare` and `--upload-pack=remote-secret` are cloned and checked for contents and normal `.git` layout. Fixed-text error assertions now pin the public error contract instead of presenting secret absence as operand-injection proof. |
| Missing sign-in/refusal coverage | Added foreign-host refusals, cancel after success, concurrent starts, slow_down continuation, and `SettingsAppSessions` through `SourceControlSignIn`, including persistence/reopen and actual stored-token secrecy. A Forgejo HTTP-refusal assertion is rejected because it contradicts inherited TypeScript behavior; HTTP policy is recorded below. |
| Windows ACL claim | Corrected. POSIX permissions are applied; Windows protection depends on host-provisioned ACLs and is unverified. |

The repository does not pin a glab binary version; discovery fixtures use
1.124.0. Its [tagged configuration source](https://github.com/gitlabhq/cli/blob/v1.124.0/internal/config/config.go#L231-L245)
checks environment values before per-host storage. The current [GitLab authentication documentation](https://docs.gitlab.com/cli/authentication/#token-precedence)
also explicitly permits environment tokens for `GITLAB_HOST`. Setting that variable
from client text therefore bypassed even the newer glab host restriction. No real
glab configuration, token or forge was accessed to verify this.

| Adapter | Credential destination audit against TypeScript |
| --- | --- |
| GitHub | At least as strict: per-host saved/app/CLI credentials, pinned-host checks, gh token variables removed, redirects disabled. Inherited: GH_TOKEN/GITHUB_TOKEN apply to github.com and any `*.ghe.com`; enterprise variables require matching GH_HOST. Unconfigured HTTPS hosts can still be contacted by access/star operations, as in TypeScript. |
| GitLab | The reviewed Rust expansion is fixed: destinations must be configured or come from an authorized checkout remote. TypeScript's repository-name API did not select a host from repository URL text. Rust now refuses foreign URLs before process construction. |
| Bitbucket | At least as strict: client/response URLs do not change the configured API origin (default api.bitbucket.org). Custom API endpoints are host-constructor inputs. Shared HTTP rejects foreign-origin URLs, pagination and redirects before sending credentials. |
| Azure DevOps | At least as strict: PR references become numeric IDs; lookup repository text becomes repository/project operands. Network destination comes from authorized cwd/CLI configuration through --detect=true, not input.host or a returned PR URL. |
| GitCafe | At least as strict: only git.cafe/staging.git.cafe clients and host-bound credentials; response pagination/redirect origins are checked. Staging does not silently acquire production-only settings tokens. |
| Forgejo/Gitea | At least as strict: an unambiguous configured fj/tea login must serve the remote/alias/mount; credentials attach to that login's base URL. Inherited: an explicit matching HTTP remote can select HTTP, and configured HTTP tea logins are supported. This round preserves that policy. |
| Cursor Origin | At least as strict: authentication comes from the explicitly configured CLI cwd; bearer requests use the constructor's API base (default api.cursor.com), restrict paths to v1/origin and do not follow redirects. PR/repository hosts do not change that base. |

Inherited clone policy is unchanged: the server must authorize destinations,
including UNC/device paths and symlink/reparse traversal on Windows; this crate
sets no `GIT_ALLOW_PROTOCOL`. Local/file clones and credentialed URLs persisted in
`.git/config` therefore need explicit host policy. Pending app sign-ins remain
shared by host among settings-write clients. These decisions and the atomic
settings blocker are recorded for the server integrator in the seam file.
