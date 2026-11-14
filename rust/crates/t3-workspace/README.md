# t3-workspace

Native tokio services; not a wasm dependency. All filesystem state is rooted in
directories supplied by the caller. This crate never opens an application database.

## API

- `WorkspaceFileSystem::{read_file, write_file, get_metadata}` consume/return
  `t3-contracts` types. Pass the home directory, `InternalPaths`, a fresh
  `WorkspaceRoots` provider and shared `SearchIndexes`. The provider loads current
  authorization for each operation, including reads and metadata.
- `KnownWorkspaces::from_project_snapshot` validates managed/project roots and
  thread worktrees against real Git listings. `new` accepts already authorized
  roots. Neither accepts filesystem roots as grants.
- `WorkspaceEntries::{browse, list, search, search_contents, list_ignored_files}`
  implement host directory completion, workspace trees, mention/palette search
  and grep. Ignored-file queries trim and validate at 256 UTF-16 units, then
  filter paths case-insensitively before the 200-file cap. Share its indexes with
  `WorkspaceFileSystem` so new files refresh existing indexes. `SearchIndex::watch` returns an owned subscription.
- `SearchIndexes` retains separate path/content indexes for 15 minutes after
  access; `evict_idle` also allows a server timer to reclaim idle indexes without
  another search. Initialization uses a separate cell per key, with at most 16
  cached or initializing indexes. Active subscriptions pin their indexes.
  Dropping indexes stops their watchers off the async executor.
- `ProjectFiles` validates roots and reads project records supplied as plain
  `Project` data. `load_project_file`/`parse_project_file` decode JSON with comments
  and trailing commas through `t3-contracts::t3_project_file`.
- `FaviconResolver` implements explicit override, t3.json, known-path and source
  metadata precedence, positive/negative TTLs and invalidation.
- `ScriptRunner::run_for_thread` executes setup/settle scripts using
  `t3-terminal`, with link fencing, runtime environment, random completion
  markers, shell readiness, output callbacks and completion receivers.
  Supply scripts after `resolve_scripts` applies settings inheritance. Keep the
  runner alive for its background observers; dropping it cancels those observers.
- `restore::WorktreeRestore::restore` restores managed missing/leftover checkouts
  and direct scratch children, under the supplied shared `WorkspaceLeases`.
  Supply authoritative project roots, current/previous managed roots and the
  configured `<base>/scratch` root; callers still validate cwd before launch.
- `WorkspaceLeases` and `WorkspaceReservations` provide exclusive workspace
  operations and overlapping restore/start reservations with cancellation cleanup.
- `AssetSigner` requires at least 32 supplied bytes of the existing
  `asset-access-signing-key` secret. `AssetAccess::{issue_url, resolve}` implements
  scoped signed grants and descriptor-bound media identity checks.
  `ExternalAssets` is the seam for authoritative tool items, native app icons and
  forge URL narrowing; `NativeAppIcons` supplies the native icon implementation.
  File responses own an `OpenMediaFile`; transports must serve that descriptor.
- `AttachmentStore::{issue_upload, validate_upload, upload, delete_pending,
  persist_chat_attachments}` covers signed streaming uploads and legacy data URLs.
  `plan_claim` validates pending claims and returns paths for orchestration to
  move under its transaction/recovery protocol. `DocumentText` is the injected
  document-extraction service called after successful publication.

## Ported sources and compatibility

The specification is `apps/server/src/workspace/{WorkspacePaths,
WorkspaceFileSystem,WorkspaceEntries,WorkspaceSearchIndex,KnownWorkspaces,
workspaceLease}.ts`, `project/{T3ProjectFileLoader,ProjectFaviconResolver,
ProjectSetupScriptRunner}.ts`, `assets/{AssetSigner,AssetAccess,MediaFile,
NativeAppIconResolver,icns}.ts`, `attachmentStore.ts`, `attachmentPaths.ts`,
`imageMime.ts`, and the legacy attachment persistence function in `ws.ts`.
Shared project-script helpers, header MIME identification and image-dimension
parsing are included. Tests port their filesystem, parsing, security and lifecycle
behaviors using real temporary files, symlinks/junctions, Git, watchers and shells.

Attachment names remain `<id>.<extension>` directly under the supplied attachment
directory. Pending IDs, SHA-256 deterministic IDs, legacy image lookup order,
`.part` staging, `.txt` sidecars, one-hour partial cleanup and 24-hour pending
cleanup match TypeScript. Upload and asset payload formats are version 1 JSON,
base64url without padding, signed with HMAC-SHA256. Native icon cache files remain
`<providerStatusCacheDir>/native-app-icons/<sha256>.png`.

Path search/listing supplements fff with internal file and directory links,
without descending through linked trees. It applies nearest `.ignore` NFA rules
and each nested repository's Git rules, rejects external and `.git` targets,
corrects watcher-indexed link kinds, and scans sixteen directories at a time.
Concurrent scans coalesce. Watcher events invalidate links; ordinary file edits
do not. Unwatched/incomplete scans expire after two seconds. The first read waits
for completion; subsequent reads wait five milliseconds before using the last
finished scan. fff search, progress, watcher setup and refresh run off the async
executor. The existing content-search containment/budget policy remains in force.

Search depends on **`fff-search = 0.11.0`**, the published Rust core of the pinned
`@ff-labs/fff-node` **0.11.0**. Both artifacts identify upstream commit
`95fd777c2529fc7b4d7572dabff64cc07268f2c5`. The Node wrapper passes no frecency
or history database, uses Neovim mode, zero combo settings and default Git recency.
This crate uses those same ranking settings and file/directory/mixed/grep query
parsers. Path ranking stays in fff. Content matching uses bounded Rust regex over
validated file descriptors, because fff's grep reopens paths and caches mappings
outside the containment boundary. It caps candidate enumeration at 25,000 files,
files at 1 MiB, matches at 100 lines per file, and each search at 250 ms.
Source: [published core](https://crates.io/crates/fff-search/0.11.0),
[Node defaults](https://github.com/dmtrKovalenko/fff/blob/95fd777c2529fc7b4d7572dabff64cc07268f2c5/packages/fff-node/src/finder.ts),
[FFI options](https://github.com/dmtrKovalenko/fff/blob/95fd777c2529fc7b4d7572dabff64cc07268f2c5/crates/fff-c/src/lib.rs).

## Remaining integration and differences

- Project creation/update/deletion commands, persistence, clone progress tracking,
  enrichment publication and linked-environment behavior in `ProjectService.ts`,
  `ProjectMutation.ts`, `ProjectCloneTracker.ts`, `ProjectEnrichmentService.ts`
  remain orchestration responsibilities. Actual clone-to-directory is implemented
  by `sourceControl/SourceControlRepositoryService.ts`, outside this crate's port.
  `ManagedProjectFolders.ts`, `ScratchThreadPromotion.ts`, `RepositoryIdentityResolver.ts`,
  `WorktreeSetupTracker.ts` and agent-session import/scanning have not been ported
  here. Orchestration send/start and terminal-recovery adapters must bind the
  shared worktree restore service and propagate recreation failures.
- The document parsers behind `provider/DocumentText.ts` are not implemented;
  the caller must provide `DocumentText`. The attachment claim plan does not
  perform database commits, claim journaling or authorization of thread mutations.
- HTTP response construction in `project/http.ts`,
  `assets/sourceControlMediaResponse.ts` and the asset routes remains in transport
  and source-control crates. Authentication/RPC scopes, issuing-owner checks in
  MCP attachment tools, authoritative project favicon overrides and thread
  workspace lookup must be supplied by their owning layer, before minting URLs.
- Filesystem operations use capability-relative no-follow opens beneath a pinned
  root, validate final handle paths, and reject regular files with multiple hard
  links. This deliberately refuses ordinary hard-linked files too, closing the
  TypeScript hard-link write and state-file read gaps. Windows directory handles
  deny delete sharing during an operation. Linux validates `/proc/self/fd` paths;
  macOS uses `F_GETPATH`; other hosts fail closed.
- All read, list, browse, metadata and search services require current known-root
  authorization. Absolute text reads must remain in the requested workspace.
  TypeScript permits arbitrary local host reads and browsing. Explicit media
  grants retain their separate host-file authorization contract.
- Authorization is checked against the pinned root as well as the requested
  path. Signed workspace roots resolve without following newly introduced
  ancestor links, while preserving Windows short-name compatibility.
- Network paths, mapped network drives on Windows, and network-target links are
  refused. Windows also refuses streams, reserved device names and trailing dots
  or spaces. Real short names resolve before Git metadata and asset dependency
  checks. Upload IDs must parse before any alias lookup.
- Writes are limited to 10 MiB, sync a staging file, preserve existing file
  permissions, and atomically replace the destination. New Unix files use mode
  0600. Metadata accepts at most 1,024 paths. Icon reads are capped at 2 MiB and
  favicon source inspection at 1 MiB. These bounds strengthen TypeScript.
- Search order uses the exact core. Alphabetical tree/browse ordering uses ICU
  root collation; JavaScript uses the server's default locale. Rankings involving
  files with tied upstream scores retain upstream ordering. OS timestamps/Git
  state still influence fff's ranking in both implementations.
- Idle index reclamation runs on access or explicit `evict_idle`, rather than an
  automatic Effect scope timer. Unrelated index builds and saves do not share an
  initialization lock. Watch subscriptions retain 32 batches and report lag when
  a reader falls behind. Listing/classification and metadata reads are sequential
  filesystem work on a blocking worker rather than TypeScript's bounded parallel batches.
- Favicon probes preserve candidate precedence but run sequentially. Native icon
  fallback selection sorts resource filenames instead of relying on readdir order.
- Media revision strings use device, inode, size and mtime; TypeScript additionally
  includes ctime. Identity-bound access itself retains device/inode checks and
  holds the accepted descriptor. Image dimensions are bounded to the same 256 KiB.
- Asset token parsing rejects extra dot segments; the TypeScript asset reader
  ignores them. Attachment path normalization rejects leading parent traversal
  rather than letting an absolute path's root clamp it. Base64 validation rejects
  noncanonical discarded padding bits. These restrictions do not change valid
  existing stored paths or generated tokens.
- Script observers close successful terminals using `close_idle` immediately;
  TypeScript first waits up to one second for a prompt redraw. Startup partial-line
  callback flushing is not implemented. Output caps count Unicode scalars instead
  of JavaScript UTF-16 units, avoiding isolated surrogate strings.
- Windows tests execute PowerShell, Git Bash, real symlinks/junctions and NTFS
  8.3 aliases, a Node-generated media token, and deterministic path swaps.
  Network-target links are rejected without connecting to a share. Native macOS
  icon commands, APFS name behavior, cloud/dedup reparse files, native Unix FIFO
  behavior and fish/nushell execution remain unverified on this host.

## Public API changes from the reviewed version

- `AssetSigner::new` now returns `Result<Self>`.
- `ExternalAssets::validate_source_control` returns `Option<SourceControlMedia>`
  with authoritative `fetch_url` and `file_name`, replacing its boolean answer.
- `ResolvedAsset::File.file` is an `OpenMediaFile`, replacing
  `Option<OpenMediaFile>`; every successful file resolution owns a descriptor.
- `WorkspaceFileSystem.known` is `Arc<dyn WorkspaceRoots>` rather than a snapshot;
  `WorkspaceEntries` also requires this field. `WorkspaceRoots::load` and
  `WorkspaceRootsFuture` are new; async closures implement the provider trait.
- `SearchIndexes::new(InternalPaths)` replaces `Default`, and
  `SearchIndex::new` takes an additional `InternalPaths` argument.
- `WatchSubscription.receiver` is a bounded broadcast receiver, replacing an
  unbounded mpsc receiver. Consumers must handle `RecvError::Lagged` by refreshing.
- `paths::{validate_host_path, relative_to}` are new public helpers;
  `paths::canonicalize` now refuses invalid and network paths.
- New resource-bound constants are `filesystem::{WRITE_MAX_BYTES,
  METADATA_MAX_PATHS}`, `assets::{ICON_MAX_BYTES, TOOL_IMAGE_MAX_BYTES}` and
  `search::MAX_INDEXES`. Search/read/write signatures and wire/token formats stay
  unchanged, with service-boundary validation added.

No wire types were duplicated locally for migration to `t3-contracts`. Local
upload/asset token claims follow private server schemas, and service inputs,
claim plans and lifecycle guards are internal integration types.

## Checks

From `rust/`, with `CARGO_TARGET_DIR` set to
`C:/Users/Administrator/t3code-rust/rust/target/agents/t3-workspace`:

```text
cargo fmt -p t3-workspace
cargo clippy -p t3-workspace --all-targets -- -D warnings
cargo test -p t3-workspace
```
