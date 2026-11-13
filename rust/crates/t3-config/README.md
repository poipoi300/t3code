# t3-config

Native, transport-neutral configuration and T3 home files. This crate does not
target wasm. It uses `t3-contracts` for settings, provider instances, bootstrap
envelopes, keybindings and configuration issues, and `t3-persistence` for SQLite
access and ownership. It does not start a server or discover the user's home.

## Public API and integration

- `config::resolve_config(&ConfigInputs)` is pure. Supply the account home,
  process cwd, environment map, flags, decoded desktop bootstrap, service mode,
  persisted observability endpoints and optional automatic-port selection.
  `needs_automatic_port` identifies the web automatic-selection path. Socket
  probing, including WSL Windows listeners, belongs to the caller. Resolve again
  with the selected port. `ResolvedConfig::ensure_directories()` creates the
  directories and sweeps expired pending attachments. Tokens have an explicit
  accessor and the resolved configuration deliberately has no `Debug` derive.
- `home::HomeLayout::derive` exposes the paths below. `resolve_dev_home` takes
  the dev runner's `--home-dir`, a detected worktree home and ambient home.
  `environment::resolve_worktree_home` detects linked worktrees from `.git`
  files without invoking Git, resolving relative cwd inputs against the process
  cwd first. `resolve_worktrees_directory` expands the setting
  and refuses relative paths and filesystem roots.
- `SettingsService::{new,with_database}` returns an `Arc` service.
  `reload`, `update`, `update_provider_instance`, and `with_snapshot` share one
  write lock. `snapshot` reads a separate cache, including inside a `with_snapshot`
  callback or during a save. First use initializes that cache through a shared
  async cell; a failed initialization can be retried. Pass the already-open
  `Database` to `with_database` for legacy project and provider-history migration;
  the constructor never opens a
  second database. Mutations reread the file before applying their patch; a failed
  reload/save leaves the previous cache available.
- `KeybindingsService` exposes `snapshot`, `reload`, `sync_defaults`, `upsert`,
  and `remove`. `default_bindings`, `parse_shortcut`, `parse_when`, `compile_rule`
  and the encoders are also available. The 88 defaults retain source order;
  custom commands replace that command's defaults and the last 256 rules win.
  Snapshots use a separate cache as settings do; default backfill uses a single
  validated custom-rule snapshot.
- Both services expose `subscribe_changes` and `start`. Acquire a subscription
  before a snapshot to buffer subsequent changes. Subscriptions are unbounded
  Tokio channels, as the TypeScript PubSubs are. Keep the returned `WatchHandle`
  alive; dropping it stops that watcher. Start one watcher per service. Returning
  from `start` is the readiness barrier. Settings also watch and follow symlink
  target directories. Both watchers filter file events and debounce 100 ms.
  An invalid external settings edit emits defaults with automatic thread
  continuation disabled; an invalid keybindings edit emits defaults plus issues.
  Neither overwrites the invalid file, and mutations refuse malformed files.
  Reads use lossy UTF-8 decoding and strip one leading UTF-8 BOM, matching Effect's
  `TextDecoder`. UTF-16 files normally produce parse failures and safe defaults;
  actual filesystem errors propagate. Watch filters canonicalize parent paths,
  including when an event removes a file under a symlinked home.
- `secrets::SecretStore` reads/writes the existing raw `.bin` format with atomic
  publication and owner-only POSIX permissions. Settings migrate inline field
  credentials, materialize saved secrets, retain redacted values, remove stale
  slots and roll secret changes back if materialization or file commit fails.
  `redact_for_client` shapes settings for client capabilities. References stay
  references; this crate does not call external secret-provider drivers.
- `runtime::{read_runtime_state,read_live_runtime_state,persist_runtime_state,
  clear_runtime_state,runtime_origin}` handle discovery records.
  `RuntimeState::for_config` takes a supplied ISO start time and server version.
  `ServerOwner::acquire` holds **the persistence crate's** `OwnershipLock` for its
  lifetime, checks live pre-lock records with PID-reuse detection, publishes an
  owner UUID and only clears its own record on drop. Every five seconds it stats
  the lock's file identity; `ownership_health` exposes warnings or loss. It never
  deletes or recreates an acquired SQLite lock. Unix identity checks use stat,
  without opening/closing an extra SQLite-inode descriptor.
  Acquire `ServerOwner` before opening/migrating the database, then pass
  `migration_owner_directory: None` to `Database::open_with_options`/`open_v2`.
  The server already holds the same ownership lock that CLI migrations acquire;
  requesting a second migration lock would reject its own server.
- `policy::{managed_policy_path,selected_policy_path,decode}` and
  `ManagedPolicy::load_file` implement strict file decoding; absence means no
  policy and malformed/unknown properties fail. `loaded_policy` returns the
  12-hex SHA-256 fingerprint of the encoded document, retaining record insertion
  order. Only development builds honor `T3CODE_DEV_MANAGED_POLICY_PATH`.

## Resolution precedence

First present wins, including `false` and port zero. Environment values are
validated even when a higher-priority flag overrides them.

| Option | Highest to lowest precedence; default |
| --- | --- |
| mode | flag, `T3CODE_MODE`, desktop bootstrap; `web` |
| port | flag, `T3CODE_PORT`, bootstrap; desktop 3773, web automatic selection preferring 3773 |
| base directory | first-present `--base-dir` / `T3CODE_HOME`, then discard that selection if blank; bootstrap `t3Home`; `<account home>/.t3` |
| dev runner home | nonblank `--home-dir`, linked worktree `<root>/.t3`, nonblank `T3CODE_HOME`; default home |
| cwd | trimmed, home-expanded positional argument; supplied process cwd |
| dev URL | flag, `VITE_DEV_SERVER_URL`; absent |
| host | flag, `T3CODE_HOST`, bootstrap; private-network `0.0.0.0`, desktop `127.0.0.1`, web unspecified |
| LAN | flag, `T3CODE_LAN`; service-managed mode. Only web without an explicit host becomes private-network-only |
| no browser | headless forces true; flag, `T3CODE_NO_BROWSER`, bootstrap; desktop true, web false |
| project bootstrap | forced option, headless false, flag, `T3CODE_AUTO_BOOTSTRAP_PROJECT_FROM_CWD`; web true |
| WS event logging | flag, `T3CODE_LOG_WS_EVENTS`; whether a dev URL is present |
| Tailscale Serve | flag, `T3CODE_TAILSCALE_SERVE`, bootstrap; false |
| Tailscale HTTPS port | flag, `T3CODE_TAILSCALE_SERVE_PORT`, bootstrap; 443 |
| bootstrap fd | flag, `T3CODE_BOOTSTRAP_FD`; absent; envelope reading belongs to CLI |
| dev auth | trimmed `T3CODE_DEV_AUTH_TOKEN`; absent/blank or at least 32 UTF-16 units, only read for web dev mode |
| dev origins | `T3CODE_DEV_ALLOWED_ORIGINS`, comma-separated and trimmed; empty |
| logging | CLI log level, `T3CODE_LOG_LEVEL`; Info |
| tracing | `T3CODE_TRACE_MIN_LEVEL` Info, `T3CODE_TRACE_TIMING_ENABLED` true, `T3CODE_TRACE_MAX_BYTES` 10 MiB, `T3CODE_TRACE_MAX_FILES` 10, `T3CODE_TRACE_BATCH_WINDOW_MS` 1000 |
| trace path | `T3CODE_TRACE_FILE`; layout's `logs/server.trace.ndjson` |
| OTLP export | `T3CODE_OTLP_PROTOCOL` http/json, `T3CODE_OTLP_HEADERS` absent, `T3CODE_OTLP_EXPORT_INTERVAL_MS` 10000 |
| OTLP endpoints | disabled SDK wins; `T3CODE_OTLP_{TRACES,METRICS,LOGS}_URL`, standard OTEL signal/generic endpoint, bootstrap URL, saved settings URL |
| SDK disable | `T3CODE_OTEL_SDK_DISABLED`, `OTEL_SDK_DISABLED`; false |

OTEL per-signal protocol/headers override generic values. A malformed claimed
OTEL signal blocks fallback export; exporter `none` blocks it too. A generic OTEL
endpoint gains `/v1/<signal>` without losing its query. `OTEL_RESOURCE_ATTRIBUTES`
is decoded separately. T3 headers retain whitespace-separated pairs and literal
equals signs in values. `persisted_observability` reads just observability, so an
unrelated invalid setting cannot drop those endpoints. `resolve_static_dir`
checks bundled `client/index.html` before monorepo `../../web/dist/index.html`.

## Home layout

Paths are relative to the base directory. `S` means `userdata`, or `dev` when a
dev URL exists without an explicit flag/environment home. A bootstrap-only home
does not count as explicit. The dev runner passes its selected worktree home as
explicit, so it uses `.t3/userdata`. Directories used by other services are exposed
without migrating or interpreting their contents. Dynamic leaf names remain
owned by their writer, except terminal-history encoding and secret slot names.

| Path | Format | Writer / owner |
| --- | --- | --- |
| `S/` | directory, best-effort 0700 on POSIX | configuration |
| `S/statev2.sqlite`, `-wal`, `-shm` | SQLite, WAL/shared-memory sidecars | t3-persistence |
| `S/state.sqlite` and sidecars | legacy SQLite import source | old server; imported by t3-persistence |
| `S/server-owner.sqlite` | lifetime exclusive SQLite transaction | t3-persistence OwnershipLock |
| `S/server-runtime.json` | version-1 compact JSON plus newline | ServerOwner / server lifecycle |
| `S/settings.json` | sparse pretty JSON plus newline; comments/trailing commas accepted on read | SettingsService or external editor |
| `S/keybindings.json` | pretty JSON array plus newline; comments/trailing commas accepted on read | KeybindingsService or external editor |
| `S/keybindings-migrations` | migration IDs, one per line, trailing newline | KeybindingsService |
| `S/secrets/<name>.bin` | raw bytes, 0600 on POSIX; not encrypted by this file store | SecretStore, auth and settings |
| `S/anonymous-id` | installation identity text | telemetry |
| `S/environment-id` | environment UUID/text plus newline | environment identity |
| `S/environment-id.recovery` | persistent recovery winner plus newline | environment identity |
| `S/themes/` | environment theme JSON files | environmentTheme |
| `S/attachments/` | stored uploads, pending uploads, `.part` files, text sidecars | attachmentStore / upload handlers |
| `S/browser-artifacts/` | browser screenshots and captures | MCP/browser services |
| `S/logs/server.trace.ndjson` and rotations | NDJSON trace records | serverLogger |
| `S/logs/provider/events.log` and rotations | provider event log | provider services |
| `S/logs/terminals/terminal_<base64url-thread>[_<base64url-terminal>].log` | terminal history text | terminal Manager |
| `S/logs/terminals/terminal_sha256.<hash>[_<hash>].log` | history for filenames over 143 bytes | terminal Manager |
| `S/logs/terminals/<legacy-safe-thread>.log` | legacy history text | old terminal Manager, read for migration |
| `S/logs/boot-service.log` | service/launcher text log | bootService / serviceLauncher |
| `S/logs/relay-client.ndjson` | relay event log | ManagedEndpointRuntime |
| `S/logs/server-child*.log*` | desktop backend text logs | desktop shell |
| `S/logs/server-<pid>-<timestamp>.heapsnapshot` | V8 heap snapshot | observability HeapSnapshot |
| `S/provider-processes/` | Linux process-identity records | process/providerProcesses |
| `S/model-manifest.json` | cached JSON model manifest | provider ModelManifest |
| `S/usage-model-rates.json` | cached model prices JSON | usage UsageService |
| `S/usage-scan-cache-v15.json` | versioned scan cache JSON | usage UsageService |
| `S/usage-scan-cache-v{5..14}.json`, `S/usage-scan-cache.json` | legacy scan caches | older UsageService; current writer removes after migration |
| `S/speech/models/` | downloaded speech model files | SpeechModelStore |
| `S/providers/codex/<instance>/shadow/` | managed Codex home | CodexManagedHome |
| `S/providers/antigravity/<key>/` | provider browser profile | antigravityAuthSupport |
| `S/antigravity-tmp/<key-prefix>/` | provider temporary runtime | antigravityAuthSupport |
| `S/server-browser/{profiles,launchers,downloads}/` | browser profiles, launchers and downloads | ServerBrowser |
| `S/device/bin/` | generated AgentDevice shim | AgentDeviceShim |
| `S/device/targets/<session>/` | generated launcher bindings | AgentDeviceShim |
| `S/device/hosts/<hash>.json` | host/device configuration JSON | AgentDeviceTarget |
| `S/device/agent-device/{hub.json,daemon.json,...}` | daemon state and diagnostics | LocalDeviceHost / AgentDevice |
| `S/device/eas/<host>.json` | device-session ledger JSON | EasDeviceHost |
| `S/device/simlock/<host>/` | device-session state | SimlockDeviceHost |
| `caches/` | provider discovery/status caches | provider services |
| `caches/worktree-checkouts/` | checkout cache trees | worktree checkout service |
| `caches/native-app-icons/` | extracted icon assets | NativeAppIconResolver |
| `caches/pull-requests/` | persisted PR read-cache records | PullRequestReadCache |
| `worktrees/` | default managed Git worktrees | worktree services |
| `projects/`, `scratch/` | managed project folders | ManagedProjectFolders |
| `tools/<tool>/<version>/` | managed executables/packages | device/browser/provider installation services |
| `usage-host-id` | host identity text | UsageService |
| `source/` | downloaded source checkout | triage |
| `service.env` | environment assignments, shell-style text | CLI service configuration |
| `runtime/service-state.json` | versioned service state JSON | serviceLauncher |
| `runtime/service-launcher.lock` | launcher ownership record | serviceLauncher |
| `runtime/.service-stopping` | stop marker | serviceLauncher |
| `runtime/.restart-pending` | deferred version/restart marker | bootService / launcher |
| `runtime/desktop-update-restart` | update restart marker; mtime carries expiry | desktop update handoff |
| `runtime/launchers` | newline-separated registered launcher paths | pinnedRuntime |
| `runtime/versions/<version>/{t3,t3.exe}` | platform executable | pinnedRuntime |
| `runtime/versions/<version>/.install-complete` | completed installation sentinel | pinnedRuntime |
| `runtime/db-backup/<update-id>/` | database snapshots and restore marker | serviceLauncher |

Atomic writes use a scoped `<filename>.*` sibling directory with `contents.tmp`;
cleanup removes it. Secret publication uses a temporary owner-only file and
rename/hard-link. There is no new persistent filename or alternate home format.

## Source coverage and remaining seams

Ported behavior, including supporting shared algorithms:

- `apps/server/src/config.ts`: home layout, directories and static client lookup.
- `apps/server/src/cli/config.ts`: flag/environment/bootstrap precedence and
  observability resolution; `binCli.ts` delegates configuration to that module.
- `scripts/dev-runner.ts` (home selection only) and
  `packages/shared/src/devHome.ts`.
- `apps/server/src/os-jank.ts` (path expansion/base resolution and pure HOME
  hydration), `worktreesDirectory.ts` (setting resolution only).
- `apps/server/src/serverSettings.ts`: file/cache/watch/update lifecycle,
  streaming/idle migrations, provider envelopes/history, project folding,
  source-control migration, sparse writes, secret rollback and text-generation
  fallback. `settingsSecrets.ts` and
  `sourceControl/sourceControlHostSecrets.ts`: persisted slot names, retention,
  redaction and reference compatibility.
- `packages/shared/src/serverSettings.ts`: patch operations, record removals,
  per-project replacements, model preferences, model selection options and
  worktree history. `backgroundActivitySettings.ts`: preset/custom normalization.
- `apps/server/src/keybindings.ts`, `packages/shared/src/keybindings.ts`:
  defaults, parsing, merging, normalized edits, startup backfill, migration
  marker, issue events and truncation.
- `apps/server/src/serverRuntimeState.ts`, `serverOwnership.ts` (ownership and
  monitoring), `legacyServerOwner.ts` (PID-reuse decision).
- `apps/server/src/environment/ServerEnvironment.ts` (identity persistence) and
  `ServerEnvironmentLabel.ts` (pure label/machine-info fallback).
- `apps/server/src/auth/ServerSecretStore.ts` (get/set/create/remove),
  `packages/shared/src/{atomicWrite,schemaJson,otelEnvironment}.ts`.
- `apps/server/src/policy/ManagedPolicy.ts` and
  `packages/shared/src/managedPolicyFile.ts` (file-path selection, strict JSON
  loader and fingerprint only).
- Path conventions from `attachmentPaths.ts`, `attachmentStore.ts` (startup
  pending sweep), `terminal/Manager.ts`, provider/device/usage/cloud services.

The caller still owns OS and product services: automatic port probes, bootstrap
fd transport, service-launcher liveness, the pre-0.1.0 boot-service compatibility
preflight in `serverOwnership.ts`, shell/registry PATH hydration in `os-jank.ts`,
the runtime descriptor/machine probes and endpoint/open-target services under
`environment/`, telemetry resource construction and provider-driver execution.
Managed policy **file loading is implemented, enforcement is not**: callers must
apply/validate `managedSettings.ts`, `managedProviders.ts`, `managedLimits.ts` and
`managedUpdates.ts` before representing a server as managed. Registry Group Policy
and macOS managed-preferences sources in `managedPolicyFile.ts` are not file-loader
fallbacks here and remain unported. No OS policy file is read implicitly.

Known differences: Rust error/validation messages and OTEL diagnostic warning
text differ from Effect's messages; invalid keybinding entries have a generic
message with the same wire issue kind and index. Windows process liveness and
legacy process start time use native `sysinfo` data instead of Node signal-zero
and `ps`/PowerShell subprocess probes. Changes are serialized through native
filesystem access; large JSON files are read/encoded synchronously, unlike
Effect's filesystem implementation. File-policy loading returns a document and
fingerprint, not an enforcement-ready `ManagedPolicy` Effect layer. Rust strings
and `serde_json` cannot represent the unpaired UTF-16 surrogate escapes JavaScript
accepts; the contracts corpus keeps those separately in `non-scalar/`. Such files
remain untouched on a parse failure. SHA-256 and persistent file formats otherwise
retain their existing representations.

Local disk types `runtime::RuntimeState` and `runtime::LoadedPolicy` are **to move
to t3-contracts**; the TypeScript versions are currently server-local. Runtime
mode, resolved paths, inputs and watcher handles are native service types.
`ConfigInputs` and `SettingsState` omit their payloads in `Debug`; telemetry debug
output omits headers, URLs, resource attributes and diagnostic text. Existing public
types, method signatures and trait implementations remain available.

## Verification

Tests use explicit temporary homes exclusively. Behavior cases come from
`serverSettings.test.ts`, `keybindings.test.ts`, `serverRuntimeState.test.ts`,
`cli/config.test.ts`, `os-jank.test.ts` and shared path/environment helpers.
They exercise real files, Windows file locks, native watchers and SQLite,
including an external symlink-target edit. Tests await events or state and never
sleep or assert elapsed time; retry/debounce timers use Tokio's paused clock.
Both native watchers are tested under a symlinked home on Windows, and canonical
remove events are tested without requiring the removed file to exist. macOS
FSEvents behavior remains unexecuted. The POSIX lock-replacement case uses Tokio's
paused clock and remains unverified on this Windows host, as do POSIX permission and
signal-zero branches. The complete TypeScript service/policy/PATH-probe suites
are not claimed as ported by the file-only portions above.
The file-shape table covers BOM, CRLF, comments, trailing commas, empty/whitespace,
UTF-16, malformed UTF-8 (inside and outside strings), truncated/wrong-root JSON,
and 8 MiB padding, asserting byte preservation. Settings also decode every current
TypeScript-generated scalar `ServerSettings` fixture. No installed-home corpus was
read; these generated samples are not evidence about an actual installation.

From `rust/`, with `CARGO_TARGET_DIR` set to
`C:\Users\Administrator\t3code-rust\rust\target\agents\t3-config`:

```text
cargo fmt -p t3-config
cargo clippy -p t3-config --all-targets -- -D warnings
cargo test -p t3-config
```
