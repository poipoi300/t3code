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
Forge operations/setup are injected through `ReviewHost`; PR title/body generation
is an input. `VcsDriverRegistry` and `resolve_vcs_kind` provide Git-only routing
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

Checkpoint containment deliberately differs from
`apps/server/src/vcs/GitVcsDriver.ts`: capture scans without traversing directory
links and removes excluded paths from its private index before staging. If the
scope contains unsafe links, staging uses a NUL-delimited list of safe files,
filtered by Git's own ignore rules. Ignored links are never named in add
pathspecs; changing ignore rules cannot expose a scanned unsafe link during add.
Cached deletions, sparse entries and embedded repositories retain Git semantics.
Windows exclusions match index paths case-insensitively, including case-only replacements.
Directory symlinks and reparse points (including junctions), external or dangling file
symlinks, and uninspectable paths cannot be followed outside the worktree. Safe
internal file symlinks retain Git's symlink semantics. A scope whose ancestor is
a link is rejected. Restore checks before invoking Git and again before writing;
clean checks before each pass. An unsafe link returns `GitError::UnsafeRestore`
without running the destructive command. This also protects tracked directories
replaced by junctions after capture. Tests exercise real Windows junctions and
directory/file symlinks; outside files remain present and byte-identical.
Restore ownership compares the canonical deepest existing ancestor plus missing
suffix components, with Windows case folding; missing scopes/owners cannot evade
overlap checks.

Disposable indexes and checkpoint plumbing disable index/ref/checkout hooks.
User commit hooks and configured clean/smudge filters remain intentional Git
behavior, subject to cancellation and the command's deadline. Commands default
to noninteractive askpass/credential-manager settings, `GIT_PAGER=cat`, true
editors, and `GIT_SSH_COMMAND=ssh -oBatchMode=yes`; explicit environment patches
can select another SSH implementation. This SSH default supersedes configured
`core.sshCommand`. Inherited repository/namespace/pathspec selectors and Git
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

Remaining parity limits: `git/GitManager.ts` settled/confirmed branch-PR lookup,
Git-state fingerprints, last-known PR fallback and exponential provider-lookup
failure caches, PR state-change subscriptions, hosting-provider metadata, all
toast follow-up cases, and the new-worktree rewritten-head reflog policy are not
fully ported. `ReviewHost` still needs a forge-service adapter; hosted forge
operations have not been integration-tested. `git/linkCreatedPullRequest.ts`,
`refreshPushedPullRequests.ts` and `vcs/VcsStatusBroadcaster.ts` need the server's
orchestration/project/subscription services. `detachStackFrame` is Effect-specific.
Worktree lifecycle still lacks t3.json/settings discovery, persisted branch-base
metadata, progress, macOS checkout caches and index refresh; submodules are
explicit. Native remote normalization lacks some host-specific rewrites in
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
