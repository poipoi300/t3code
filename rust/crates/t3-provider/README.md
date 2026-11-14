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
| `codex` | PATH/custom CLI; managed/local packages | Manifest compatibility and per-model minimums | Native app-server and managed account catalog; bundled/custom models | Reasoning, configured service tiers/cyber, defaults | Four runtime modes, explicit override precedence | ChatGPT OAuth/PKCE/JWT, transfer, renewable scoped tokens, revocation, private broker | Host refresh/lifecycle assembly; final scoped review |
| `claudeAgent` | PATH/custom CLI; Windows entry resolution; prompt-free native probe | Manifest compatibility; model min/max filtering | Bundled/reported/custom models; runtime suffix/context resolution | Effort cap sources, Ultracode migration | Permission/plan/sandbox precedence and bypass availability | CLI-owned login guidance; account/usage probes and five-minute cache with one-use fallback; isolated homes | Additional synced-plugin/worktree/link cases and host lifecycle assembly; maintenance requires injected resolver |
| `cursor` | Shipped SDK HTTP catalog; no CLI invented | Manifest compatibility | Cached per-key HTTP models, strict SDK schema and custom models | Parameter ordering, defaults, booleans, effort/context/fast mapping | Windows auto-review/sandbox precedence and agent/plan mapping | T3 credential store, legacy migration, scoped browser login via narrow SDK trait, configured-key precedence, verified atomic commit and logout | SDK browser-login transport bridge supplied by host; host lifecycle wiring |
| `grok` | PATH/custom CLI; updater-disabled version probe | Manifest compatibility | Custom-model composition; metadata input | Reasoning metadata/default/order conversion | Remaining | Credential environment only | `grok models` auth/model parsing, ACP initialize catalog, login/setup, launch/runtime modes, skills, maintenance and capability integration |
| `opencode` | PATH/custom CLI, external HTTP identity, owned server/cache/idle lifetime | Minimum 1.14.19; generation classification; manifest compatibility | v1 connected-provider and v2 settling catalogs; independent workspace models, skills and commands | Variant/agent/default/family legacy mapping | v1 approval/sandbox rules and v2 runtime/plan ordering | Read-only upstream readiness and private UTF-8 Basic transport; auth remains CLI-owned | Registered-service handoff/AgentScope and POSIX orphan ledger; host/adapter capability assembly; text-generation integration |
| `antigravity` | Managed ACP route identified | Release-tag compatibility; pure managed-release selection | Bundled manifest | Raw profiles/common inheritance | Permission and CLI-mode mapping | Config secret resolution only | Platform install discovery, harness/profile isolation/materialization, release downloads/verification, OAuth/callback/profile leases, live catalog and capabilities |
| `kiro` | PATH/custom CLI | Manifest compatibility | Input list/default aliases | Static effort map; input agent template | Autopilot mapping | Input `whoami` parser | Execute whoami/model/template probes with source gating, ACP setup, full status publication, maintenance and capability integration |
| `pi` | PATH/custom CLI | Minimum 0.80.5; manifest compatibility | Custom-model composition only | Common inheritance only | Remaining | Credential environment only | Offline RPC catalog, auth-check refinement, launch-arg semantics, modes, maintenance and capabilities |
| `muse` | PATH/custom CLI, no automatic update during probe | Revision parser/comparison and manifest compatibility | Input `meta` catalog conversion | Reported/unknown/empty effort variants and defaults | Remaining | Source has no MSP auth signal; no readiness inference implemented | Read-only SDK catalog fetch, readiness, startup deadlines, runtime modes, login/setup guidance, launcher/mise/channel updates and capabilities |
| `fx` | PATH/custom CLI and complete status/model commands | Source has no minimum; compatibility resolver available | Fetched CLI/default/custom list | Effort descriptor and common inheritance | Ask/auto mapping, advertised auto/code compatibility | Status method/missing/unknown parsing; login remains CLI-owned | Managed snapshot refresh worker, workspace skills and runtime/adapter capability integration |
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

Builds use two jobs and `D:\t3code-rust-target\server-e`. Latest focused checks:
provider service (4), usage (3), ACP catalog (2) and ACP snapshots (2) pass.
Auth cancellation/write ordering passed red/green; terminal-host auth fixtures
are being checked. Formatting passes. Strict no-deps Clippy found two local
warnings, now repaired; rerun it after these edits. Full crate tests have not yet
been rerun after the ACP additions. Strict dependency Clippy previously failed in
`t3-codex-app-server` (`result_large_err` / `large_enum_variant`); owner files are
untouched. Earlier focused Cursor, OpenCode, usage, registry, probe, auth, managed
installation, broker, identity, reset-credit and maintenance suites passed.

`ProviderService` owns snapshot subscriptions and rejects replaced instances'
late results; a refresh reads the latest live snapshot under the storage lock and
preserves update progress. ACP readiness is offline, background probe results are
cached for 15 minutes, and missing commands override cached readiness. Discovery
does not prove authentication. Confirmation bindings use source-compatible JSON
field order and persist only a hash and boolean. Cancellation retains ownership
of pending disk writes. ACP test sessions are deleted and children closed; no
fixture permits a provider turn.

Next complete the shared managed-provider refresh/usage/settings lifecycle for
Codex and Claude, including deferred startup, demand-gated periodic probes and
stale enrichment guards. Then finish the remaining checklist rows. Cursor SDK
browser login and ACP PTY execution require host-injected transports. OpenCode
registered-service replacement needs the host AgentScope handoff; POSIX orphan
ledger behavior remains absent. `t3-process::SpawnedProcess::wait` releases
Windows detached helpers after leader exit; explicit close/forget is covered.
No live provider requests, real credentials, provider turns or git mutations.
