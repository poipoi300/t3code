# t3-project review round — M1/M2/L5

Source: C:\Users\Administrator\t3code-rust\rust\crates\t3-project.
The previous dependency-blocked hand-off is superseded. No subagents or commits.

Implemented and observed red-to-green: container refusal before project commit;
linked-session branch/worktreePath/runtime cwd; project-ID warning on settings
cleanup failure; credential stripping before identity/enrichment publication and
setup progress/error publication. Native t3-git container probe and its real
repository/Windows junction matrix were added under explicit coordinator permission;
no safety.rs/checkpoint.rs edits. Other siblings are read-only.

The server already supplies native ManagedFolderIo as HostFolders; no duplicate
backend was added. Tests pin its Windows junction predicate and exercise managed
cleanup through the temp fixture backend. New nested-junction assertions ensure
the std::fs::remove_dir_all primitive preserves the outside sentinel.

Server/owner actions are in briefs/server-seam-requests.md, section
"t3-project review follow-up — M1/M2/L5": inject the single environment Git via
ProjectService::new_with_git / RepositoryIdentityResolver::with_git; automatic
startup must call bootstrap_automatic and skip only None; Git setup owner must
redact its independent setup tracker before clients subscribe. RecentSession
Importable now boxes SessionThread (Preview owner's Clippy request).

Native SessionInventory still needs the workspace owner's public descriptor-
bound discovery/streaming/restat outside registered roots; scoped.rs remains
private. Import runtime repository and native-history locking/policy need the
persistence/provider-runtime and orchestration 05/04 owners. Discovery budgets,
zstd decompression and selection remain project-owned once those seams exist.
No real provider home/live T3 directory or network remote was used.

Checks use D:\t3code-rust-target\project, TWO configured jobs and the configured
rustc-gate wrapper, with 30-minute cargo deadlines and no bypass. Final scoped
format (including --check), exact all-target Clippy with -D warnings and all 50
project tests pass, including the exact-value, legacy-root and nested-junction
assertions. The final Git container matrix also passes (one test). Every cargo
run is finished; no background work remains. See README for coverage/differences
and the seam file for the next server/Git-owner actions.
No report should claim the prior external compilation errors are still blockers.
