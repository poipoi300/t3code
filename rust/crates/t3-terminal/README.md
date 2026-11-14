# t3-terminal

Native, transport-neutral tokio terminal service; not a wasm crate. The TypeScript
specification is `apps/server/src/terminal/{Manager,NodePtyAdapter,OutputProtocol}.ts`.
`portable-pty` supplies ConPTY on Windows and native PTYs on Unix. All terminal
wire inputs, snapshots, events and status types are re-exported directly from
`t3-contracts::terminal`; there are no local wire payload definitions.

## API

Construct `TerminalManager` inside a tokio runtime using `ManagerOptions`:

- `new(logs_dir)` accepts the resolved TypeScript terminal logs directory.
- `from_base_dir(base)` selects `base/userdata/logs/terminals`.
- `from_state_dir(state)` selects `state/logs/terminals`, including a caller's
  resolved development state directory.

`open`, `get_shell`, `write`, `resize`, `clear`, and `close` implement terminal
operations. `get_shell` returns only the executable. `attach` opens or resumes;
`observe` requires an existing session and never opens or resizes it. Streams
return canonical contract events from `recv().await`. Reuse a connection's viewer
ID for attaches and resizes. Dropping a stream schedules detachment without
locking the manager in its destructor.

`write(input)` validates and enqueues synchronously when called, returning a
future for completion. Each PTY has one FIFO writer and a bounded 2,048-message
queue. A full queue returns a contextual write error rather than blocking the
caller. An accepted write is not cancelled by dropping its completion future.
PTY writes never hold a session or registry lock. The contract's 65,536 UTF-16
unit input limit also bounds each queued message.

`subscribe` returns lifecycle and output events. `subscribe_metadata` begins with
a sorted summary snapshot. `close_idle` inspects subprocess activity;
`close_on_settle` closes flagged terminals and then idle terminals. Thread deletion
calls `close` without a terminal ID and with `deleteHistory=true`. History deletion
and idle-inspection failures are logged rather than failing close operations.
`flush` is a persistence barrier. `shutdown` stops and persists sessions silently:
clients receive no `closed` or metadata `remove` notification, so they can resume
after a server restart. Its final PTY drain has one two-second deadline. Dropping
the last manager schedules cleanup; use explicit `shutdown` before stopping the
runtime when persistence must be guaranteed.

## Ported behavior

Sessions are keyed by `(threadId, terminalId)`. Each terminal has its own lock;
the registry only protects cell lookup, and the publication hub only protects
cached summaries and subscriptions. Spawn, environment resolution and disk I/O
never hold either shared lock. Background workers park when there are no sessions
and log failures and panics. Snapshot subscription and sequence assignment share
the terminal lock.

Running opens reuse their process. Changes to cwd, worktree path or resolved
environment restart it and clear history. Exited/error sessions restart through
open or explicit attach restart. Resume preserves closed and evicted IDs; unknown
IDs can recover disk history after server restart. Attached viewers share their
smallest grid, including the initial spawn of a restarting attach. Retries retain
a viewer's last resize; viewer ownership survives reopen.

Shell fallback retries only executable-not-found errors. Shell candidates,
PowerShell/zsh/macOS bash arguments, environment filtering, terminal identity,
fish query suppression, AppImage scrubbing, provider-home expansion and managed
PATH merging follow `Manager.ts` and `packages/shared/src/shell.ts`. Windows shell
environment lookups ignore key case. Managed PATH merging trims entries, removes
empties and duplicates, and appends managed directories. Provider environment
resolution is injected and fails closed; running attaches do not resolve it again.
Set `app_version` to the server's actual version; the standalone default is this
crate's package version.

History retains 5,000 lines and 8 MiB by default, with UTF-8-safe tails that start
in terminal parser ground state. Discarding an unfinished sequence retains its
state until later output terminates it. Persisted tails scan the skipped prefix
with a streaming decoder before discarding a cut sequence. `sequence` exposes
both UTF-8 and UTF-16 helpers; the latter preserves split surrogate state.
File names,
143-byte filename limits, migrations and plain UTF-8 disk format match TypeScript.
The 250 ms persistence worker appends only the pending tail; it materializes full
history for rewriting, recovery or compaction. Failed writes invalidate the
append cursor; flushing attempts every session even when another fails. Files
compact on close, oversized read or twice the byte limit. Replay strips queries
across split chunks and resets inherited input/alternate-screen modes; cold
history uses the same 512 KiB replay window. Windows output strips ConPTY's
`CSI ?9001h` and `CSI ?1004h` controls across split reads before streaming or saving
history. Other setters and later application cursor queries are preserved.

Stream queues charge UTF-8 bytes and envelopes, allow one oversized snapshot and
resync overflow atomically without spawning or changing viewer sizing. Raw event
subscriptions end on overflow; metadata streams recover a current summary snapshot.
`OutputWindow` supplies `OutputProtocol.ts`'s eight-chunk/64-KiB ACK accounting;
the transport must apply it when sending batches. Subprocess inspection uses one
native process table per poll, ignores childless same-shell prompt helpers,
updates labels and backs off idle or failed queries. Idle close validates the
session generation, sequence and input count after its fresh inspection.

## Process lifetime and platform limits

Windows defaults to TypeScript-compatible console lifetime: terminate the shell
and close its ConPTY, terminating attached console processes. Detached programs
such as editors, browsers and daemons survive both close and normal shell exit.
Set `ManagerOptions.kill_descendants = true` to assign a kill-on-close Job Object;
that explicit policy also terminates detached descendants on close, shutdown,
manager drop and normal shell exit. The job uses the owned process handle.

The optional Job Object still has a spawn-to-assignment window: `portable-pty`
0.9 cannot create its child suspended or expose the primary thread for assignment
before resume. A shell profile can start a descendant in that window. Closing it
requires a PTY backend API for suspended creation, job assignment and checked
resume. There is no unsafe code or claim of complete coverage during that window.

On Unix the wait worker alone signals and reaps the child; stop/drop only send
requests, so no stored PID is signalled after reaping. Shutdown sends SIGTERM and
escalates to SIGKILL after one second if the shell has not exited. Signals reach
the shell's process group, not every job: interactive jobs with separate groups
and detached sessions can survive. Exit comes from the wait worker after at most
200 ms of reader drain, even if a background job retains the slave. Unix PTY I/O
is nonblocking and observes cancellation; a full input buffer cannot stall close.
Signal death reports exit code zero; normal exit reports signal zero, matching
node-pty. Signal numbers are reconstructed from portable-pty's signal descriptions;
unknown descriptions remain nullable. ConPTY cursor negotiation searches the
startup burst, including earlier controls, and has a two-second deadline.

`t3-process` supplies executable resolution. Its current spawn/tree API owns piped
Tokio children and cannot spawn or adopt a portable-pty child. PTY lifetime stays
in `src/process.rs`; sharing it requires an owned PTY-child API with wait/reap
coordination, console teardown, optional job policy and suspended assignment.
No process is killed by name or process-table discovery.

## Integration remaining

- The WebSocket RPC/ACK adapter (`OutputProtocol.ts`) remains outside this crate.
- The application supplies provider environment preparation and managed binary
  directories. Provider settings/home materialization, cwd recovery, workspace
  leases, port discovery and telemetry registration from `Manager.ts` need their
  owning services. `AgentScope.wrap` (systemd scope and OOM score adjustment,
  `Manager.ts`) is also not ported; `t3-process` does not expose that service.
- Native process enumeration replaces the TypeScript native-telemetry/ps/WMI
  fallback chain. Native queries run off the async runtime but cannot yet be
  interrupted at TypeScript's inspection deadlines. Direct-child selection uses
  PID ordering.

## Verification

From `rust/`, with an isolated `CARGO_TARGET_DIR`:

```text
cargo fmt -p t3-terminal --check
cargo clippy -p t3-terminal --all-targets -- -D warnings
cargo test -p t3-terminal
cargo check -p t3-terminal --target x86_64-unknown-linux-gnu --tests
```

Windows tests use real cmd/PowerShell PTYs, detached socket children, temporary
history directories and native process inspection. They cover 400 concurrent
ordered writes, close/exit/replay/reconnect, exact sequence adjacency, viewer
sizing, slow-open isolation, silent shutdown, spawn-error classification,
persistence failures, environment rules, large Unicode chunks, query filtering,
wire goldens and ACK/overflow behavior. Tests await events and state rather than
sleeping or asserting elapsed time.

The Unix-only tests in `tests/posix.rs` cover a background job retaining the slave,
blocked PTY input, signal exit shape and closing after the child has been reaped.
They compile for Linux on this Windows host but are **unexecuted** here. Unix and
macOS runtime behavior still needs native verification.
