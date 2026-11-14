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
| `server.searchAcpRegistry` | `acp_service::AcpRegistryService::search` | `registry_service::RegistryAgentCatalog`; focused tests pass |
| `server.prepareAcpRegistryAgent` | `acp_service::AcpRegistryService::prepare` | `registry_service::RegistryAgentCatalog`; binary and package backends implemented |
| `server.uninstallAcpRegistryManagedBinary` | `acp_service::AcpRegistryService::uninstall_managed_binary` | `registry_service::RegistryAgentCatalog`; binary and package backends implemented |
| `server.acceptAcpRegistryUrlAuth` | `acp_service::AcpRegistryService::accept_url_authentication` | Coordinator tests pass |
| `server.listAcpRegistrySessions` | `acp_service::AcpRegistryService::list_sessions` | Mapping through injected native manager and host |
| `server.importAcpRegistrySession` | `acp_service::AcpRegistryService::import_session` | Serialized launch/history repair through host |
| `server.deleteAcpRegistrySession` | `acp_service::AcpRegistryService::delete_session` | Capability and imported-thread guard |
| `server.listAcpRegistryProviders` | `acp_service::AcpRegistryService::list_providers` | Advertised capability guard |
| `server.setAcpRegistryProvider` | `acp_service::AcpRegistryService::set_provider` | Header/API validation and stop-before-write |
| `server.disableAcpRegistryProvider` | `acp_service::AcpRegistryService::disable_provider` | Required-provider guard and stop-before-write |
| `server.logoutAcpRegistry` | `acp_service::AcpRegistryService::logout` | Advertised logout; controller/native path |

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
| `codex` | PATH/custom CLI; managed/local packages | Manifest compatibility and per-model minimums | Native app-server and managed account catalog; bundled/custom models | Reasoning, configured service tiers/cyber, defaults | Four runtime modes, explicit override precedence | ChatGPT OAuth/PKCE/JWT, transfer, renewable scoped tokens, revocation, private broker | Final integration review; server supplies settings/background streams and adapter lifecycle |
| `claudeAgent` | PATH/custom CLI; Windows entry resolution; prompt-free native probe | Manifest compatibility; model min/max filtering | Bundled/reported/custom models; runtime suffix/context resolution | Effort cap sources, Ultracode migration | Permission/plan/sandbox precedence and bypass availability | CLI-owned login guidance; account/usage probes and five-minute cache with one-use fallback; isolated homes | Final integration review; server supplies settings/background streams; maintenance requires injected resolver |
| `cursor` | Shipped SDK HTTP catalog; no CLI invented | Manifest compatibility | Cached per-key HTTP models, strict SDK schema and custom models | Parameter ordering, defaults, booleans, effort/context/fast mapping | Windows auto-review/sandbox precedence and agent/plan mapping | T3 credential store, legacy migration, scoped browser login via narrow SDK trait, configured-key precedence, verified atomic commit and logout | SDK browser-login transport bridge supplied by host; host lifecycle wiring |
| `grok` | PATH/custom CLI; updater-disabled probes; initialize-only metadata and CLI skill inventory | Manifest compatibility | CLI fallback and ACP catalogs, defaults and custom models | Reasoning metadata/default/order conversion | Supervised/Auto/Full-access argv and credential selection | CLI login verdict/recheck, API-key precedence, CLI-owned login guidance, official-account billing with custom-deployment refusal | Text-generation/adapter attachment; final integration review |
| `opencode` | PATH/custom CLI, external HTTP identity, owned server/cache/idle lifetime | Minimum 1.14.19; generation classification; manifest compatibility | v1 connected-provider and v2 settling catalogs; independent workspace models, skills and commands | Variant/agent/default/family legacy mapping | v1 approval/sandbox rules and v2 runtime/plan ordering | Read-only upstream readiness and private UTF-8 Basic transport; auth remains CLI-owned | Registered-service handoff backend/AgentScope supplied by host; POSIX orphan ledger; adapter/text-generation attachment |
| `antigravity` | Managed ACP route identified | Release-tag compatibility; pure managed-release selection | Bundled manifest | Raw profiles/common inheritance | Permission and CLI-mode mapping | Config secret resolution only | Platform install discovery, harness/profile isolation/materialization, release downloads/verification, OAuth/callback/profile leases, live catalog and capabilities |
| `kiro` | CLI version/whoami; auth-gated model/template probes | Manifest compatibility | Live list/default aliases and custom models | Static efforts and live agent template | Autopilot mapping; approval/full modes | First-line whoami verdict; no login inference from API-key settings | Adapter attachment; manual-only maintenance matches source |
| `pi` | PATH/custom CLI | Minimum 0.80.5; manifest compatibility | Custom-model composition only | Common inheritance only | Remaining | Credential environment only | Offline RPC catalog, auth-check refinement, launch-arg semantics, modes, maintenance and capabilities |
| `muse` | CLI version and read-only MSP catalog seam; updater/key isolation | Revision parser/comparison and manifest compatibility | Validated meta catalog and custom models | Unknown/empty/reported efforts and defaults | Explicit ask/unrestricted permissions and serve args | Ready catalogs retain unknown auth; login/setup guidance | Native MSP transport supplied by host; usage reports, maintenance verification and adapter attachment |
| `fx` | Complete CLI status/models; managed lifecycle and workspace skills | Manifest compatibility | Live CLI/default/custom list | Effort descriptor and common inheritance | Ask/auto mapping, advertised auto/code compatibility | Status parsing and CLI-owned login guidance | Adapter attachment; manual-only maintenance matches source |
| `acpRegistry` | Offline local/index inspection, managed binary/package install and receipts, validated cache, safe uninstall | Exact-version package rules and versioned probe cache | Background ACP catalogs, current/default/custom models and learned per-model options | ACP config controls and legacy session modes | Advertised config and mode descriptors; adapter policy remains external | Agent/browser consent, env-var verification, optional terminal host, persisted confirmation binding, logout | Host PTY/session-import assembly; Windows ARM archive-tool selection; cancellation and cross-platform hardening |

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
  `packages/provider-acp-registry/src/server/{AcpRegistrySupport,probe,auth}.ts`
  and ACP runtime model/config parsing. `acp_service::AcpRegistryService` exposes
  all eleven application RPC operations; `AcpNativeServices` uses `t3-acp` for
  read-only probes and management, without sending prompts.
- `packages/provider-fx/src/server/{status,acpSupport}.ts`, provider Cursor/Kiro/Grok
  `server/status.ts`, Muse `server/{modelCatalog,maintenance}.ts` and OpenCode
  `server/versionProbe.ts`. Only the behavior identified above is ported.

Shared services still being completed: remaining driver runtime construction,
adapter attachment/teardown, registry workspace
projection rules, settings stream wiring, `ProviderAuthService`
integration of the implemented auth ownership/session/callback/secret-delivery flows,
`providerMaintenanceRunner` command coordination and notifications,
`ProviderUsageLimitsIngestion`, per-driver usage readers, model visibility/order
and complete `orchestrator_capabilities` integration. Other provider-core helpers
(attachments, failures, text generation, MCP/session/continuation services, native
logging, instructions and projections) have not been ported into this crate.
Native protocols and per-provider adapters remain with their assigned owners.

## Differences and verification limits

- The host supplies settings, paths, background demand and adapter/SDK effects.
  ACP owns explicitly constructed background probe/watch tasks. No T3 home, live
  credentials or product server is discovered implicitly.
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
  separate 30-second header/body deadlines. Registry disk caching, staged
  publication and per-install locks are implemented. Windows ARM archive-tool
  selection and hostile archive-member fixtures remain to verify.
- POSIX process behavior, permissions, real installations, live provider SDKs,
  browser/device OAuth and real update endpoints were not exercised on Windows.

Run from `rust/` with
`CARGO_TARGET_DIR=D:\t3code-rust-target\server-e` and the configured two Cargo jobs:

```text
cargo fmt -p t3-provider
cargo clippy -p t3-provider --all-targets -- -D warnings
cargo test -p t3-provider
```

Tests use temporary files, actual `t3-process` fixture executables, a local HTTP
fixture and paused Tokio clocks/channels. They never start a provider turn.
The installed-Codex version probe is opt-in and ignored by default.

## Current continuation checkpoint

Builds use two jobs and `D:\t3code-rust-target\server-e`. The last full crate
run passed 197 tests with one installed-binary probe ignored. Since that run,
Claude skills (5), instance reconciliation (9), host reconciliation (1),
status cache (2), and managed lifecycle (7) pass
in focused runs. Strict no-deps Clippy passed after the Grok additions. Strict dependency Clippy previously failed in
`t3-codex-app-server` (`result_large_err` / `large_enum_variant`); owner files are
untouched. Formatting passes.

`managed_provider::ManagedProvider` supplies deferred startup, demand/interval
control, one deferred transient failure, serialized probes, usage merging and
label clearing, generation guards, cancellable background enrichment, coalesced usage refresh
and cancellation-safe ownership. Seven lifecycle tests pass. Native Codex/Claude expose pure placeholders
and `into_managed`; managed Codex also subscribes to auth transitions. Claude's
real fixture verifies cache invalidation for explicit usage refresh. The synced
plugin, worktree/backlink and declared symlink-boundary cases pass on Windows.

`host::ProviderHost` reconciles typed settings through `InstanceRegistry`, stops
retired sessions before activating replacements, retries retirement failures,
registers initial snapshots/auth controllers and publishes unavailable rows.
`NativeProviderFactory` assembles direct Codex/Claude runtimes; managed Codex
runtime dependencies remain supplied through the existing factory seam. The host
fixture proves retirement/drop/create ordering and retained state after failure.
Policy changes now retire unchanged live configurations; unavailable-reason Debug
redacts configuration/vault detail. Both fixes passed red/green regressions.

ACP catalog (2), auth (4), snapshots (2), native wrappers (1) and connection (2)
passed. Discovery never proves login; confirmation writes retain the persistence
lock through cancellation. Provider service (4) and usage (3) pass, including live
snapshot precedence and `zeroSpendCap` retention. ACP probes delete empty sessions
and close owned children. No fixture permits a provider turn.

`status_cache` validates identities, merges current custom models with cached
built-ins, and atomically persists machine rows without update/workspace state.
Only managed placeholders hydrate; completed checks stay authoritative. Registry
secret reloads retain working instances on failure and carry checked values
without rereading other live credentials. Host methods apply these changes under
the same retirement/activation lock. Unsafe cache file names are rejected instead
of allowing paths outside the cache directory.

Codex/Claude background enrichment and automatic manifest rechecks passed the
full suite. Cursor and OpenCode expose pure placeholders and managed lifecycle
construction; Cursor (10) and OpenCode (12) focused suites pass.
`opencode_service::OpenCodeServiceReplacement` implements address/version guards,
configured environment rules and an uninterruptible handoff/start pair through
`OpenCodeServiceBackend`. Its three fake-backend tests pass, including RPC
cancellation during stop. The host supplies the native Service/AgentScope backend;
this crate does not stop an arbitrary registered PID.

Grok status/catalog/skills/modes (3), account usage (3), and maintenance (7) focused
suites pass. ACP initialization sends extension metadata and never authenticates,
creates a session or prompts. Model login text is retried only when signed out
without an API key. Billing reads only the official credential scope, refuses
custom auth/endpoints/config sections, and redacts requests and refusal errors.
`windowDurationMins` is emitted as the current contract's integer type; a failed
fixture caught the lenient decoder dropping a float-valued window. Timestamp
validation matches the source's strict casing. Grok native updates retain
mise/Nix/Homebrew ownership and allow only its package's postinstall.

Kiro status/model/template and model parsing tests pass. fx metadata/roots/workspace refresh tests pass; workspace skill refresh performs no CLI probes. Muse status/readiness, launcher guidance, deadlines and key isolation pass three focused tests; strict optional catalog parsing passes its regression. Muse launcher/mise/channel maintenance verification is next.

Next finish the remaining Pi, Muse and Antigravity checklist rows, then
perform final scoped verification. Cursor SDK browser login and ACP PTY execution
remain host-injected effects. OpenCode's POSIX orphan ledger remains absent.
`t3-process::SpawnedProcess::wait` releases detached Windows helpers after leader
exit; explicit close/forget is covered. No other owner's files were changed.
No live provider requests, real credentials, provider turns or git mutations.
