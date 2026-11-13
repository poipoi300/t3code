# Paused at coordinator request, 2026-11-13

Resume this same task/thread. The coordinator requested an immediate pause because
the account's weekly usage is nearly exhausted. No commits or pushes were made.
Do not spawn subagents. Only this crate is owned by this task.

## Location and build rules

- Crate: `C:\Users\Administrator\t3code-rust\rust\crates\t3-device`.
- It was assembled at `D:\t3code-rust-target\server-i\staging\t3-device` and
  passed `cargo check` there before being moved into the workspace. The staging
  directory is no longer present.
- Every Cargo invocation must set
  `CARGO_TARGET_DIR=D:\t3code-rust-target\server-i`, run from
  `C:\Users\Administrator\t3code-rust\rust`, and scope to `-p t3-device`.
- Keep the workspace's two-job setting. The assigned build directory is also
  being used by a `t3-preview` task; occasional build locks are expected. Do not
  change it, kill their processes, or override jobs.
- Read the original request, shared
  `C:\Users\Administrator\t3code-rust-notes\briefs\server-port.md`, Rust rules,
  and `t3-mcp` README/traits for the governing contracts.

## Implemented so far

- `service`: consent, host registry/reconciliation, state revisions and lossless
  initial subscription, inspection/update/retry, open/close/shutdown, progress,
  incomplete-discovery retention, screenshot/detail/actions, per-thread lookup,
  agent target/launcher lifetime, and optional mutation admission checks.
- `io`, `host`: narrow process/HTTP/host/readiness/routing/launcher seams and
  concrete `t3-process`/reqwest adapters. Secrets and external diagnostics are
  scrubbed rather than carrying raw request/process causes into wire errors.
- `hub`, `actions`: hub operations and native iOS/Android actions/detail parsing.
- `local`: SDK discovery, helper startup/supervision, cached daemon revalidation,
  safe startup diagnostics, retained agent release during hub failure, stopping
  with the recorded CLI version, Linux runtime-directory ownership check.
- `ssh`: validation, local-target detection, probe/bootstrap, port forwards,
  Windows diagnosis, three-miss health monitoring and reconnect loop. The latter
  was added recently and needs stronger dedicated tests.
- `cloud`: Expo credential and GraphQL API, accounts/Simlock identity,
  EAS durable session ledger, uncertain creates/failed stops and cleanup;
  Simlock catalog filtering/caching, leases, CLI sessions, renewals and routing.
- `agent`, `tools`, `project`: endpoint files and launchers, staged installation,
  completed-version inventory and maintenance locking/pruning, static EAS project
  discovery without executing app config.
- `proxy`, `mjpeg`: Axum mount, auth admission, strict allowlists, credential/ticket
  stripping, stream forwarding, screenshot routing, WebSocket connector seam and
  one-time MJPEG still-frame repeat.
- Optional `mcp` feature implements the four existing `t3-mcp::services::DeviceService`
  methods in `DeviceAdapter`, with a `Threads` integration trait.
- No wire types were redefined; domain inputs/outputs use `t3-contracts`.

## Verification at pause

The last running command has FINISHED; no build/test launched by this task is
still in flight:

```powershell
$env:CARGO_TARGET_DIR='D:\t3code-rust-target\server-i'
cargo test -p t3-device --features mcp
```

Result: **76 tests passed, 8 suites, 0 failures**, with the latest source.
`cargo fmt -p t3-device` also ran just before that command.

Earlier focused runs: service 14, boundaries 23, cloud 12 (one URL-parity failure
was observed then fixed), proxy_stream 9, project_tools 9. The latest complete
run includes hosts tests and confirms the URL fix.

The required Clippy check has NOT passed yet. Its previous run reported one
`collapsible_if` in `hub.rs`; that was fixed, but substantial edits followed.
No new Clippy run was started after the pause request. The optional MCP build
previously had a typed-map lookup error; that is fixed and the latest all-feature
test command compiled successfully.

Tests use fake tools/HTTP/hosts and temporary filesystem state. No adb, emulator,
xcrun, SSH host, Expo/Simlock API, connected device, simulator, npm installation,
product dev server, or live T3 data was exercised. Node ran only an isolated
formatter function extracted from TypeScript to generate quick-start goldens.

## What to do first on resume

1. Run the scoped Clippy check and inspect/fix its actual findings. Do not assume
   the required checks are complete because tests passed. Then finish the focused
   parity/security audit below, write README, and rerun required checks once.
2. Review all new source and the original TS behavioral tests carefully; several
   TS test files were inspected through behavior/assertion extracts rather than
   read line-for-line. In particular finish comparing EAS/Simlock lifetime,
   cancellation and stale-host tests, LocalDeviceHost tests, and the generated
   Expo response schemas. The initial broad source pass is not proof of full
   parity.

## Outstanding work and known audit findings

- **README is still missing.** Write the required ported/unported inventory,
  external seams, integration requirements, real-tool limitations and precise
  behavior differences. This crate is not yet ready for the requested final
  implementation report.
- Finish checking credential leakage. Two concrete pending fixes identified just
  before pause: Simlock identity's non-agent `role` string should scrub the token
  before formatting its error; EAS preview `remote_id` should validate the selector
  before it is placed in agent CLI flags. Add refusal witnesses before fixes.
- Add actual MCP adapter tests (authorization under the mutation lock, host
  selectors, open/close/screenshot routing). The feature currently compiles, but
  the 76 tests do not directly drive the adapter methods.
- `tests/quick_start.json` is a four-case golden produced from the unchanged TS
  `agentDeviceQuickStart` function (iOS, Android, EAS, Simlock). Wire a Rust test
  to `mcp::quick_start` and assert exact equality. The fixture currently has no
  consumer. The function was expanded to preserve the original guidance; do not
  leave the earlier shortened version or assume parity without the comparison.
- Add focused tests for SSH reconnect and health miss reset, Simlock renewal/404
  removal/maximum lease age and stop-during-open, stale host replacement/listing,
  launchers binding/pruning cancellation, and real owned loopback WebSocket frame
  forwarding/cancelled handshake. Tests must never use real devices or APIs.
- Revisit EAS abandoned-start cleanup and ledger retirement races. The host has
  `retire`/`close_host` to close old writers and retains failed billed stops; its
  limits/retry timeouts exist, but original TS has more concurrency witnesses.
- `NativeHostFactory::test` currently returns an error for a local SSH target;
  TypeScript returns the local host's summary. Resolve this through an injected
  local host/summary rather than redefining contract types.
- NativeHostFactory supplies a concrete `agent_entry` to Simlock rather than
  lazily invoking Toolchain.ensure as TS does. Complete the first-use install
  seam (and its fake test) if required for full runnable construction.
- Verify every action refinement (not just enum decoding), especially location
  number/range rules, app ids, URL payloads, and identifiers from remote outputs.
- Registry settings changes need the owning config service to call `reconcile`;
  no config-crate subscription adapter was added. Server WS/HTTP composition is
  intentionally not edited. `DeviceAdapter::Threads` and proxy `Authenticator`
  must be wired by their owning server/MCP crates.
- Compatibility payloads `launcher.mjs`, `remote_environment.sh`, and
  `remote_bootstrap.cjs` were extracted from the TS implementation. Rust launches
  them through `t3-process`; the SSH payload still uses Node to manage remote
  tools. Document this retained Node dependency, provenance and scope. Audit the
  payload against the shared process-ownership rules; local stale-PID reaping
  was deliberately omitted, whereas the remote compatibility script retains
  its recorded-owner checks.
- Known intentional differences to document/assess: credentials/userinfo in HTTP
  URLs and option-like/NUL/CR/LF command selectors are refused; Simlock gateway
  validation is stricter than TS's regex/URL.canParse (origin only); redirects
  are refused so credentials cannot move to a different origin; raw causes are
  removed from public errors; external errors are typed Rust errors with safe
  text, not exact Effect cause objects; open/configure/close/shutdown complete in
  owned Tokio tasks when callers drop (TS local open is interruptible); timestamps
  currently use nanoseconds for session identity, while TS uses millisecond ISO;
  MJPEG tracking copies bounded frame buffers rather than retaining zero-copy
  chunk views. Some error guidance is still shorter than TS.
- `project::literal_ids` is a bounded conservative Rust scanner, not a JS parser.
  Existing refusal/inheritance cases pass, but compare remaining TS holder/spread
  edge cases. Its whitespace/comment handling must fail closed on ambiguity.
- A crate-local `Cargo.lock` came along from the standalone staged build. Remove
  that redundant file before completion; the workspace lockfile is authoritative.
- No independent review was spawned (user expressly forbids subagents). Do a
  self-audit for weak tests and fail-open/hanging paths, as originally requested.

Required final commands, from `rust/` with the assigned target:
`cargo fmt -p t3-device`,
`cargo clippy -p t3-device --all-targets -- -D warnings`,
`cargo test -p t3-device`. Also verify the optional `mcp` feature after its tests.
Do not commit or push, or edit another crate/the workspace manifest.
