# t3-git

Native installed-Git plumbing on `t3-process`; no libgit2. `Git::new(cwd)` supports
repository detection, porcelain-v2 status, current branch, refs, checkpoint
capture/lookup/diff/restore/pruning and worktree add/list/remove. `execute` is the
bounded escape hatch for higher-level workflows. Environment patches merge;
command patches win. Inherited repository selectors are cleared before patches.
Git command concurrency is bounded to eight across handles. Every command with
no deadline or a deadline above 30 seconds bypasses that lane; explicit bypasses
are preserved. Background status fetches have their own four-process bound.
Windows commands always receive `core.longpaths=true`. Worktree removal has a
five-minute deadline and accepts both Git diagnostics for an already missing tree.

User operations ported from `apps/server/src/vcs/GitVcsDriverCore.ts`: `commit`
(subject/body and explicit staging), `commit_message` (supplied message, change
detection and checkout lease), `prepare_commit_context` (disposable index and
objects), push/pull/fetch, branch/ref create/switch/rename/delete, upstream/config
reads and serialized retrying writes, remote reuse, and worktree-base fetching.
Selected paths are validated NUL-free repository-relative literals (including
names beginning with `-`); partial commits refuse merge,
cherry-pick and revert state before resetting staging. Commit-all checks conflict
marker pairs. `CommitReporter` streams output and Trace2 hook events. Publishing
preserves comparison bases instead of overwriting differently named upstreams.

`status_details_local`, `status_details_remote`, `cached_local_status` and
`list_branches` expose client status, root-relative files, change totals,
upstream/default divergence, worktree locations, filtering and pagination using
`t3-contracts` results. Repository discovery caches positive/negative results for
ten minutes/one second; explicit discovery coalesces for five seconds. Branch
snapshots last two minutes, explicit refreshes coalesce for five seconds, failed
refreshes back off thirty seconds, and mutations invalidate even on failure.
Static default/remote reads last five minutes. Status fetches preserve FETCH_HEAD,
disable maintenance/prompts, wait at most five seconds without cancelling the
writer, and back off from thirty seconds to fifteen minutes. Untracked totals
obey the 5,000-file/256-MiB limits and failure backoff.

`review_diff_preview`, `review_branch_commits`, `review_diff_file_contents` and
`read_range_context` port the review queries in `GitVcsDriverCore.ts`: independent
metadata/patch budgets, disposable intent-to-add indexes, stable patch flags and
hashes, unborn/root/merge commits, shallow-parent rejection, binary/size checks
and realpath containment. Server callers must supply state/secrets directories
with `with_internal_paths` before file expansion.

`GitWorkflowService` (`GitManager` alias) coordinates supplied-message stacked
actions and PR resolution/creation/materialization/local or worktree checkout.
Forge operations are injected through `ReviewHost`; `WorkflowTextGenerator`
supplies missing commit messages and PR content. `WorktreeHost` supplies project
settings and setup execution. `VcsDriverRegistry` and `resolve_vcs_kind` provide Git-only routing
and ancestor `.t3code/vcs.json` discovery. The TypeScript registry registers no
non-Git driver; configured `jj` is unsupported there and here.

`execute_command` produces sanitized `CommandError` diagnostics with
`to_contract`; `execute_vcs` produces `VcsFailure` carrying contract `VcsError`
variants. Errors retain counts/fixed diagnoses, never argv or stderr. URLs in diagnostic
details and streamed commit output are redacted. Remote/refspec components are
validated before assembling fetch/push destinations; fetch-source APIs reject
destination refspecs. Config keys cannot become command options, and public
config writes and single-commit fetches invalidate status and repository metadata
caches. Checkout refresh resolves its target to a commit OID before merge/reset,
so revision input cannot become reset options.
Authentication/rate-limit/custom classifications, trusted diagnostic prefixes,
push rejection reasons and index/config/ref locks follow `vcs/VcsProcess.ts` and
`GitVcsDriverCore.ts`. Cancellation remains native `ProcessError::Cancelled`.

Checkpoint references are compatible with orchestration-v2: SHA-256 of the scope,
first 32 lowercase hex characters, base64url of that ASCII text, then
`refs/t3/orchestration-v2/checkpoints/<encoded>/ordinal/<ordinal>`. Capture uses a
private index in the common Git directory and preserves the user index. Commits
parent HEAD, not the previous checkpoint. Diff options use exact repository-root
literal paths even when the workspace is a subdirectory. Raw patches truncate
without a marker; aggregate query patches reserve marker space inside 10,000,000
bytes. Numstat overflows fail rather than yielding incomplete summaries.

`restore_checkpoint` is the underlying mutating store primitive.
`restore_checkpoint_isolated` additionally validates `RestoreSafety`, including
other active and archived threads, checkpoint scopes and eligible provider
sessions. Callers supply complete context and must hold the orchestration lease
and restore reservation while checking and restoring. Database-backed context,
leases, rollback/provider coordination and baseline capture idempotence remain
in the orchestration layer, outside this crate. Capture and disposable commit/
review operations own cancellation scopes: dropping the caller cancels the child
operation, which awaits process termination before deleting scratch files.
`drain_operations` awaits these finalizers after an aborted caller; the Tokio
runtime must remain alive until they finish. Git requests a one-second SIGTERM
grace through `t3-process`; Windows job termination remains immediate.

### Checkpoint filesystem boundary

Restore checks the paths Git selects and refuses unsafe junctions, Windows mount
points and other name-surrogate reparse points found at check time. It checks candidate
ancestors and directories Git will replace; true symlink and file ancestors are
left for Git to replace without following. Submodules remain non-recursive leaves.
Clean always checks selected paths' ancestors. Git for Windows 2.23.0 and newer
[unlink junctions without traversing them](https://github.com/git-for-windows/git/releases/tag/v2.23.0.windows.1),
so listed clean paths are leaves on recognized supported releases. Only those
listed paths and their ancestors are classified by Rust in fast clean mode;
non-junction name surrogates nested inside a listed directory are not inspected.
Older or unrecognized Windows builds retain the stricter selected-directory walk.
POSIX mount points are not detected. In particular a FUSE mount could be cleaned
through; callers must prohibit mounts in the scope or reject such restores.
Device-number checks alone would miss same-device bind mounts, and this Windows
implementation has no verified POSIX mount-table/namespace boundary.

A fresh clean preflight runs before mutation. Each clean pass is checked again;
a later safety refusal names the path and says when tracked files were restored.
These checks do not prevent a process from swapping a directory for a junction
after its check or while Git is running. Callers must stop or quiesce the thread's
own watchers, dev servers and other writers before restore, and hold that state
through restore, clean and index finalization. The orchestration seam must enforce
this; t3-git does not stop those processes. A post-write scan could only report
damage after it happened and could miss a link removed by the writer. Staging in
a scratch tree would still race when publishing into the live tree and would not
atomically preserve ignored files, submodules and the index. Neither is a substitute
for quiescence or OS-enforced isolation of the Git child.

Capture reads what `git add` reads. Git for Windows may read through a junction
during `add -A`, as the TypeScript driver does. Capture has no link scan, ignore
enumeration, explicit file list or link removal.

### Repository identity and configured commands

Repository pinning and checkpoint-wide fsmonitor suppression harden TypeScript's
behavior, which discovers repositories from the working directory.
Checkpoint operations and automatic status/branch reads pin a `RepositoryBinding`
(canonical top-level, per-worktree Git directory and common directory). Commands
use explicit `--git-dir` and `--work-tree`; discovery uses only `rev-parse` with
fsmonitor disabled. `GIT_COMMON_DIR` also pins the recorded common directory, so
linked-worktree commands do not re-resolve it through a mutable pointer.
Shared-ref/config reads may select the recorded common
directory while keeping the recorded worktree and its identity checks.
A changed discovery result, `.git` added or redirected between
the scope and top-level, or a changed `commondir` pointer refuses the operation and
names the changed path. Descendant submodules and nested worktrees remain leaves
of their containing scope and may have their own bindings.

Until integration supplies durable identities, the first protected use records
each scope in a process-lifetime cache, shared even across newly constructed Git
handles. This does not survive a server restart and trusts what exists at first
use. Project binding and thread worktree creation must call `bind_repository`
before starting agent processes, persist the result outside the checkout, and
install it with `with_repository_binding` before status/checkpoint work after a
restart. The external server/orchestration seam requests describe that requirement.

Every checkpoint command disables fsmonitor and hooks; status/branch reads disable
fsmonitor, and all produced diffs disable external diff and textconv helpers.
Capture's `add` may run clean/process filters; restore may run smudge/process
filters, and its worktree comparisons/index reset may run clean/process filters.
Status and untracked totals may also run clean/process filters. These preserve
Git/LFS behavior. The low-level `status` helper can also run `post-index-change`
when Git refreshes its index; detailed status suppresses optional index writes
with `GIT_OPTIONAL_LOCKS=0`. Clean itself runs no configured filter. Helpers come from
the pinned repository's configuration hierarchy, including user/system settings;
workspace attributes can select an already configured filter. Missing objects in
a partial clone may trigger Git's lazy fetch, including transport/credential
helpers. Noninteractive reads use no pager/editor or aliases; checkpoint hooks
remain disabled. User-initiated commit/network operations retain their documented
hooks and transport configuration.

Pinning does not make the real Git configuration trustworthy. An agent able to
edit it, an included config or participating user config can configure filters
that the server runs with the server's privileges:
this remains a security finding. Codex workspace-write protects Git metadata by
default, while full access or approved escalation can write it. Claude's protected
paths require approval in default/acceptEdits, classifier approval in auto, and are
denied in dontAsk; bypassPermissions allows them. Its optional shell sandbox denies
Git config writes, but native Windows shell commands are unsandboxed. These are
provider/version/policy dependent, not protections enforced by t3-git. Adapter and
primary-documentation evidence is recorded in the Round 3 report.

Restore ownership compares the canonical deepest existing ancestor plus missing
suffix components, with Windows case folding; missing scopes/owners cannot evade
overlap checks.

Disposable indexes and checkpoint plumbing disable index/ref/checkout hooks.
User commit hooks and configured clean/smudge filters remain intentional Git
behavior, subject to cancellation and the command's deadline. Only background
status fetches and scoped fetches disable interactive credential prompts.
User-initiated network commands inherit transport/credential settings, and only
`LANGUAGE=C` is global; `LC_ALL=C` is scoped to the locale-sensitive reads/clean.
Inherited repository/namespace/pathspec selectors and Git
tracing are cleared; explicit patches still merge with command precedence,
including case-insensitive environment-key matching on Windows. Nested unborn
repository recovery is bounded by 64 candidates and per-command deadlines rather
than a shared five-second budget; its regression uses a real Git forwarding
fixture and Tokio's paused clock, without a global test mutex.

One deliberate Windows correction: when `git clean` repeatedly reports
`Removing ./` for an already emptied subdirectory, restore succeeds after checking
the directory is empty. The TypeScript four-pass loop rejects that case.
Numstat sorting uses ICU root collation rather than JavaScript's host-selected
locale. POSIX process groups, symlink/newline paths and the macOS fsync fallback
have not been executed on a POSIX host. Checkpoint ref mutation is restricted to `refs/t3/`;
ref listing defaults to the orchestration-v2 namespace and leaves legacy
`refs/t3/checkpoints/` refs alone. `prune_checkpoint_refs(scope_id, keep)` lists
and deletes only the namespace derived from that scope ID, including when `keep`
is empty. The former repository-wide one-argument form was removed; this is the
only library signature change. No downstream `t3-orchestration` or `t3-forge`
caller used it. Checkpoint writes/deletions use `--no-deref`, so symbolic refs
cannot redirect them into another scope or a user branch.

`GitApplicationService` exposes all ten VCS application RPCs without transport
logic. `VcsStatusBroadcaster` shares HEAD directory watches and remote pollers by
canonical checkout, sends initial snapshots, rejects stale branch/PR publication,
coalesces queued local reads and applies configured polling/background/auto-pull
policy. The default remote interval is 30 seconds; failed polls back off from
30 seconds to 15 minutes. Disabled fetching still performs an initial remote
read. Subscriptions cancel their final shared worker on drop; `close` and
`shutdown` also await finalizers, including subscription-owned background fetches.
`WorktreeSetupTracker` provides sliding setup snapshots, ordered stages, bounded
output, monotonic sequences, 30-second retention and cleanup-aware cancellation.
Launch producers and subscriptions must share its instance. `GitActionFollowups`
ports created-PR linking and pushed-project refresh routing over `GitActionHost`.
Multi-project hosts inject `GitWorkflowFactory` into the facade and broadcaster
so each checkout receives its own forge binding. The broadcaster also exposes
branch-change notifications for orchestration's worktree branch synchronization.
Server construction and owner requests are in the external server seam brief.

Ref listings reread HEAD even with cached refs, discard stale checkout locations,
and reject missing remote defaults and remote HEAD aliases. Untracked review
staging is capped at 256 MiB, smallest files first; available patches remain
visible while incomplete totals/file lists are omitted. Clone completion must
invalidate the shared Git handle's caches; both pre-init and unborn cases have
regressions. Worktree application creation records comparison-base metadata,
refreshes the index before returning and resolves submodule settings with
`t3.json` fallback (recursive by default, top-level or none when selected).

Remaining parity limits: `git/GitManager.ts` settled/confirmed branch-PR lookup,
Git-state fingerprints, last-known PR fallback and exponential provider-lookup
failure caches, PR state-change subscriptions, hosting-provider metadata, all
toast follow-up cases, and the new-worktree rewritten-head reflog policy are not
fully ported. A forge-service adapter has been added by the forge owner; hosted
forge operations have not been integration-tested here. Production construction
still needs the project/config, orchestration, background-policy and textgen
adapters described in the seam brief. `detachStackFrame` is Effect-specific.
`create_worktree_with_options` and `WorktreeCreationOptions` provide checkout and
submodule progress; `WorktreeSetupTracker::worktree_reporter` maps it to stages.
Worktree lifecycle still lacks macOS checkout caches. Native remote normalization
lacks some host-specific rewrites in
`sourceControlDefinitions.ts`. Process-boundary cause payloads are null, while
classifications/counts are retained. Rust scalar truncation differs from JS
UTF-16 slicing for non-BMP subject/progress text. Native-only driver options and
service contexts are local; wire results/errors reuse `t3-contracts`.
No stash action or standalone merge/rebase command/status API exists in the
specified TypeScript directories; none was invented. `git-filter-fixture` and
`git-gate-fixture` are only test subprocesses (a blocking filter and an I/O gate
forwarding to real Git). The ported tests run real Git in isolated temporary
repositories, including supplied messages, staging, merge protection,
push targets, fetch/pull, cached status/branches, review budgets/files and aborts.

Review hardening (2026-11-14): working-tree file expansion fails closed until
`with_internal_paths(state_dir, secrets_dir)` is configured. Construct one root
Git handle with the server environment and internal paths and derive checkout
handles with `.at()` so locks, caches and invalidation are shared. On Windows,
scratch-index and checkpoint commands disable hooks through a private empty
temporary directory retained by that root handle, under the configured state
directory (otherwise the process temp directory); this hardens the TypeScript
`/dev/null` convention. User commit hooks still run. Windows containment reads
reparse tags directly through
`FSCTL_GET_REPARSE_POINT`. Action subscriptions retain at most 128 events, evicting
old hook output first while preserving terminal events; this bound is stricter
than TypeScript's currently unbounded callback stream.

Automatic status and branch reads pass `-c core.fsmonitor=false`, including
repository metadata, background fetches and scratch-index reads for untracked
change totals. This is deliberate hardening beyond TypeScript. Their diff and
numstat commands also pass `--no-ext-diff --no-textconv`; piped noninteractive
reads do not start `core.pager`. Git clean/process filters retain the existing
Git/LFS behavior, including when status compares file contents.
