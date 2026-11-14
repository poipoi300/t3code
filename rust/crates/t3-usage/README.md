# t3-usage

Native, transport-neutral usage reporting. Provider files are opened read-only;
durable writes stay in the explicitly supplied T3 base/state directories, and
SQLite decoding uses private temporary copies. No home
directory or process environment is read implicitly. This crate does not target
Wasm and does not spawn processes or start a server.

## API

- `UsageService::{new,with_dependencies,read_summary,read_summary_with_settings,refresh_rates}`:
  async contract-typed summaries and pricing snapshots. `ServiceConfig` supplies
  base/state/home paths, host identity, source directories and resolved provider
  instances. `UsageSettings` supplies aliases and price overrides per request.
  Equal concurrent requests share a detached scan; different requests serialize
  cache mutation. Changed cache persistence is awaited without holding the state
  lock. Failed publications remain dirty and retry on the next read.
- `reader::{read_transcript,list_transcripts}`: streaming JSON projection, plain
  and zstd JSONL, tentative tails, FNV-1a guards and resumable reducer state.
- `database::{read_opencode,OpenCodeCache,read_antigravity}`: read-only SQLite and legacy JSON
  readers. SQLite projects OpenCode usage fields before returning message data.
  Antigravity decodes metadata protobufs, retries, model evidence and shared IDs.
- `aggregation::{UsageAggregator,resolve_model_aliases,canonical_model_filter}`
  and `pricing::*`: period/model/source buckets, channel-max deduplication,
  pricing tiers, cache savings, category costs and speed premiums.
- `cache::{decode_scan_cache,write_scan_cache,prune_scan_cache}`: TypeScript v16
  disk format, intern tables, legacy invalidation and retained deleted history.
- `limits::*`: contract windows, reset times, Codex/Claude streamed updates,
  provider response conversion, merge/freshness rules and probe interfaces.
- `UsageLimitSources::{current,subscribe,refresh,consume_reset_credit}`: serialized
  hub refresh/redemption with change subscriptions. The server owns scheduling.
- `sources::{resolve_sources,resolve_codex_history}`: provider path rules and the
  first V2 running selection's historical Codex tier. Uncertain evidence stays
  unresolved. `Error` is the public typed failure enum.

## Sources and persistence

| Provider | Local source |
| --- | --- |
| Claude | configured home, `CLAUDE_CONFIG_DIR`, or `<home>/.claude`; `projects/**/*.jsonl[.zst]` |
| Codex | configured shared home, eligible `CODEX_HOME`, or `<home>/.codex`; `sessions/**/*.jsonl[.zst]` |
| Grok | `GROK_HOME` or `<home>/.grok`; `sessions/**/updates.jsonl` only |
| Pi | supplied launch `--session-dir`, plus `PI_CODING_AGENT_SESSION_DIR`, or `PI_CODING_AGENT_DIR/sessions`, or `<instance-home>/.pi/agent/sessions` |
| Muse | discovered Muse home; `sessions/**/session.jsonl` only |
| OpenCode | comma-separated `OPENCODE_DATA_DIR`, or absolute `XDG_DATA_HOME/opencode`, or `<home>/.local/share/opencode`; `opencode[-suffix].db`, `storage/message/**/*.json` |
| Antigravity | comma-separated `ANTIGRAVITY_DATA_DIR`, otherwise `.gemini/{antigravity,antigravity-cli,antigravity-ide,antigravity-backup}` and `.config/antigravity` under supplied home; optional resolved instance profile `antigravity-acp`; nested `conversations` when present |
| Cursor | account API via `ProviderUsageProbe::scan`; no local transcript format |

Writes are `<base>/usage-host-id`, `<state>/usage-model-rates.json` and
`<state>/usage-scan-cache-v16.json`. Cache version 4–16 history is understood;
pre-v13 Claude and incompatible Codex reducers force reparsing. The service keeps
92 days of cached file history, uses 36-hour mtime slack, limits hourly windows
to 24 hours, and reads at most four transcripts concurrently. Live files retain
walk order before deleted cached files. Successful persistence removes legacy
cache filenames and stale scan-cache/rates temporary directories after one hour,
once per service lifetime. Unchanged reads do not rewrite either document. Rates
are saved only after a successful fetch; each fetch has a ten-second timeout and
runs outside the state lock.
Sources retain their saved volume identity while retained usage depends on it.

Codex cache entries before v16 retain totals but cannot resume a grown transcript.
Every baseline movement is dated; re-emitted older or same-time lower snapshots
retain the current baseline. OpenCode caches projected rows by database identity,
table, rowid and update/size markers; fork lineage deduplicates copied replies
without dropping replies after the fork boundary. Database work runs on the
service's blocking worker. `cache::{CursorAccountCache,decode_cursor_account_caches,
write_cursor_account_caches}` supplies the interoperable v2 Cursor account format;
the authenticated account adapter owns loading and publishing
`usage-cursor-account-cache-v2.json` and must reject personal-only v1 caches.

Provider input is bounded: 16 MiB per JSON line, 64 nesting levels, 20,000 JSON
structural nodes or protobuf fields, 4 KiB per selected string, 256 MiB per file,
20,000 records and 32 MiB retained data per file. A scan shares a 1 GiB byte
budget, a 128 MiB retained-data budget (including existing cache), a 30-second
work deadline and bounded directory walks (20,000 entries, 64 levels). Zstd
decoders cap their window at 8 MiB and decoded bytes at the smaller of 256 MiB
and the larger of 8 MiB or 200 times compressed size. Plain reads stop at their
initial file length, including when another process appends continuously.
Oversized/deep lines are skipped; file/scan budget failures retain prior usage
and report counts, without logging transcript contents. Links and non-regular
files are excluded. Invalid UTF-8 uses Node's replacement decoding.

SQLite readers copy bounded main and WAL files into a private temporary directory
using ordinary shared read handles. They reject active rollback journals and
copies whose source size/mtime changes, then close all source handles before
opening SQLite on the copy. SQLite recovery, transactions, WAL/SHM files and
locks affect only that copy. Queries use `query_only`, `trusted_schema=OFF`, a
16 MiB value limit, and a five-second/ten-million-VM-operation ceiling within
the scan deadline. This trades copy/hash I/O and potentially skipped busy
databases for keeping the owning application's writes, checkpoints and renames
independent of decoding. Main/WAL content hashes allow Antigravity parse reuse
on Windows and detect changes even when mtimes are restored.

## Runtime integrations

`runtime::UsageRuntime::new(UsageRuntimeInputs)` assembles the shared scan service,
bounded rate fetcher and hub-source snapshots from explicit host dependencies.
`ProviderUsageDiscovery` supplies coherent safe provider statuses and private
resolved history inputs through a watch receiver; scans capture the current
inputs once, including in concurrent-request identity. Config subscriptions emit
an initial snapshot and typed provider/source changes. `UsageEndpoint` captures
the effective origin for a turn's later reset fallback. The host owns refresh
scheduling, authentication and subscription lifetime.

`http::RateHttpClient` supplies streamed responses to `BoundedRateFetcher`.
The fetcher bounds the whole exchange to ten seconds and 8 MiB, rejects non-2xx
responses and oversized/deep JSON, and parses on a blocking worker. Tests inject
HTTP responses; they never contact a rate endpoint.

`RateFetcher` supplies the two LiteLLM URLs; `Clock` makes freshness deterministic.
`ProviderUsageProbe` supplies Muse read-only SDK home discovery, Cursor account
scans, and optional Codex historical tiers (the SQLite helper can supply these).
It distinguishes pending Muse discovery from an unavailable installation.
`UsageLimitProbe` supplies authenticated provider reports; `LimitSourceProbe`
supplies CLIProxy hub accounts and reset-credit redemption. All are tested with
fakes. Provider-limit and hub requests have a crate-owned fifteen-second timeout;
raw probe failures are replaced with fixed messages so credentials or response
bodies cannot escape through errors. Adapters own credentials, endpoint
selection, transport limits and CLI execution;
any eventual process adapter must use `t3-process`.

Claude limits need its SDK usage request; Codex limits need app-server account
requests/notifications; Cursor history/limits, Grok subscription limits,
OpenCode Go limits and Antigravity quotas need network APIs; Muse discovery and
subscription reports need its SDK. Pi has no subscription-limit probe here.
Those authenticated transports, Cursor account pagination/cache, provider
credential stores, and application wiring remain outside this crate. The
CLIProxy HTTP/auth implementation in `apps/server/src/usage/cliproxyApi.ts` is
represented by `LimitSourceProbe`, rather than duplicated.

## Parity and validation

Behavior is ported from `apps/server/src/usage/{UsageService,usageTranscriptReader,
usageAggregation,usageScanCache,usagePricing,CodexUsageHistory,UsageLimitSources}.ts`,
`apps/server/src/provider/Drivers/*Usage*.ts`, provider usage modules under
`packages/provider-{grok,pi,muse,opencode}/src/server`, and provider limit helpers.
The four requested TypeScript test suites supply reader, aggregation, cache and
service cases. Tests include real read-only SQLite/WAL files and synthetic
transcripts; they never use real provider directories.

`tools/generate-fixtures.mjs` reuses the TypeScript test data shapes and executes
the actual TypeScript functions to generate the checked-in golden expectations.
Run from the repository root with Node 26 and installed repository dependencies:
`node --experimental-strip-types rust/crates/t3-usage/tools/generate-fixtures.mjs`.
Rust tests need only the checked-in fixtures. No local wire types need moving to
`t3-contracts`; records, configuration and reducer types are internal equivalents
of provider-core service types. Output types come from `t3-contracts`.

Known differences: independent windows serialize instead of overlapping scans;
persistence is awaited instead of detached. Cache token counters must be
nonnegative safe integers: TypeScript's `usageScanCache.ts` accepts any finite
number, but its public `UsageTokenTotals` contract (`packages/contracts/src/usage.ts`)
requires `NonNegativeInt`, matching Rust's `u64` fields. Negative/fractional cache
values are therefore rejected rather than published as invalid contract data.
The explicit safety budgets above also apply to cached documents (64 MiB,
two million structural nodes), so exceptionally large inputs can be skipped.
Date parsing accepts RFC3339 and ISO dates rather than all JavaScript `Date.parse`
spellings. Bucket and limit-window sorting uses ICU root collation (matching the
original's default `en-US` ordering); a TypeScript process configured with a
different locale can still sort tailored characters differently. Equal bucket
sort keys retain insertion order across sources. Time zone names ignore ASCII
case. Atomic writes retry rename three times, warn once on failure, and sync
the parent directory on Unix; cache-file symlinks are replaced by regular files.
Hub account refreshes are serialized rather than four-way concurrent.
Source snapshots expose HTTP(S) origins. `UsageLimitSources::reset_at` refreshes
under a five-second bound, selects the failed turn's endpoint and driver/model,
and returns the earliest account recovery only if every matching account is spent.
Claude enabled $0 extra-usage budgets expose `zeroSpendCap` without a percentage.
POSIX behavior has not been run on this Windows host.

The TypeScript aggregation test "splits a bucket's cost by category and speed"
has inconsistent numeric assertions: its categories and implementation total
`0.013875`, but its assertion expects `0.016875` (its speed assertions differ
too). The generated golden and Rust port follow the actual TypeScript output:
fast cost `0.00925`, speed premium `0.004625`.

From `rust/`, with `CARGO_TARGET_DIR` set to the assigned agent build directory:
`cargo fmt -p t3-usage`, `cargo clippy -p t3-usage --all-targets -- -D warnings`,
`cargo test -p t3-usage`.
