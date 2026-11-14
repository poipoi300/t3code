# t3-provider

Native provider configuration, discovery, catalogs, auth, usage, installation and
maintenance services. The checklist distinguishes implemented service behavior
from the host/native bridges still required for production integration.
This crate does not implement or redefine
`t3-orchestration::provider::ProviderAdapter` and does not target WebAssembly.

## Public API

- `domain::ProviderDomain::open`: one policy-aware environment composition root
  for all 29 handler services, shared secrets, settings reconciliation, provider
  snapshots, managed Codex runtime admission and deterministic shutdown. See the
  external server recipe for required host inputs and startup barriers.

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
  Direct and managed Codex and CLI-owned Claude login keep separate credential
  paths; Claude intentionally has no in-app login controller.
- `service::ProviderService`: snapshot subscriptions, refresh, update coordination,
  reset-credit consumption and text-generator model listing. `ProviderHost`
  reconciles typed settings, secret changes and policy through `InstanceRegistry`,
  retiring sessions before replacements. `NativeProviderFactory` constructs
  direct Codex/Claude instances; `ManagedCodexProvider` supplies the managed path.
- `managed_provider::ManagedProvider`: deferred startup, demand/interval checks,
  serialized probes, transient-failure deferral, authoritative auth/usage updates,
  coalesced usage refresh and cancellable enrichment/manifest subscriptions.
  `status_cache::ProviderStatusCache` owns bounded atomic snapshot persistence.
- `CursorProvider`, `GrokProvider`, `OpenCodeProvider`, `KiroProvider`, `PiProvider`,
  `MuseProvider` and `FxInstance` implement `ProviderInstanceService` in their
  corresponding modules. Constructors take explicit instance settings, paths,
  environment and dependencies; `into_managed` adds the common lifecycle.
  `ProcessPiProbe` runs only the fixed, prompt-free inventory controls.
- `antigravity_provider::AntigravityProvider` and
  `host::AntigravityProviderFactory`: lazy private profiles, no-spawn installation
  health checks, explicit metadata-only model refresh, per-model thinking levels,
  commands/skills, Google auth/logout and scoped quota reads.
  `AntigravityInstallation` supplies managed installs and executable leases;
  `ProcessAntigravityFactory` owns disposable ACP children and cleanup.
- `acp_service::AcpRegistryService`: all eleven registry RPC operations through
  `RegistryAgentCatalog`, the native manager and `AcpApplicationHost`.
  `text_generators::TextGenerators` lists configured OpenAI/Anthropic-compatible
  endpoints with resolved secrets. `catalog::list_capability_models` applies
  visibility/order/pinning/search and source catalog limits for MCP callers.

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
| `server.refreshProviders` | `domain::ProviderDomain::refresh_providers` | Policy-aware secret reconciliation and focused refresh/response-shaping tests pass |
| `server.updateProvider` | `service::ProviderService::update_provider` | Coordination, current-version precheck, ownership recheck and terminal verification implemented; native Codex/Claude maintenance injected with `with_maintenance` |
| `server.listTextGeneratorModels` | `service::ProviderService::list_text_generator_models` / `text_generators::TextGenerators::list_models` | Implemented; attach with `set_text_generators` |
| `provider.chatgpt.handoff.subscribe` | `handoff::HandoffService::subscribe` | Isolated in-memory stream; verified export and disconnect cancellation tested |
| `provider.install.start` | `installations::ProviderInstallations::start` | Codex and Antigravity installers; register each driver with the router |
| `provider.install.cancel` | `installations::ProviderInstallations::cancel` | Implemented |
| `provider.install.subscribe` | `installations::ProviderInstallations::subscribe` | Implemented |
| `provider.install.remove` | `installations::ProviderInstallations::remove` | Managed guards and protected executable/router tests pass |
| `provider.consumeResetCredit` | `service::ProviderService::consume_reset_credit` | Native Codex control and Claude HTTP/file paths implemented; recorded-response/coordinator tests pass |
| `server.searchAcpRegistry` | `acp_service::AcpRegistryService::search` | `registry_service::RegistryAgentCatalog`; focused tests pass |
| `server.prepareAcpRegistryAgent` | `acp_service::AcpRegistryService::prepare` | `registry_service::RegistryAgentCatalog`; binary and package backends implemented |
| `server.uninstallAcpRegistryManagedBinary` | `acp_service::AcpRegistryService::uninstall_managed_binary` | `registry_service::RegistryAgentCatalog`; binary and package backends implemented |
| `server.acceptAcpRegistryUrlAuth` | `acp_service::AcpRegistryService::accept_url_authentication` | Coordinator tests pass |
| `server.listAcpRegistrySessions` | `acp_service::AcpRegistryService::list_sessions` | Mapping through injected native manager and host; at most 16 reads, source order retained |
| `server.importAcpRegistrySession` | `acp_service::AcpRegistryService::import_session` | Serialized launch/history repair through host; `ImportLaunch.created_by` is `User` |
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
  Registry discovery explicitly return `Error::NotPorted` from this generic CLI
  API; their provider-specific SDK/installation/registry services handle discovery.
  `managed_release` never downgrades installs.
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
  descriptions. `registry_service`, `registry_installation` and
  `registry_packages` execute the validated binary/npm/uv recipes with receipts,
  staged publication, cancellation, protected paths and safe uninstall.
- `usage`: sparse usage-window merging, clamping, no-op snapshot identity,
  preservation of reset metadata and fresh live limits after older probes.
  Finite percentages use `speech::SpeechNumber::Finite(wire::JsNumber)`;
  non-finite values clamp to zero. JavaScript numeric equality keeps
  integer/float no-ops stable.

## Driver checklist

“CLI” below is an executable/version check, not an authenticated readiness probe.
Common instance/environment/secret/policy support applies to every row.
“Remaining” names host/native integration outside the provider-service boundary,
including a missing OpenCode process-service dependency. Live integrations and
POSIX execution have not been verified on this Windows machine.

| Driver kind | Discovery ported | Version gate ported | Models ported | Options ported | Modes ported | Auth ported | Remaining |
| --- | --- | --- | --- | --- | --- | --- | --- |
| `codex` | PATH/custom CLI; managed/local packages | Manifest compatibility and per-model minimums | Native app-server and managed account catalog; bundled/custom models | Reasoning, configured service tiers/cyber, defaults | Four runtime modes, explicit override precedence | ChatGPT OAuth/PKCE/JWT, transfer, renewable scoped tokens, revocation, private broker | `t3-server` package G wires settings/background streams; `t3-orchestration` owns adapter lifecycle |
| `claudeAgent` | PATH/custom CLI; Windows entry resolution; prompt-free native probe | Manifest compatibility; model min/max filtering | Bundled/reported/custom models; runtime suffix/context resolution | Effort cap sources, Ultracode migration | Permission/plan/sandbox precedence and bypass availability | CLI-owned login guidance; account/usage probes and five-minute cache with one-use fallback; isolated homes | `t3-server` package G wires settings/background streams and injects `NativeMaintenance`; `t3-orchestration` owns adapter lifecycle |
| `cursor` | Shipped SDK HTTP catalog; no CLI invented | Manifest compatibility | Cached per-key HTTP models, strict SDK schema and custom models | Parameter ordering, defaults, booleans, effort/context/fast mapping | Windows auto-review/sandbox precedence and agent/plan mapping | T3 credential store, legacy migration, scoped browser login via narrow SDK trait, configured-key precedence, verified atomic commit and logout | `t3-server` package G supplies the narrow `CursorLogin` SDK bridge and host wiring |
| `grok` | PATH/custom CLI; updater-disabled probes; initialize-only metadata and CLI skill inventory | Manifest compatibility | CLI fallback and ACP catalogs, defaults and custom models | Reasoning metadata/default/order conversion | Supervised/Auto/Full-access argv and credential selection | CLI login verdict/recheck, API-key precedence, CLI-owned login guidance, official-account billing with custom-deployment refusal | `t3-orchestration` supplies adapter/text-generation attachment; `t3-server` package G wires lifecycle |
| `opencode` | PATH/custom CLI, external HTTP identity, owned server/cache/idle lifetime | Minimum 1.14.19; generation classification; manifest compatibility | v1 connected-provider and v2 settling catalogs; independent workspace models, skills and commands | Variant/agent/default/family legacy mapping | v1 approval/sandbox rules and v2 runtime/plan ordering | Read-only upstream readiness and private UTF-8 Basic transport; auth remains CLI-owned | `t3-server` package G supplies `OpenCodeServiceBackend`; `t3-process` supplies the still-missing native `LedgerProcesses` bridge for the persisted orphan ledger; `t3-orchestration` attaches adapters |
| `antigravity` | Leased managed/external ACP runtimes; paired harness/PATH discovery | Official CLI target/digest validation and compatibility | Explicit catalog refresh and live config callbacks; learned thinking options and workspace skills | Per-model observations, default aliases and duplicate labels | Permission and CLI-mode mapping | Client-owned Google flow, callback binding, profile isolation, safe browser helper and token/quota probe | `t3-server` package G wires the supplied factory/browser helper/installer; `t3-orchestration` forwards adapter catalog events |
| `kiro` | CLI version/whoami; auth-gated model/template probes | Manifest compatibility | Live list/default aliases and custom models | Static efforts and live agent template | Autopilot mapping; approval/full modes | First-line whoami verdict; no login inference from API-key settings | `t3-orchestration` adapter attachment; manual-only maintenance matches source |
| `pi` | CLI version and owned offline ephemeral RPC inventory | Minimum 0.80.5; manifest compatibility | Reported provider/id pairs, defaults, custom models, text-generation exclusions | Thinking opt-ins/clamped defaults and service tiers | Launch validation, extension/tool restrictions and runtime MCP environment | Inventory-derived readiness; no-refresh auth-check refinement | `t3-orchestration` attaches the adapter and materializes its runtime MCP extension |
| `muse` | CLI version and read-only MSP catalog seam; updater/key isolation | Revision parser/comparison and manifest compatibility | Validated meta catalog and custom models | Unknown/empty/reported efforts and defaults | Explicit ask/unrestricted permissions and serve args | Ready catalogs retain unknown auth; login/setup guidance | Native MSP protocol bridge implementing `MuseCatalog` remains external (server package G/native-client owner); `t3-orchestration` attaches the adapter |
| `fx` | Complete CLI status/models; managed lifecycle and workspace skills | Manifest compatibility | Live CLI/default/custom list | Effort descriptor and common inheritance | Ask/auto mapping, advertised auto/code compatibility | Status parsing and CLI-owned login guidance | `t3-orchestration` adapter attachment; manual-only maintenance matches source |
| `acpRegistry` | Offline local/index inspection, managed binary/package install and receipts, validated cache, safe uninstall | Exact-version package rules and versioned probe cache | Background ACP catalogs, current/default/custom models and learned per-model options | ACP config controls and legacy session modes | Advertised config and mode descriptors; adapter policy remains external | Agent/browser consent, env-var verification, optional terminal host, persisted confirmation binding, logout | `t3-server` package G supplies PTY, projections/import-history and thread launch via `AcpApplicationHost`; `t3-orchestration` attaches the adapter |

## Source coverage and remaining shared work

Authority: TypeScript checkout `dea07a1b12d954c2b7f4bfacb47354d9fbb64f5d`, merged as `817a9186f9`.
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
- `packages/provider-{fx,cursor,grok,opencode,kiro,pi,muse}/src/server/` status,
  discovery, catalog, usage, skills and maintenance helpers, and their
  `apps/server/src/provider/Drivers/*Driver.ts` construction/mode rules.
- `apps/server/src/provider/{AntigravityInstallation,AntigravityProvider}.ts`,
  `AntigravityAuth.ts`, `Drivers/Antigravity{Driver,Usage,Skills}.ts` and
  `acp/Antigravity*.ts` helpers:
  installation/release validation, runtime setup/filtering, auth callbacks,
  model observations, private profiles, skills, quota and temporary-session files.
- `apps/server/src/mcp/OrchestratorMcpService.ts::listCapabilityModels`:
  hidden/custom/pinned models, stable preference order, provider/model search
  terms and bounded/full catalogs; the corresponding source listing cases are
  covered in `tests/catalog.rs`.

The supplied services cover the requested configuration/discovery/catalog/mode/
auth/setup/policy/RPC boundary. The application still needs the bridges below;
they must not be mistaken for verified end-to-end provider support:

- **`t3-server`, provider handlers package G:** connect settings/demand streams,
  register driver factories/installers, implement session retirement and the ACP
  application host, expose the token broker/helper commands and Cursor SDK login,
  and attach native MSP catalog support for Muse. `ImportLaunch.created_by` must
  be passed to thread launch, alongside its native-history metadata.
- **`t3-orchestration`, provider adapters:** adapter lifecycle, provider turns,
  runtime MCP extension materialization, text-generation execution and live
  callbacks/usage ingestion. This crate exposes mode/catalog transforms and
  callback methods; it does not replace the existing adapter trait.
- **`t3-process`, process-service owner:** provide the native `LedgerProcesses`
  bridge for Linux/Darwin process identity/group observation and group signals.
  `opencode_ledger::OpenCodeLedger` owns persistence, optional serve argv, owner
  checks and validated orphan decisions; `OpenCodeRuntime::new_with_ledger`
  tracks captured children and forgets on exit. The server integrator must open
  one ledger at startup with its state directory, platform, captured owner PID and
  that bridge, run `reap_orphans` as a supervised background task, and pass the
  same ledger to every owned OpenCode runtime. Reaping logs/skips each bad record
  independently. The default runtime cannot reap earlier groups without this bridge.
  `OpenCodeServiceReplacement` retains its tested host-backend seam.
- **MCP handler owner:** call `catalog::list_capability_models` with preferences,
  normalized search terms and favorite/inherited/custom pinning. Project scopes,
  permission grants and the final `orchestrator_capabilities` response remain
  application concerns.

Source helpers for attachments, failure rendering, native logging, prompts,
instructions, continuation/session execution and orchestration projections were
not copied here; they cross the adapter/application boundary. No provider turns
were implemented or exercised as part of this port.

## Differences and verification limits

- Deliberate hardening beyond TypeScript: redacted text-generator keys are reused
  only when both the draft kind and base URL exactly match the saved configuration;
  another endpoint or protocol receives no saved credential.

- Deliberate hardening beyond TypeScript: registry archives permit only regular
  files/directories, at most 10,000 entries and 1 GiB of summed declared expansion
  before extraction. Tar metadata comes from numeric-owner verbose listings; ZIP
  metadata is checked directly. Registry-supplied SHA-256 remains optional.
- Inherited limitation (review 2 L5): a refresh-token rotation can be lost if the
  20-second exchange timeout or credential-lease loss lands after the provider
  answers but before persistence. This matches TypeScript; recovery requires
  reconnecting. Completing/persisting an already-sent exchange needs a separate
  concurrency design decision, so refresh semantics are unchanged.

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
  publication and per-install locks are implemented. Windows ARM selection,
  System32 archive-tool resolution and hostile archive members have fixtures.
- Pi's discovery stdout cap is 8 MiB (the source RPC client permits larger
  frames); an oversized catalog fails with a fixed, payload-free error.
- The current `t3-contracts` usage window duration is `Option<u64>`. Muse accepts
  positive finite fractional minutes in TypeScript, so fractional reports cannot
  be represented faithfully here and are rejected. The `t3-contracts` owner must
  widen that field before exact parity is possible; percentages already use
  `JsNumber` through the current contract.
- POSIX process behavior, permissions, real installations, live provider SDKs,
  browser/device OAuth and real update endpoints were not exercised on Windows.

Run from `rust/` with the configured job limit/compiler wrapper and
`CARGO_TARGET_DIR=D:\t3code-rust-target\server-providers` for every Cargo command:

```text
cargo fmt -p t3-provider --check
cargo clippy --offline -p t3-provider --all-targets -- -D warnings
cargo test --offline -p t3-provider
```

Tests use temporary files, owned `t3-process` fixture executables, recorded
responses, ephemeral loopback HTTP and Tokio clocks/channels. They never start a
provider turn or call a provider endpoint. The installed-Codex version probe is
opt-in and ignored by default. Secret-bearing requests, credentials, callbacks,
profiles, native launches, release URLs and errors have formatting assertions.

## Verification and integration hand-off

The merged-contract full crate run passed **257 tests, zero failed, one opt-in
ignored** across 64 test binaries/doc-test summaries. Scoped formatting and
strict dependency-inclusive `cargo clippy --all-targets -- -D warnings` pass;
the former 57-finding Codex dependency blocker has been repaired by its owner.
The test transcript is
`D:\t3code-rust-target\server-e\provider-tests-drift.log`.

Focused regressions cover ACP's 16-read mapping bound/order and import actor,
Antigravity catalog refresh/per-model thinking/retirement/no-spawn health,
withdrawal during validation, Windows protected paths and capability listing.
The full run exposed a concurrent Pi fixture-log interleave; complete JSON records
now append in one write, and the subsequent focused and full suites pass.
Integration order: attach Codex/Claude first through the services and RPC mapping
above, then Cursor/OpenCode and the remaining drivers. The missing native/host
bridges and the contract/process-service limitations have named owners above;
there is no provider-service work hidden behind an unimplemented RPC method.
No live credentials, provider servers, real turns, product servers or git
mutations were used. Only `rust/crates/t3-provider` and the explicitly requested external server seam
note were edited intentionally; Cargo updates the shared lockfile when resolving this crate's dependencies.

Already present at takeover: instance/environment/policy/catalog helpers,
partial CLI discovery, secret and registry parsing, modes/usage helpers and
focused tests. Those implementations were retained where source behavior matched.
Replaced from the interrupted work: `usage.rs`'s obsolete finite-number
construction with the current `SpeechNumber::Finite(JsNumber)` contract and
partial generic status paths with provider-specific services. Subsequent
regressions fixed Windows managed-install protection, Antigravity no-spawn health
and concurrent fixture logging; all pass in the final suite.


## Upstream merge 817a9186f9 — integration and drift round

`domain::ProviderDomain::open` now composes the handler services, shared secrets,
instance registry, direct/managed Codex and Claude factories, settings watch,
managed installation router, ACP catalog/service and token broker. The exact
29-RPC/config/editor/dependent-owner recipe is in the authorized external note
`C:\Users\Administrator\t3code-rust-notes\briefs\server-seam-requests.md`,
section “t3-provider recipe — unified domain and 29 RPCs (2026-11-14)”. Domain,
host and broker fixtures pass; the domain fixture uses temporary homes and fake
provider effects and verifies startup metadata, deferred probes, current settings
and retirement/broker revocation.

The domain requires the host's loaded `ManagedProviders` policy. It applies
limits at startup, settings reconciliation and supplied-secret refresh, disables
API text generators under an allowlist, and exposes unavailable rows for missing
native bridges. Server settings writes still call `policy.vet_write` before
persistence. `domain.apply_secret(reference, value)` and
`domain.refresh_providers(input)` preserve this policy boundary.

| Drift area | Implementation / remaining owner |
| --- | --- |
| Readiness/current settings | Managed-provider barrier plus domain watch; no probe on readiness-channel closure |
| Context budgets | Current `AGENT_CONTEXT_BUDGET_JSON` embedded data; instruction/tool-list assembly belongs to orchestration 11 / MCP |
| Claude/Codex custom endpoint | Strict routing decode and HTTP(S) origin metadata; workspace Claude settings override parent environment |
| Codex slow quota read | Full discovery budget retained; gated offline fixture answers after four virtual seconds and reports 42% |
| Antigravity frontmatter / MacPorts / zeroSpendCap | Existing bounded scanning, manual-only ownership and retained cap behavior |
| Sign-in URLs | Auth publication reuses canonical trim/HTTP(S)/16,384 UTF-16 validation; invalid interactions fail without publishing a URL; compatible wire dropping remains contracts-owned |
| Cursor account history | Shared stateful reader, personal + own-user team scopes, v2 cache helpers from t3-usage, 16-entry TTL/expiry cache, six slots/two scopes, no partial results |
| Cursor SDK defaults | `models::cursor_sdk_model_selection` and `cursor_context_window_tokens`; pass live catalog for interactive runs and text generation |
| Async process launch | Native/ACP/Pi/OpenCode/Antigravity use `spawn_non_blocking`; `muse::CatalogRequest::spawn` supplies the same launch path to the MSP bridge |
| OpenCode activation | Caller-bound model read, plugin-only one-second retry for two minutes, configured settle deadline and warm-cache progress |
| OpenCode metadata | Compact per-workspace variants/context cache, input limit precedence, uncached failures and preserved unconfirmed variants |
| OpenCode ledger | Persisted optional argv; exact PID/start/group checks, wrapper exec, strict old entries; fake process backend tests; native process bridge remains t3-process-owned |

Native adapter changes remain blocked on their assigned **t3-orchestration**
owners, not implemented by metadata discovery: 06/11/integrator must apply
Claude skill `mentionsFrom = inlineContextLength`; 12 must wire Cursor catalog
defaults and checkpoint context separately from complete/partial billed totals,
OpenCode step context/capacity and resumed child ordinals, v2 lost-report call
settlement, and Pi UTF-8/truncated command-output deltas. Grok's no-interrupt
steering flag is already present in `adapters/acp.rs`. Native title generation
also needs the adapter execution bridge; `TextGenerators` lists models only.
The server recipe explains construction and startup order for all three consumers.

Verification after all changes: 257 passed, zero failed, one opt-in ignored;
scoped formatting and dependency-inclusive Clippy with warnings denied passed.
The sign-in URL, policy and OpenCode deadline/activation regressions were observed
failing before their behavior fixes. Existing frontmatter, MacPorts and
zeroSpendCap cases also pass in the full suite. The native ledger backend,
adapter changes, MSP bridge and title runner bindings above remain the named
cross-crate blockers; no additional owned service edits are in flight.
No real provider home, credentials, provider network or provider turns were used.


## Native server wiring

`t3-server` starts `ProvidersAdapter` after the existing orchestration session
manager, inserts the same `ProviderDomain` and `SecretResolver` handles into
Domains, and merges provider rows into configuration snapshots/status streams.
Its optional `ProviderEffects` replaces only network/process edges in socket
fixtures. `DomainPorts` also accepts optional maintenance probe/executor and
registry fetch/command/download ports; `None` retains the native implementation.

The server serves auth/setup and installation RPCs, provider updates, ACP catalog
search/prepare/uninstall and text-generator model lists. Authorized reconnect
exports only the contract-defined hint; profile import strips that hint before
saving registration. Handoff remains unsupported pending the server/cloud
owner's trusted destination binding. Global refresh/reset awaits the shared usage
runtime and source-command projection; eight native ACP session/consent RPCs
await `AcpSessionHost` from the orchestration/native adapter owner. Configuration
editor IDs await the host/process owner's shared editor discovery/launch bridge.
These server bindings are distinct from the domain implementations in the table.

The wiring round also repairs independent review 2's M1: fresh managed Codex
installation uses the build pin, and only a supported managed installation
contributes an update baseline. Recorded archive tests cover fresh installation,
a disappeared local executable and a subsequent upstream update offer. Review 2's
remaining low findings are addressed in the review verification below. The
OpenCode/Antigravity runtime bridges still require their named host bindings.

Final wiring verification: 9 default-feature server socket tests and 9 focused
producer tests (2 library, 6 installation, 1 domain) pass. Producer all-target
Clippy and server provider/host/git/terminal Clippy pass with dependency linting
disabled and warnings denied. Broader lint findings belong to project/preview
and the shared no-terminal workspace import; exact locations and all remaining
bindings are recorded in the external server-wiring-providers report. Formatting
passes; no provider process, provider network or real provider home was used.


## Reviewed boundaries and verification

`ProviderHost` reads its `SettingsSource` under the host lock. Mutable settings
callers must pass a live source; `ServerSettings` remains a fixed snapshot for
fixtures and one-shot callers. The domain supplies its policy-filtered source.
Credential commits use a separate lock so published auth state remains readable.
Diagnostics record module/line, safe category/class and an opaque redacted cause;
wire errors retain their fixed messages.

Review round 1 verification on Windows: 268 provider tests/doctests passed, one
opt-in real-provider probe ignored; nine server provider socket tests passed.
All-target dependency-inclusive Clippy with warnings denied and scoped formatting
passed. The Unix broker's post-create integration case is included but could not
run on Windows; the portable directory-invariant matrix passes here. The three
transport/parser caps and malformed managed-agent uninstall IDs are covered.


Review round 2 producer verification on Windows: 278 provider tests/doctests
passed, one opt-in provider probe ignored; all-target dependency-inclusive Clippy
with warnings denied and scoped formatting passed. Codex probe caches include
negative results and are invalidated by executable fingerprints; managed and
local version grammars now match the TypeScript checks. Ledger cleanup continues
past bad records. Archive tests cover links, aggregate expansion and ZIP metadata.
JWT tests cover algorithm confusion, ambiguous keys and required-claim shapes.
Nine server provider socket tests also pass, including reconnect/import redaction
and scope refusal; an initial t3-project dependency declaration failure cleared
on that owner's repair and the unchanged retry. No build/test remains running.
The native Linux/Darwin ledger bridge and server startup wiring remain the named
cross-crate obligations above; the production native process bridge was not
exercised by the fake ledger backend on Windows.
