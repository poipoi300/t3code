# t3-host

Native transport-neutral host services; does not target wasm. Constructors take
explicit isolated paths. No constructor discovers or opens the live T3 home.
Each service has its own module; process execution, contracts, storage, Git,
provider secrets, attachment primitives and guest signing retain their existing
owners. No sibling crate or workspace manifest was edited.

## File-by-file coverage

Inventory was performed before implementation. A contract name or unsupported
handler was not counted as an implementation. Paths are relative to
`apps/server/src/`. "Ported" describes the service implementation; transport and
composition wiring are separate work owned by `t3-server`.

| TypeScript file | Rust home | Status |
| --- | --- | --- |
| storageCleanup.ts | t3-host::storage_cleanup | Ported retention, worktree and scratch safeguards, fail-closed safety reads, reports, automatic worker and local dry run; differences below |
| attachmentStore.ts | t3-workspace::attachments; t3-config startup sweep | Existing owner; document extraction is injected; no duplicate |
| imageMime.ts | t3-workspace::attachments | Existing owner; no duplicate |
| zipArchive.ts | t3-host::zip_archive | Ported with explicit resource limits and additional refusals |
| policy/ManagedPolicy.ts | t3-config::policy + t3-host::policy | File decode/fingerprint reused; host enforcement ported; native policy sources remain incomplete in config |
| policy/managedLimits.ts | t3-host::policy | Ported bind/auth/connect/runtime/capability gates |
| policy/managedSettings.ts | t3-host::policy | Ported defaults, locks, write vetting, stored-value preservation and summary |
| policy/managedProviders.ts | t3-provider::policy | Existing owner, directly reused |
| policy/managedUpdates.ts | t3-host::policy | Ported update/channel/mirror gates |
| policy/policyTemplates.ts | t3-host::policy_templates | Ported ADMX, ADML and macOS profile generation; TS byte fixtures |
| guest/GuestShareService.ts | t3-cloud::guest + t3-host::guest | Existing signing/create/redeem/check reused; content, media, history, output, upload submission and sent-reply tracking ported; adapters required |
| guest/guestAttachments.ts | t3-cloud::guest + t3-host::guest | Existing classification/signatures reused; host media and file inspection ported; storage remains attachment owner's responsibility |
| guest/guestView.ts | t3-host::guest | Ported model/share projections |
| guest/http.ts | t3-server transport owner | HTTP cookie, multipart and SSE adapter not ported here; host methods provided |
| triage/TriageService.ts | t3-host::triage | Ported preparation, refs, scoped clones, cache publication and stale staging cleanup |
| triage/triageSession.ts | t3-host::triage | Ported sessions/context/paths/server description |
| triage/triagePrompt.ts | t3-host::triage | Ported exact context/seed/playbook text |
| background/BackgroundPolicy.ts | t3-host::background | Ported client leases, disconnect/expiry, profiles, scope and opportunistic gates |
| background/HostPowerMonitor.ts | t3-host::background | Ported snapshots, timestamp rejection and meaningful changes |
| background/KeepAwake.ts | t3-host::keep_awake | Commands, grace, owned process lifetime and retry ported; active-run/settings feed belongs to composition |
| serviceLauncher.ts | t3-host::service_launcher | Definitions, env/state/version codecs and cancellation-safe pre-trial backup ported; full supervisor/rollback blocked on native launcher IPC |
| os-jank.ts | t3-config::home/environment | Existing home/path resolution; shell and registry PATH hydration remains unported |
| settingsSecrets.ts | t3-config::secrets | Existing slot storage, redaction and rollback owner |
| secrets/SecretResolver.ts | t3-provider::secrets | Existing resolver/cache/1Password owner |
| secrets/secretProviderDefinitions.ts | t3-provider::secrets | Existing provider definitions/1Password owner |
| secrets/settingsSecretValue.ts | t3-host::secrets | Ported fail-closed consumers using existing resolver |
| secrets/SecretCheck.ts | t3-host::secrets | Ported fresh check and same-read registry refresh |
| secrets/SecretRequests.ts | t3-host::secret_requests | Ported project-bound one-use refs, delivery, recording rollback, startup/hourly expiry worker |
| environmentTheme.ts | t3-host::environment_theme | Ported bounded guarded reader and scoped filesystem watcher |
| worktreesDirectory.ts | t3-config::home + t3-host::worktrees | Existing expansion reused; managed-location and cleanup validation ported |
| desktopUpdate/DesktopUpdateMarker.ts | t3-host::desktop_update | Ported read-only marker TTL |
| desktopUpdate/DesktopAppUpdate.ts | t3-host::desktop_update | Ported run/progress/commit state machine; desktop control adapter required |
| review/ReviewService.ts | t3-git + t3-host::review | Existing queries reused; current-workspace authorization boundary ported |
| persistence/DatabaseStorage.ts | t3-host::database_storage | Ported status, online vacuum, paced history scheduling and reclaim marker/restart service; platform disk/restart/prune ports required |
| persistence/DatabaseMaintenance.ts (reclaimFiles/runRequestedReclaim) | t3-host::database_storage | Marker/attempt/retry/failure lifecycle ported; actual exclusive offline repair needs persistence/composition adapter |
| cloud/npmRelease.ts | t3-host::npm_release | Ported native bounded npm/registry lookup, canonical metadata/errors, single-flight 32-entry/one-minute cache and two npm permits |
| cloud/selfUpdate.ts | t3-host::self_update | Ported update/progress/desktop commit, policy/migration gates and retained handoff; native launcher client port required |
| cloud/pinnedRuntime.ts | t3-host::pinned_runtime | Ported download/checksum/safe extraction/preflight/publication and fail-closed startup pruning; limits below |
| cloud/serviceLauncherClient.ts | t3-host::self_update::LauncherClient | Required typed native IPC port; no fake managed capability when disconnected; transport/startup context adapter remains with server/process owner |
| process/externalLauncher.ts (editor path), packages/shared/src/editor.ts | t3-host::editor | Ported discovery, bundle/install candidates, positioned arguments, guarded launch/reveal plans; detached launch port required from process owner; canonical root guard intentionally stricter |
| serverOwnership.ts, serverRuntimeState.ts | t3-config::runtime + t3-server composition | Existing owners; not duplicated |
| serverSettings.ts, keybindings.ts | t3-config | Existing owners; not duplicated |

## Handler ledger and API

Rows are from `briefs/server-plan.md`. These methods are callable services; this
crate does not register transports or add a second ownership/startup path.

| Ledger row | One service call |
| --- | --- |
| server.reportClientActivity (ws.ts:2689) | BackgroundPolicy::report_client_activity(session, rpc_client, input) |
| server.runStorageCleanup (ws.ts:2642) | StorageCleanup::run_now() |
| server.getStorageCleanupReport stream (ws.ts:2643) | StorageCleanup::reports() (initial optional report, then each run) |
| server.prepareTriage (ws.ts:2687) | TriageService::prepare(input) |
| server.checkSecretSource (ws.ts:2670) | SecretCheck::check(reference) |
| secrets.answerRequest (ws.ts:2303) | SecretRequests::answer(input) |
| guestShare.create (ws.ts:2299) | Existing GuestSharing::create(input), reexported from guest |
| server.getDatabaseStorage | DatabaseStorage::status() (initial optional status, then watch changes) |
| server.reclaimDatabaseSpace | DatabaseStorage::request_reclaim() |
| server.getNpmRelease | NpmRelease::get(input.spec) |
| server.updateServer | ServerSelfUpdate::update(input) |
| server.updateServerWithProgress | ServerSelfUpdate::update_with_progress(input) |
| shell.openInEditor | ExternalLauncher::launch_editor(input) |
| POST /api/guest/redeem | Existing GuestSharing::redeem(request) |
| POST /api/guest/messages | GuestShareContent::submit(session, request, uploads, store) |
| GET /api/guest/session | GuestShareContent::session(session) |
| GET /api/guest/stream | GuestShareContent::stream(session) |
| GET /api/guest/render | GuestShareContent::render(session, item) |
| GET /api/guest/item | GuestShareContent::item(session, item) |
| GET /api/guest/history | GuestShareContent::history(session, cursor) |
| GET /api/guest/command-output | GuestShareContent::command_output(session, item) |
| GET /api/guest/attachment | GuestShareContent::attachment(session, id) |
| GET /api/guest/media | GuestShareContent::media(session, path) |
| review.getBranchCommits | ReviewService::get_branch_commits(input) |
| review.getDiffPreview | ReviewService::get_diff_preview(input) |
| review.getDiffFileContents | ReviewService::get_diff_file_contents(input) |
| server.commitDesktopUpdate | ServerSelfUpdate::commit_desktop_update(request_id) |
| subscribeServerConfig theme source | EnvironmentThemes::stream_changes() |
| attachments.createUploadUrl, attachments.delete, assets.persistChatAttachments, HTTP upload | Existing t3-workspace::AttachmentStore methods; not duplicated |

Additional APIs: cleanup `preview`, `run_automatic`, `start`,
`request_automatic_sweep`, `latest_report`, `drain`; background `snapshot`,
`remove_rpc_client`, `should_run_scope_work`, `should_run_opportunistic_work`;
power `snapshot/report/stream_changes`; managed-policy loading, overlays, summary,
reachability, Tailscale route closure and update gates; `SecretRequests` ref,
consume and file helpers; `TriageService::write_session/build_context/drain`;
archive `open/new/read_entry`; theme `current/start`; desktop marker/run;
service-definition renderers and state/environment helpers.

Construction and per-method host requirements for all thirteen newly requested
RPCs are in `C:\Users\Administrator\t3code-rust-notes\briefs\server-seam-requests.md`,
section "t3-host: thirteen unserved RPC seams". All new update/npm tests inject
HTTP hosts and cannot access the public network.

## Composition contracts and remaining ownership

- `CleanupHost` must supply current settings/projections including deleted and
  archived owners, live providers, starting/running terminals, pending deletion
  effects, project roots, managed roots and the SAME workspace lease used for
  provider startup/checkpoint restore (`WorkspaceLocks::workspace_leases()` in
  t3-orchestration, not the separate checkpoint snapshot mutex). Its `storage_cleanup_settings` method
  must read only settings, without loading projections, before expired file
  deletion. Directory scanning/deletion runs on the blocking pool. Cleanup never deletes projection rows:
  the TS source retains branch/worktree paths after removing a checkout.
  Call `request_automatic_sweep` on source-equivalent activity/settings events.
- `SecretRefresh` must rebuild provider state from the exact freshly resolved
  value. `RequestCards` dispatches metadata-only commands; no secret/ref enters
  event payloads. Retain `SecretExpiryWorker`, `CleanupWorker` and `ThemeWatch`
  handles for server lifetime; dropping them cancels their owned tasks.
- `GuestThreads::stream` remains the existing cloud/orchestration seam. It MUST
  produce the guest view, bound snapshots to 600 rows / 20 user turns, subscribe
  without a snapshot/live gap, coalesce live events, and terminate on share
  replacement/closure. Command output checks access when opened and every
  30 seconds, independently of frame rate. Guest image parsing runs outside the
  shared reply-cache mutex; an edited reply is rechecked before authorizing media.
  `GuestContent` supplies existing wire/detail projectors, timeline/fork queries,
  HTML render extraction and the shared Markdown parser/host path decoder.
  `GuestUploadStore` reuses the attachment owner's validated IDs/paths, bounds
  copying to the inspected length, and cleans a partial copy on cancellation.
  Guest submission must atomically recheck enabled/host/expiry/share identity and
  pending message/file limits in the command decider. Returned files roll back
  automatically if a later copy or dispatch fails or is cancelled.
- `ReviewWorkspaces` checks current canonical workspace roots. Desktop control,
  host-power events and keep-awake wanted activity/settings feeds need adapters.
  Managed policy enforcement must run BEFORE any listener/Connect exposure.
- The pre-trial backup/cancellation transition is implemented and tested, including
  no trial and preserved pending state after a stop. Full OS launcher supervision
  and rollback still need the native launcher
  entrypoint and a process-owner IPC facility equivalent to the TS extra Node
  IPC channel. `t3-process` currently exposes stdin/stdout/stderr only. Definitions
  are generated and tested; no service was installed or controlled on this host.
  Shell/PATH hydration also remains unported. These are explicit incomplete parts.
- Database composition must implement available disk space/device identity,
  supervised restart, ProjectionMaintenance with the supplied pacing callback,
  and OfflineRepair through the persistence owner's exclusive connection. Until
  offline repair exists, advertise restart_available=false: status remains useful
  and repair fails closed with the manual doctor command.
- Update construction reuses the shared managed policy, desktop control,
  persistence database, installer and launcher client. Desktop checked track
  switches forward both channel and target version; unsupported adapters refuse.
  The RPC facade keeps preparation and commit alive after caller cancellation.
  Startup runtime pruning needs the installed unit executable and authoritative
  process enumeration; unknown state/processes preserve completed runtimes.
- Editor config and launch use the same discovery service and authoritative
  workspace-root and detached-launch ports. `t3-process` has only owned kill-on-drop
  children today; a proper detach operation is required before native editor launch
  can be wired without closing the editor on server restart. Client editor IDs, paths and optional project roots are
  checked before launch; executable paths/base args come exclusively from discovery.
- Native registry/macOS managed-preference policy sources belong with the
  existing `t3-config` loader, whose current implementation loads files only.
  No sibling edit was made to fill that gap.

## Behavior differences and verification limits

1. Cleanup offers a local-only `preview`; the TS RPC only runs cleanup. Symlinks
   and junctions at destructive roots are refused, nested links are not followed,
   and capability-relative deletion protects the state boundary. Network paths
   are additionally refused. Full final thread/settings equality can retain a
   checkout after unrelated changes; TS compares only relevant fields. Orphan
   report ordering and some refusal text differ. Most Git inspection commands
   use a 1 MiB/30s bound rather than each TS operation's individual byte/time
   limits; hidden index enumeration uses a 64 KiB retained buffer, callbacks and
   batch hashing with the same 10,000 flagged-file limit. A single index line
   over 1 MiB is additionally refused. Reports retain at most 200 priority-sorted
   entries. Measurement caps are 2 million entries and 30 seconds. Safety snapshot,
   scheduled-task and workspace-lease waits additionally refuse after 30 seconds.
2. ZIP requires explicit compressed/member/count/expanded limits and returns a
   bounded member buffer instead of a Node stream. It additionally refuses
   duplicate names, encryption, symlinks/special entries, Windows drive/colon
   paths and unsupported compression. Original helper delegated extraction
   budgets to its installer callers; this API makes those budgets explicit.
3. Guest media/uploads additionally refuse symlink/nonregular files. Token
   issuance, HMAC and constant-time comparison are reused from t3-cloud, not
   reimplemented. Decoding verified claims is needed because its public check
   returns unit; unexpanded compact IDs are refused. New guest service tests use
   real signed sessions and private disk key storage. Snapshot/projection/parser
   semantics still depend on the adapters described above.
4. Secret file publication uses ref-named `.partial` staging with no-clobber
   final publication. The expiry sweep also recognizes legacy `.tmp` leftovers
   in its dedicated plaintext-files directory. Failed per-file removals do not
   stop remaining expiry work; TS already continues ref-removal failures, but
   its plaintext-file loop stops on a removal failure. This is deliberate hardening. Unix 0700/0600 protection matches the source;
   Windows uses the source's inherited ACL behavior. Errors carry static reason
   codes or tagged managed-policy/settings-secret data, never OS/provider causes
   or supplied secret values. Diagnostics omit
   source logging/telemetry spans. Settings/ref wire types come from contracts;
   persisted JSON uses contracts::stored serializers.
5. Triage stale staging removal fails closed if capability-safe removal fails;
   TS ignores that removal error. Source checkout identity is checked before a
   cache is trusted. Context/seed/playbook bytes match TS fixtures when given the
   same inputs; native callers provide the runtime-version fact explicitly.
6. Watch receivers coalesce intermediate states; cleanup reports use an
   unbounded subscriber channel. The self-update facade retains desktop commit
   after caller cancellation; callers bypassing it and invoking DesktopAppUpdate
   directly do not get that retention. Desktop/power native IPC needs its owner. Review maps Git/VCS errors to a redacted
   host error rather than preserving TS typed diagnostic details.
7. Service launchers additionally reject unsupported Windows argument quoting
   and control characters. Dotenv compatibility is covered for source-tested
   quoting/multiline/protected variables; it is a native parser, not Node's
   parseEnv. ServiceState/ServiceUpdate are local persisted launcher types and
   ManagedPolicyRefusedError is a local tagged service error to consider moving
   to t3-contracts; other local types are host adapters, not
   replacement wire contracts.
8. Rust strings cannot represent lone UTF-16 surrogates; date parsing has Chrono's
   range. Existing sibling behavior is not re-ported or certified here. POSIX
   native paths/inhibitors/permissions have not been executed on this Windows
   machine. No live service, live T3 database or public network clone was used.

9. Editor launch additionally requires existing absolute paths within canonical
   discovered workspace roots, rejects parent traversal/network paths/control
   characters, and checks the optional project root contains the target. This is
   stricter than TS and required by the host-port task. Discovery uses a blocking
   60-second cache rather than stale-while-revalidate; detached launch is supplied by the process owner rather than faking Node unref
   with an owned kill-on-drop task. Line/column conversion uses bounded
   Rust integers. Native platform integrations were not exercised on macOS/Linux.
10. Runtime installs additionally cap downloads at 512 MiB, checksums at 1 MiB,
    expanded archives at 1 GiB/100,000 entries, reject links/special entries,
    duplicate files and unsafe paths, and bound preflight output to 1 MiB.
    Extraction is a blocking safe parser rather than a tar subprocess; a timed-out
    blocking filesystem operation can finish its current operation before stopping.
    Pruning additionally refuses junctions and only deletes `.pruned-` directories
    whose suffix is an exact service version. Orphan/prune ordering can differ.
11. npm additionally bounds registry bodies to 256 KiB and includes semaphore wait
    in the 15-second lookup bound. Migration metadata is limited to safe integer
    range. Native transport errors use sanitized reasons instead of raw causes.
    The native database service exposes explicit failures from manual maintenance;
    its periodic worker ignores them as TS does. Background worker handles must be
    retained by composition and dropped on shutdown. Disk-space reads additionally
    fail after 30 seconds; background repairs can take longer under their owner.

## Tests

Rust tests use isolated real Git worktrees, real directories/junctions/hardlinks,
real ZIP bytes and files, private disk secrets, and deterministic clock/service
seams. Generated service definitions, managed policy profiles and 12 triage text
cases pin TS bytes. Tests assert data, authorization refusals and lifecycle
transitions; no sleep polling is used. Test files correspond to source behavior:

| TypeScript tests | Rust tests |
| --- | --- |
| storageCleanup.test.ts | storage_cleanup.rs |
| BackgroundPolicy.test.ts, HostPowerMonitor.test.ts | background.rs |
| KeepAwake.test.ts | keep_awake.rs |
| GuestShareService.test.ts, guestView.test.ts, guestAttachments.test.ts | guest.rs plus existing t3-cloud tests; parser/transport cases await adapters |
| guest/http.test.ts | HTTP behavior remains t3-server's responsibility |
| ManagedPolicy.test.ts, managedLimits.test.ts, policyTemplates.test.ts | policy.rs; native policy source cases remain config-owned |
| triageSession.test.ts, triagePrompt.test.ts, TriageService.test.ts | service_triage.rs |
| SecretCheck.test.ts, SecretRequests.test.ts | secrets.rs |
| secretProviderDefinitions.test.ts | existing t3-provider owner |
| DesktopAppUpdate.test.ts, DesktopUpdateMarker.test.ts | desktop_update.rs |
| ReviewService.test.ts, worktreesDirectory.test.ts | review_worktrees.rs |
| serviceLauncher.test.ts | service_triage.rs (definitions, env/state/version and backup-stop cases) |
| persistence/DatabaseStorage.test.ts, DatabaseMaintenance.test.ts (requested repair) | database_storage.rs; actual offline SQL repair waits for its owner |
| cloud/npmRelease.test.ts | npm_release.rs |
| cloud/selfUpdate.test.ts, pinnedRuntime.test.ts | self_update.rs, pinned_runtime.rs; all HTTP is injected |
| process/externalLauncher.test.ts, shared/editor.test.ts | editor.rs |
| environmentTheme.test.ts | archive_theme.rs |
| attachmentStore.test.ts, imageMime.test.ts | existing t3-workspace owner |

## Independent review follow-up

Reviewed against the TypeScript source before each behavior fix. Regressions were
observed failing before their fixes. Details, red/green evidence and source line
references are in the external review-fix report at
`D:\t3code-rust-target\server-j\t3-host-review-fixes.md`.

| Finding | Disposition and regression |
| --- | --- |
| 1 Output access per frame | Fixed: 100 frames cause no extra access reads; revocation at 30 seconds |
| 2 Full snapshot per expired file / blocking walk | Fixed: one projection read, settings-only checks, blocking-pool scan/deletion |
| 3 Windows data-read open for measurement | Fixed: reuse metadata-only query; real locked file and hardlinks |
| 4 Preview remote calls and lease | Fixed: remote eligibility conservatively kept; refs, PR calls and lease unchanged |
| 5 Policy organisation/tag shape | Fixed reachability tag/action/detail and update organisation messages; rejected tagged-update claim because TS returns strings |
| 6 Unswept plaintext temporary files | Fixed ref.partial staging and legacy tmp recognition; production-name and orphan expiry tests |
| 7 One failed expiry deletion aborts sweep | Fixed both loops; bad ref and Windows-locked file tests; TS file-loop distinction noted above |
| 8 Windows path case fallback | Fixed case keys plus original/canonical spelling checks; missing terminal/provider/project cwd tests |
| 9 Orphan full clone skipped | Fixed second-level reporting while first-level checkout remains excluded |
| 10 Worktree report path order | Fixed projection insertion order, with live groups before deleted-only groups |
| 11 Linked cleanup root counted failed | Fixed kept outcome; outside target bytes remain untouched |
| 12 Settings-secret error shape/check failure | Fixed contract tag/provider label and failed-result behavior for backend errors |
| 13 Missing-path parent components | Fixed lexical normalisation before authorization; platform-neutral regression; native Unix not run |
| 14 Markdown under shared mutex | Fixed immutable reply cache with per-reply parse cache and post-parse edit check; lock/race and cache lifecycle tests |

Deletion tests additionally cover tracked-changes `git clean -ffdx`, settings/
thread/pending-deletion changes at lease acquisition, deleted ownership with idle
subagents, ancestor settlement and grace, integrated branch eligibility, exact-head
merged PR eligibility, and forced submodule removal with dirty files/unpushed
stored history protected. Nested registered orphan worktrees and guest cache pin/
eviction have focused coverage too. No live service or live T3 data is used.

## Current verification

Current commands run from `rust/`, with `CARGO_TARGET_DIR` set to
`D:\t3code-rust-target\host` and the existing two-job cargo config unchanged.
`cargo fmt -p t3-host` passed. `cargo test -p t3-host` passed **101 tests**,
zero failed or ignored, across 16 test binaries (plus zero doc tests). Package
Clippy with `--no-deps -- -D warnings` passed. Exact Clippy without `--no-deps`
remains blocked by **57 existing result_large_err errors in t3-codex-app-server**.
No warning was suppressed, no wrapper bypass was needed, and no sibling was fixed.

The fourteen prior
review findings remain closed; their full regression table is above. New drift
regressions caught scratch refusal, the lost-git-link failure outcome and an
unbounded safety read before fixes. A runtime-publication race test also failed
before its fix: another process's completed runtime is now validated and retained.
No new local wire types were introduced; launcher persisted types and the local
managed-policy error retain the prior relocation note. No commit was made.

Logs: `D:\t3code-rust-target\host\t3-host-tests.log`,
`t3-host-own-clippy.log`, and `t3-host-clippy.log`.
