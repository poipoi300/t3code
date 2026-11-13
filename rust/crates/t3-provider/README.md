# t3-provider

Native provider services, independent of transports and provider adapters. This
is an **incomplete port**: every driver with unfinished checklist behavior is
listed below. It is not a replacement for the TypeScript provider host yet.
It does not implement or redefine `t3-orchestration::provider::ProviderAdapter`.

## Public API

- `auth::{AuthService, AuthController}`: client-owned flow state, method and
  interaction validation, expiry/cancellation, private subscriptions, shared
  credential-binding exclusion, session admission/revocation and profile import.
  `AuthBackend`, `SessionStopper`, HTTP/callback/store interfaces isolate effects.
- `chatgpt::ChatGptAuth`: dynamic OAuth registration, PKCE/callback validation,
  account/profile retention and transfer, scope-gated access, serialized renewal,
  revoked-token clearing and best-effort remote logout. Credentials retain the
  TypeScript secret-store names and JSON layout, including legacy records.
  `identity::JwksVerifier` verifies RS256/ES256 JWTs, claims and rotating cached
  keys. `Sensitive<T>` redacts Debug/Display while preserving wire serialization.
- `native_probe::{NativeProbe, ProcessProbes}`: prompt-free native Codex and
  Claude discovery; initialize/account/model/config/skills/usage controls only.
  `account_usage`, `probe_models`, `claude_catalog` and `modes` provide the
  account, usage, option, version-filter and runtime-policy transformations.
  Host integration and the remaining provider RPC services are being completed.

## Provider RPC service mapping

| RPC | Service method | State |
| --- | --- | --- |
| `provider.auth.start` | `auth::AuthService::start` | Implemented |
| `provider.auth.respond` | `auth::AuthService::respond` | Implemented |
| `provider.auth.complete` | `auth::AuthService::complete` | Implemented |
| `provider.auth.cancel` | `auth::AuthService::cancel` | Implemented |
| `provider.auth.logout` | `auth::AuthService::logout` | Implemented |
| `provider.auth.removeMethod` | `auth::AuthService::remove_method` | Implemented |
| `provider.auth.subscribe` | `auth::AuthService::subscribe` | Implemented |
| `provider.chatgpt.reconnect-profile` | `auth::AuthService::reconnect_profile` | Implemented |
| `provider.chatgpt.import-profile` | `auth::AuthService::import_profile` | Implemented |
| `server.refreshProviders` | `service::ProviderService::refresh_providers` | Focused refresh/response-shaping tests pass |
| `server.updateProvider` | `service::ProviderService::update_provider` | Coordination, current-version precheck, ownership recheck and terminal verification implemented; native Codex/Claude maintenance injected with `with_maintenance` |
| `server.listTextGeneratorModels` | `service::ProviderService::list_text_generator_models` / `text_generators::TextGenerators::list_models` | Implemented; attach with `set_text_generators` |
| `provider.chatgpt.handoff.subscribe` | `handoff::HandoffService::subscribe` | Isolated in-memory stream; verified export and disconnect cancellation tested |
| `provider.install.start` | `installations::ProviderInstallations::start` | Codex installer implemented |
| `provider.install.cancel` | `installations::ProviderInstallations::cancel` | Implemented |
| `provider.install.subscribe` | `installations::ProviderInstallations::subscribe` | Implemented |
| `provider.install.remove` | `installations::ProviderInstallations::remove` | Managed guards and protected executable/router tests pass |
| `provider.consumeResetCredit` | `service::ProviderService::consume_reset_credit` | Native Codex control and Claude HTTP/file paths implemented; recorded-response/coordinator tests pass |
| ACP-registry methods | ACP application service | In progress |

Claude's TypeScript driver does not expose an in-app auth controller: it probes
the CLI-owned account and directs signed-out users to `claude auth login`.
Returning an unsupported in-app auth operation preserves that source contract.

## Existing configuration and catalog API

- `instances::{derive_config_map, core_driver_definitions, InstanceRegistry}`:
  take `t3_contracts::settings::ServerSettings` from the `t3-config` settings
  service. Hydrate ten default instances (ACP Registry is explicit), preserve
  multiple instances per driver, resolve enablement, decode all eleven driver
  configs, batch environment/config-field secrets, park unreadable instances,
  and publish change subscriptions. `InstanceSpec` contains resolved configuration
  and environment; it deliberately has no `Debug` implementation. `reconcile`
  returns retired/prepared ids. **The host must stop retired runtimes before
  activating replacements.** This registry owns specifications, not adapters or
  session lifetimes. Invalidate secret reads and pass affected ids in `rebuild`
  on a vault refresh/configuration change; unchanged unreadable entries stay parked.
- `environment`: configured-value precedence, Windows uppercasing/removal of
  inherited spellings, selective home expansion, Claude host-session cleanup,
  pure Codex direct/auth-overlay layout and Claude home resolution. All spawn
  environments are explicit and `t3-process` receives `inherit_env = false`.
- `discovery::{Driver, probe_version}`: configured executable/PATH resolution
  through `t3-process`, bounded version-only probes, localized Windows missing
  command detection, provider timeout overrides, Pi/OpenCode minimum versions,
  and native version parsers. Cursor SDK, Antigravity managed ACP and ACP
  Registry discovery explicitly return `Error::NotPorted` from this CLI API.
  `probe_opencode_server` uses `/api/info`, then `/global/health`, JSON identity
  checks and UTF-8 Basic credentials. `managed_release` never downgrades installs.
- `catalog::{Manifest, ModelManifest, apply_catalog, custom_models,
  option_descriptors}`: bundled catalogs and profiles, manifest validation,
  current/legacy classification, expiring new badges, default-alias transfer,
  qualified Codex families and update-required models, custom-name overrides,
  effort/boolean selection defaults, prompt-injected values, implicit fast-mode
  opt-out and legacy Ultracode selection migration. `ModelManifest` accepts
  already-read disk cache data; `refresh` returns cache data for the host to
  persist, preserves current data on failure and rejects stale CDN revisions.
- `models`: pure conversions of supplied Cursor SDK parameters/variants, Muse
  catalog/efforts, Grok reasoning metadata and Kiro model lists, effort defaults,
  agent template descriptors and `whoami` auth. These do not fetch SDK/ACP data.
- `fx::FxProvider::{initial, check}`: complete no-session fx CLI status path:
  `fx --version`, `fx status --json`, `fx models --json`; canonical
  `ServerProvider` output, default aliases, custom models, effort descriptor,
  runtime modes, CLI-owned login guidance and no app text generation. Optional
  status/model fields reject explicit null, matching the TypeScript schemas.
- `maintenance`: update command quoting, proven npm-global prefix ownership,
  exact npm/pnpm update arguments, targeted version actions, version advisories,
  Homebrew/Chocolatey/npm result parsers and configured npm latest lookup.
  `LatestVersions` shares concurrent reads, caches successes one hour/misses one
  minute, and handles owner cancellation, failures and invalidation. Fair
  `InstallationGate` excludes new executions behind waiting updates and acquires
  multi-instance locks in sorted order. It does not execute updates.
- `secrets::{OnePassword, SecretResolver, CredentialStore}`: exact `op --cache=false`
  read/inject commands, account/reference validation, temporary templates, batch
  fallback, 45-second deadlines, verbatim values, configuration-scoped caching
  of failed reads and preservation of good cached values after failed fresh reads.
  Credential tuple hashing delegates raw secret persistence to `t3-config`.
- `policy::ManagedProviders`: provider allowlists, default-instance pins,
  locked/fill/ceiling config/environment overlays, pin-bypass field ownership,
  write vetting/restoration of policy echoes, project enablement checks, locked
  field descriptions and host launch-override filtering. Pure Kiro, fx and
  Antigravity mode mapping helpers are also exposed.
- `registry`: official ACP index URL, bounded/lenient index decoding, exact
  package validation, HTTPS/no-credentials checks, platform selection including
  Windows ARM emulation fallback, distribution preference, ranked search,
  cached network reads/fallback, archive path/checksum validation, install-root
  derivation, managed npm/uv install descriptions and executable override launch
  descriptions. **Descriptions and validation do not install an agent.**
- `usage`: sparse usage-window merging, clamping, no-op snapshot identity,
  preservation of reset metadata and fresh live limits after older probes.
  Finite percentages use the contracts' `wire::JsNumber`; non-finite values
  clamp to zero. JavaScript numeric equality keeps integer/float no-ops stable.

## Driver checklist

“CLI” below is an executable/version check, not an authenticated readiness probe.
“Input” means a pure conversion of data supplied by another service. Common
instance/environment/secret/policy support applies to every row. **All rows
remain unfinished as complete driver integrations.**

| Driver kind | Discovery ported | Version gate ported | Models ported | Options ported | Modes ported | Auth ported | Remaining |
| --- | --- | --- | --- | --- | --- | --- | --- |
| `codex` | PATH/custom CLI; managed/local packages | Manifest compatibility and per-model minimums | Native app-server and managed account catalog; bundled/custom models | Reasoning, configured service tiers/cyber, defaults | Four runtime modes, explicit override precedence | ChatGPT OAuth/PKCE/JWT, transfer, renewable scoped tokens, revocation, private broker | Host refresh/lifecycle assembly; shared-instance update propagation; final scoped review |
| `claudeAgent` | PATH/custom CLI; Windows entry resolution; prompt-free native probe | Manifest compatibility; model min/max filtering | Bundled/reported/custom models; runtime suffix/context resolution | Effort cap sources, Ultracode migration | Permission/plan/sandbox precedence and bypass availability | CLI-owned login guidance; account/usage probes and five-minute cache with one-use fallback; isolated homes | Additional synced-plugin/worktree/link cases and host lifecycle assembly; maintenance requires injected resolver |
| `cursor` | SDK route identified; no CLI invented | Manifest compatibility normalization | Input SDK model conversion | Parameter ordering, defaults, booleans, effort/context/fast mapping | Remaining | Remaining | SDK catalog fetch, paid-plan/auth classification, login setup, interaction/runtime modes and capabilities |
| `grok` | PATH/custom CLI; updater-disabled version probe | Manifest compatibility | Custom-model composition; metadata input | Reasoning metadata/default/order conversion | Remaining | Credential environment only | `grok models` auth/model parsing, ACP initialize catalog, login/setup, launch/runtime modes, skills, maintenance and capability integration |
| `opencode` | PATH/custom CLI; external HTTP identity probe | Minimum 1.14.19; v1/v2 classification; manifest compatibility | Custom-model composition only | Common inheritance only | Remaining | External server password transport/secret isolation | Native v1/v2 catalogs/options/auth/modes, managed-server selection/cache, maintenance and capability integration |
| `antigravity` | Managed ACP route identified | Release-tag compatibility; pure managed-release selection | Bundled manifest | Raw profiles/common inheritance | Permission and CLI-mode mapping | Config secret resolution only | Platform install discovery, harness/profile isolation/materialization, release downloads/verification, OAuth/callback/profile leases, live catalog and capabilities |
| `kiro` | PATH/custom CLI | Manifest compatibility | Input list/default aliases | Static effort map; input agent template | Autopilot mapping | Input `whoami` parser | Execute whoami/model/template probes with source gating, ACP setup, full status publication, maintenance and capability integration |
| `pi` | PATH/custom CLI | Minimum 0.80.5; manifest compatibility | Custom-model composition only | Common inheritance only | Remaining | Credential environment only | Offline RPC catalog, auth-check refinement, launch-arg semantics, modes, maintenance and capabilities |
| `muse` | PATH/custom CLI, no automatic update during probe | Revision parser/comparison and manifest compatibility | Input `meta` catalog conversion | Reported/unknown/empty effort variants and defaults | Remaining | Source has no MSP auth signal; no readiness inference implemented | Read-only SDK catalog fetch, readiness, startup deadlines, runtime modes, login/setup guidance, launcher/mise/channel updates and capabilities |
| `fx` | PATH/custom CLI and complete status/model commands | Source has no minimum; compatibility resolver available | Fetched CLI/default/custom list | Effort descriptor and common inheritance | Ask/auto mapping, advertised auto/code compatibility | Status method/missing/unknown parsing; login remains CLI-owned | Managed snapshot refresh worker, workspace skills and runtime/adapter capability integration |
| `acpRegistry` | Index/search/platform/recipe/override descriptions | Exact-version package rules | Custom-model composition only | Common inheritance only | Remaining | Secret resolution only | Binary/package installation and receipts, staged extraction/locks/uninstall, local command inspection, ACP auth/probe/runtime coordination and capabilities |

## Source coverage and remaining shared work

Authority: TypeScript checkout `2d8a843e6be9ca170f3bce98cc92ad76617b00c4`.
`data/model-manifest.json` is a byte-for-byte bundled snapshot from
`apps/server/src/provider/model-manifest.json` (SHA-256
`9af1ba02f747c869ba5d624aece2a889cc8700c37c09123c354b3f68180a8dc8`).

Ported behavior and test cases come from:

- `packages/provider-core/src/server/{instanceEnvironment,pathExpansion,
  ModelCatalog,snapshotProbe,ProviderLatestVersions,maintenanceResolver,
  usageLimits}.ts` and their tests; corresponding option/custom-model helpers
  in `packages/shared/src/{model,semver}.ts`.
- The configuration/secret portions of
  `apps/server/src/provider/ProviderInstanceRegistry{,Hydration}.ts`,
  `ProviderCredentialStore.ts`, `ProviderInstallationGate.ts`, `ModelManifest.ts`,
  `ClaudeModelManifest.ts`, `codexModelCatalog.ts`, `providerCompatibility.ts`,
  `managedRelease.ts`, `Drivers/{ClaudeHome,ClaudeExecutable,CodexHomeLayout}.ts`.
- `apps/server/src/policy/managedProviders.ts`; every `packages/secret-provider-*`
  implementation; the decoding/search/recipe portions of
  `packages/provider-acp-registry/src/server/AcpRegistrySupport.ts`.
- `packages/provider-fx/src/server/{status,acpSupport}.ts`, provider Cursor/Kiro/Grok
  `server/status.ts`, Muse `server/{modelCatalog,maintenance}.ts` and OpenCode
  `server/versionProbe.ts`. Only the behavior identified above is ported.

Shared services still being completed: `ProviderHost`/driver runtime construction,
adapter attachment/teardown, `managedProvider`/`ProviderRegistry` refresh and
workspace caches, checked-secret carry/reload orchestration, `ProviderAuthService`
integration of the implemented auth ownership/session/callback/secret-delivery flows,
`providerMaintenanceRunner` command coordination and notifications,
`ProviderUsageLimitsIngestion`, per-driver usage readers, model visibility/order
and complete `orchestrator_capabilities` integration. Other provider-core helpers
(attachments, failures, text generation, MCP/session/continuation services, native
logging, instructions and projections) have not been ported into this crate.
Native protocols and per-provider adapters remain with their assigned owners.

## Differences and verification limits

- The host supplies settings, disk caches, wall-clock timestamps and adapter/SDK
  responses; no background provider worker or automatic network task is started.
  No T3 home, live credentials or product server is discovered implicitly.
- `FxSettings` and packaged-driver settings codecs are **to move to
  t3-contracts** when those package schemas are exported there. Manifest,
  compatibility policy and registry distribution types mirror server-local
  TypeScript schemas; they are not replacement wire response types. Public
  provider snapshots/options/auth use the existing contracts.
- JS `localeCompare` falls back to deterministic Rust lexical order for text
  tie-breaks and invalid-version comparisons. JS Date parsing is limited here to
  RFC3339 timestamps and `YYYY-MM-DD`. Some generic probe/error messages differ.
- General CLI checks report availability/version only and keep status at warning
  until a real auth/catalog service decides readiness. They do not replace the
  provider-specific TypeScript status functions. Output limits and process-tree
  handling follow `t3-process`; its documented native differences also apply.
- Registry fetch uses one 30-second request/body deadline; TypeScript permits
  separate 30-second header/body deadlines. Disk cache writes and installer
  publication/locking are not implemented. Archive helper checks alone must not
  be treated as a complete secure extractor.
- POSIX process behavior, permissions, real installations, live provider SDKs,
  browser/device OAuth and real update endpoints were not exercised on Windows.

Run from `rust/` with
`CARGO_TARGET_DIR=D:\t3code-rust-target\server-e` and the configured four Cargo jobs:

```text
cargo fmt -p t3-provider
cargo clippy -p t3-provider --all-targets -- -D warnings
cargo test -p t3-provider
```

Tests use temporary files, actual `t3-process` fixture executables, a local HTTP
fixture and paused Tokio clocks/channels. They never start a provider turn.
The installed-Codex version probe is opt-in and ignored by default.

Verified on Windows on 2026-11-13 against the landed `JsNumber` contracts:
formatting and Clippy with warnings denied pass; 66 tests pass and the one
installed-binary probe remains ignored. POSIX and live provider integrations
remain unverified as described above.

Current implementation checkpoint (2026-11-13):

- `codex_installation`: bounded downloads, pinned release data, SHA-256 and package
  layout validation, staged tar inspection/extraction, compatible active/receipt
  records, atomic activation, cancellation, local reuse, update offers and leased
  removal. Network is supplied by `ReleaseTransport`; tests use recorded archives.
- `managed_codex` supplies private environments and token-sharing configuration;
  `token_broker` supplies bounded private pipes/sockets, and `homes` isolates auth.
- `credential_lease` uses Node's `<secret>.bin.lock` directory, 120-second stale
  leases and 10-second heartbeat. Replacement detection and release are tested.
- Callback success/failure HTML and return URL allowlisting match the source.
- New modules compile; focused Windows tests pass for auth cancellation/import
  validation, callbacks, ChatGPT identity/renewal, token broker, credential leases,
  managed packages, native prompt-free probes, modes and account/usage conversion.
  The full strict Clippy command currently fails in separately owned
  `t3-codex-app-server` (`result_large_err` and `large_enum_variant`). Dependency
  files were left alone. The 66-test result above records the previous baseline.

## Active continuation — resumed after quota reset, 2026-11-13

The coordinator has lifted the quota pause. This task is **not finished**;
continue in this thread. Do not delegate, commit, alter other
crates, start provider turns, or use live network. Never read real provider/T3
credential directories. All filesystem tests use temporary directories.

Every Cargo invocation now uses `CARGO_TARGET_DIR=D:\t3code-rust-target\server-e`
and the configured four Cargo jobs; do not go back to the populated C: target. Run from
`C:\Users\Administrator\t3code-rust\rust` with `login:false` and
`rtk proxy cargo ... --offline -p t3-provider`. Use explicit UTF-8 for Python.
The manual usage refresh reports GPT/Codex quota unknown; the coordinator owns
reset/pause decisions.

### Work completed after the previous hand-off

- `auth.rs`: shared binding domains atomically exclude peer mutations/admission,
  cancel all admitted access after method validation and session stopping, and
  preserve access through controller replacement. Registry subscriptions recheck
  controller identity before publishing/ending, closing the replacement race.
  Seven focused auth tests passed, including the previously red shared-binding
  regression and replacement/removal/invalid-method behavior.
- `credential_lease.rs`: reject replacement symlinks even when they point to the
  original directory; the old owner never removes the replacement. Lease
  compromise now cancels the guarded future. ChatGPT export, finish, renewal,
  logout, profile removal and import run inside that guard. Four focused lease
  tests passed, including a paused-clock in-flight cancellation test.
- `codex_installation.rs`: resolve and validate each managed `tools/codex`
  component under the canonical base, rejecting symlinked ancestors before
  removal or creation. The regression failed before the fix; four installation
  tests then passed with recorded archives and temporary directories.
- `native_probe.rs`: native ChatGPT catalog discovery now waits up to three
  seconds (100 ms production polling, bounded by the overall deadline) for a
  `models_cache.json` stamp newer than spawn and lists again; cached models are
  retained on timeout/refetch failure. Pagination rejects cursor cycles. The
  process fixture writes a temporary stamp and supplies a different second list.
- Native fixture reset-credit control checks the supplied idempotency key and
  returns the canonical `reset` outcome. No real reset/turn is sent.
- `native_provider.rs`: deleted Claude workspaces retain user skills and only
  `/compact`, with pending false and no subprocess. Workspace snapshots reuse
  machine version and avoid account/usage probes. Five native tests passed,
  including catalog freshness, reset control and the deleted-workspace case.
- `tests/chatgpt.rs`: verified handoff exports an isolated in-memory profile;
  dropping a pending stream closes its fake callback receiver. That focused test
  passed. Existing HTTP replies, callbacks, identities and entropy are all fakes.

### Exact stopping point

The edit in hand was `managed_codex.rs` plus the new
`tests/managed_codex.rs`. The end-to-end fake test first reached the final
snapshot and failed because managed snapshots still exposed `/feedback`.
The implementation now removes that command, exposes the effective runtime's
cancellation token for host consumers, and races auth access, native probing
and account-model HTTP against admitted credential cancellation. The test
covers a reused local installation, real private local broker setup, sanitized
launch environment, isolated home/auth, account model visibility and unsupported
native quota. It starts no provider process and uses no live HTTP.

**Final pause verification:** the focused run completed successfully: auth (7),
ChatGPT (8), managed Codex (1), native provider (5): 21 passed, zero failures.
Credential lease (4) and Codex installation (4) passed separately earlier this
resume. `cargo fmt -p t3-provider` and
`cargo check --offline -p t3-provider --all-targets` both passed on the D: target
with two Cargo jobs. No full suite or Clippy was started for this pause. All
edits are finished; no source or test is half-written, and no command remains
in flight.

### First work on resume and remaining priorities

1. Re-read current files and collect/re-run only any check whose result was lost.
   Finish Codex/Claude before other drivers. Add installer-router tests; review
   managed snapshot cancellation after the second credential access and its
   failure snapshot's auth metadata. Consider controller registration during an
   already active direct flow: normal use registers fresh controllers, but the
   binding-domain reassignment path has not been exercised with a pre-active
   incoming controller.
2. Propagate a shared dynamic manifest source into native/managed snapshots,
   installation compatibility and update checks; they currently hold static
   `Arc<Manifest>` values. Wire host construction, auth/installation dependencies,
   shared usage/secret refresh and adapter lifetime seams. `t3-server` package G
   owns handlers; do not edit that crate. README RPC mapping above names the
   current application methods; ACP registry methods remain absent.
3. Review native process cleanup on early errors and cancellation; Unix broker
   permissions/cleanup remain unexecuted on Windows. Add more synced Claude
   skill/worktree/link-containment cases. Gateway grace clamps to i32 max, which
   is a documented source-comparison item rather than claimed parity.
4. Maintenance resolution: make the pnpm `or(...)` fallback lazy, compare whole
   mise-list schema rejection with source, and extend ownership negatives for
   WinGet/Chocolatey/Homebrew/wrappers. Update coordination still needs refresh
   of all sharing instances, progress and OpenCode restart shaping.
5. Then complete Cursor and OpenCode, followed by Grok, Antigravity, Kiro, Pi,
   Muse, fx and ACP registry. The driver table's incomplete rows still apply.
   Read `ws.ts`, `AcpRegistrySessions`, runtime coordinator and registry installer
   before implementing one typed service method per ACP RPC; update the table.
6. Run scoped formatting, no-deps Clippy and full crate tests once complete.
   Strict dependency Clippy previously failed in separately owned
   `t3-codex-app-server` (`result_large_err`, `large_enum_variant`); do not edit it.
   Re-read contract types before use because other owners remain active.

Previous focused groups passed separately before this resume: reset credits (3),
maintenance resolution (4), provider service (3), Claude catalog (1), skills (3),
Claude probe cache (2), homes (2), text generators (2), callback (2), broker (2),
ChatGPT (7), identity (1), account/usage (4). The old 66-test line above is the
accepted earlier baseline, not a final aggregate result for the current crate.

Current resume checkpoint: installer-router/protected executable cases pass;
managed catalog cancellation rereads current auth and clears stale usage metadata;
shared `catalog::ManifestSource` connects accepted catalog refreshes to native and
managed snapshots, installation compatibility, and update compatibility checks.
`NativeProvider::new` and `ProviderService::new` accept either a fixed manifest or
a live `Arc<ModelManifest>`; `InstallationOptions.manifest` and
`ManagedCodexProvider.manifest` now take `ManifestSource` (`.into()` from either).
The loopback refresh regression changes a live Claude catalog without rebuilding
the provider. Maintenance regression tests reject an entire malformed mise listing
and keep pnpm's installed-version probe lazy. Five focused maintenance tests pass.
Formatting completed; full crate test run is now in flight. No provider turns,
live HTTP, real credentials or git mutations were used.
