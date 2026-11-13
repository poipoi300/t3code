# t3-process

Native asynchronous process ownership; this crate is not a wasm dependency.
`spawn(&ProcessOptions)` returns piped `SpawnedProcess` handles for streaming and
interactive callers. `run(ProcessOptions)` writes stdin and collects both pipes
concurrently, with per-stream byte limits, optional truncation markers, timeout
results, cancellation and typed failures. Set `check_exit = false` for the
TypeScript runner's allowed-nonzero behavior. Bytes are retained; text accessors
use replacement decoding and the output records invalid UTF-8 independently.

`process-wrap` 10 owns POSIX process groups; Windows uses Tokio process creation
and [`win32job` 2.1](https://github.com/ohadravid/win32job-rs) for immediate job
assignment through a safe API. There is no unsafe code in this crate. Dropping an unfinished handle or run future
terminates its owned tree. Cancellation and timeout terminate the tree and wait
for the leader. `wait()` observes the leader, not every helper; after normal exit,
the job's kill-on-close limit is cleared before closing its handle, leaving
background helpers alive. Error cleanup closes the armed job, terminating its
members even when the leader has already exited. `run()` also drains both pipes,
so a helper retaining a pipe can keep collection open after its leader exits.
Windows termination is immediate. On Unix, `terminate(grace)` sends SIGTERM,
waits up to the supplied grace and then kills remaining group members; the
default grace is zero, unlike TypeScript's one second in `processRunner.ts`.
`ProcessOptions.termination_grace` selects that grace for cancellation/error
cleanup; `t3-git` sets one second. `on_stderr_chunk`, like `on_stdout_chunk`,
observes all bytes even when the retained output is truncated, supporting Git
hook progress without embedding Git behavior in this crate.
POSIX descendants that deliberately create a new session/process group escape
that group; Linux cgroup/systemd isolation is a separate service.

Windows resolution follows PATH/PATHEXT, explicit-path fallback and the native
Git-for-Windows binary preference. Successful bare-command resolutions use a
512-entry, 30-second cache keyed by program, effective PATH, normalized PATHEXT
and MSYSTEM mode, matching `packages/shared/src/shell.ts`. Cached files are
rechecked; misses, explicit paths and PATHs with relative entries are uncached.
A newly installed higher-priority executable can take up to 30 seconds to replace
a still-existing cached match. Arguments are never cached. Batch scripts are
passed directly to Rust's `Command::args`; no hand-built `cmd.exe` command line
or `raw_arg` is used. This deliberately differs from TypeScript's vulnerable
`escapeWindowsShellArg` in `packages/shared/src/shell.ts`: quotes, percent signs,
carets and shell operators stay arguments, while arguments the standard library
cannot safely encode (including CR, LF and NUL) return the existing
`ProcessError::Spawn` with `source.kind() == InvalidInput`, before executing the
script. `ResolvedCommand` keeps all its fields, but batch resolution now returns
the script path, original args and `shell_line: None` instead of a shell tail.
The resolution changes preserve public types and function signatures.

Windows children use `CREATE_NO_WINDOW`, matching the console-hiding intent of
TypeScript's Effect Node spawner (`windowsHide` defaults to true for non-detached
Windows children in `.repos/effect/packages/platform/node-shared/src/internal/nodeChildProcessSpawner.ts`).
A real console executable launched through a consoleless parent verifies that
no console is allocated; the console-window probe is compiled with Windows
PowerShell/.NET during that test. Interactive Electron launches, visible window
flashes on a desktop, GUI programs' own windows, Unix process groups and Unix
SIGTERM grace were not exercised on this Windows host.

The macOS xcrun Git shim optimization in `packages/shared/src/shell.ts` is not implemented; the normal
Git shim remains usable. These services are not implemented:
`apps/server/src/process/externalLauncher.ts`, `agentScope.ts`,
`providerProcesses.ts` and `windowsTcpListeners.ts` (all in that directory). `process-fixture` is a test
subprocess, not a server entry point.

`Environment`, `OutputCallback`, `ProcessOptions`, `ProcessOutput`, `OutputMode`,
`ProcessError`, `ResolvedCommand`, `resolve_command`, `spawn`, `run`,
`SpawnedProcess` and the re-exported `CancellationToken` remain the public API.
These are local process-service types; none belong in `t3-contracts`.

## Windows spawn cost

`run()` keeps process creation on Tokio's blocking pool; synchronous `spawn()`
retains its existing API. Windows creates an armed, non-inheritable job handle
before spawning, starts the child normally with `CREATE_NO_WINDOW`, and assigns
its actual process handle immediately, without an async yield. Cleanup closes
that kill-on-close job; normal completion clears the limit before closing it.
Assignment failure drops the owned process, killing/reaping the leader rather
than returning a live unowned handle. Rust's own batch argument encoding and the
workspace's Rust 1.99 minimum/unsafe-code denial are preserved.

The former `process-wrap::JobObject` always adds `CREATE_SUSPENDED` and resumes
by scanning a system-wide ToolHelp thread snapshot. Its `CreationFlags` wrapper
cannot disable that scan: requesting suspension leaves the child suspended, and
omitting the job loses containment. The measured replacement uses the existing
safe `win32job` dependency also used by `t3-terminal`. Atomic job-list APIs (for
example [firehazard's thread attributes](https://docs.rs/firehazard/latest/firehazard/process/struct.ThreadAttributeRef.html))
and alternative launchers such as [win-custody](https://docs.rs/win-custody/latest/win_custody/)
were inspected, but are not drop-in wrappers for Tokio/std commands and their
batch encoding/piped Tokio handles. They were not substituted or benchmarked.

The TypeScript specification (`apps/server/src/processRunner.ts` and
`.repos/effect/packages/platform/node-shared/src/NodeChildProcessSpawner.ts`)
spawns normally, then uses `taskkill /pid <pid> /T /F` on Windows during cleanup.
It does not assign a job atomically, skips tree cleanup once the leader has
exited, and does not prevent descendants escaping discovery. The other services
in `apps/server/src/process/` add no atomic Windows containment; provider scope
tracking is Linux-specific. Immediate job assignment deliberately accepts a
pre-assignment race rather than paying the snapshot cost.

A descendant created before assignment can escape; those created afterwards
inherit the job, including detached children. The probe separately measures
Tokio spawn return to assignment: cmd median/p95/max **31.8/50.1/6387.4 microseconds**,
git **29.4/53.6/136.7 microseconds**. Scheduling can stretch this interval. The
child may also execute inside `CreateProcess` before Tokio returns, so these are
not bounds on the whole race. Spawn entry to assignment provides a conservative
observed upper bound: cmd median/p95/max **6.54/24.81/92.91 ms**, git
**6.79/38.88/59.02 ms**. Neither is a hard worst-case bound. The regression fixture
starts its grandchild synchronously before initializing Tokio. The timeout test
runs it 100 times, waits for the grandchild's TCP readiness, advances the paused
clock, and asserts timeout plus socket closure. This passed on this host, as did
existing cancellation, dropped future/handle, detached-helper survival, batch
argument and console-allocation tests; it is practical evidence, not proof of
atomic containment.

Reproduce from `rust/` in PowerShell:

```powershell
$env:CARGO_TARGET_DIR='C:\Users\Administrator\t3code-rust\rust\target\agents\t3-process'
$env:T3_PROCESS_PROBE_LEGACY='1'
cargo run -p t3-process --example spawn-latency -- C:\Windows\System32\cmd.exe /d /c exit 0
cargo run -p t3-process --example spawn-latency -- git --version
```

The probe takes 200 samples per path after five warmups, with matching pipes and
console flags. It prints the resolved executable; plain Tokio and the legacy
reference spawn that same executable (`C:\Program Files\Git\mingw64\bin\git.exe`
for Git). Timings cover synchronous spawn only, including resolution for
`t3-process`, excluding wait/output collection. `T3_PROCESS_PROBE_LEGACY` enables
the former suspended-job algorithm for a repeatable before/after comparison;
that reference uses the current resolver cache, isolating the job-wrapper cost.
The probe also reports resolution and `run()` first-poll latency.

Windows, Rust 1.99.1, debug build, 2026-11-13; milliseconds (median / p95):

| Command | Plain Tokio | Former suspended job | Current t3-process |
| --- | ---: | ---: | ---: |
| cmd /d /c exit 0 | 6.39 / 36.23 | 94.62 / 146.90 | 6.59 / 36.99 |
| git --version | 6.96 / 33.49 | 97.19 / 134.03 | 7.16 / 34.38 |

Current overhead relative to Tokio is **0.20 ms median** for both commands,
**0.75 ms p95** for cmd and **0.89 ms p95** for git. Host scheduling/process
creation contributes substantial tail noise to all paths. `run()` first poll
is cmd **21.2 / 30.4 microseconds**, git **21.7 / 27.9 microseconds**.

Before any implementation change, the 100-sample original probe measured cmd
**87.03 / 108.89 ms** versus plain Tokio **6.00 / 26.61 ms**. Original bare-git
`t3-process` measured **104.37 / 135.78 ms**; that original plain-Tokio path used
the Git launcher, so use the matched-executable legacy comparison above instead.
After removing the snapshot but before caching, a further 100-sample probe
isolated bare-git resolution at **6.92 / 8.63 ms**, with total spawn
**13.00 / 40.03 ms** versus Tokio **5.47 / 7.11 ms**. TypeScript already caches
positive resolution for 30 seconds; the bounded matching cache removes that
second repeated cost. Final warm resolution is cmd **0.155 / 0.224 ms**, git
**0.207 / 0.276 ms**. Cold PATH scans still incur their filesystem cost.

POSIX process groups and SIGTERM grace were not executable on this Windows host.