# t3-shared

Pure helpers needed by both a server and a client, ported from TypeScript
`packages/shared` (and pure terminal-history helpers formerly in contracts).
This library compiles for native targets and `wasm32-unknown-unknown`. It has no
I/O, async runtime, platform shell, environment access or clock reads. Callers
supply text, snapshots, palettes, budgets and timestamps.

Transport handlers, orchestration services, database queries, cache lifetimes,
provider adapters, subprocesses, filesystem operations and platform UI belong
elsewhere. Wire schemas remain in t3-contracts. A helper does not belong here
merely because two server modules use it.

| TypeScript source | Rust module / behavior |
| --- | --- |
| `packages/shared/src/terminalOutput.ts` | `terminal_output`: incremental ANSI fold, bounded UTF-16 tail, cursor/erase/SGR, resynchronization, plain text, palette spans |
| `packages/contracts/src/terminal.ts` (pure functions) | `terminal_history`: sequence-state tracking and UTF-8 history retention outside cut sequences |
| `packages/shared/src/threadFindText.ts` | `find_text`: rendered message/plan search segments and block boundaries |
| `packages/shared/src/threadSearch.ts` | `thread_search`: non-overlapping occurrence counts and UTF-16 offsets |
| `packages/shared/src/paletteSearch.ts` | `palette_search`, `search_policy`: folding, word/phrase matching, highlights, typos, picks, usage/recency, group/title/message ranking |
| `packages/shared/src/threadPullRequests.ts` (search projection) | `search_policy`: visible/newest PR titles, numbers and whole-query references |
| `packages/shared/src/toolOutput.ts`, `htmlRender.ts`, `mcpApp.ts` (bounded metadata used by find) | `tool_output`, `find_text`: bounded envelopes and embedded-result recognition |
| `packages/shared/src/codexMarkdownDirectives.ts`, `codexArtifactTemplates.ts` (rendered find labels) | `find_directives` (private child of `find_text`) |
| `packages/shared/src/composerContextLegacy.ts`, `composerContextReferences.ts` (find visibility) | `find_legacy` (private child), `find_text` |
| `packages/shared/src/assistantCitations.ts`, `fileLinks.ts`, `markdownLinks.ts`, `inlineSkills.ts`, `proposedPlanText.ts`, `markdownPipeline.ts` (find rendering) | `find_text`: citation comments, file/skill labels, plan headings, insight and Markdown boundaries |
| `apps/server/src/orchestration-v2/ThreadSearch.ts` (pure policy only) | `search_policy`: FTS/scan narrowing and displayed-link normalization; no SQL execution |
| `apps/server/src/textGeneration/TextGenerators.ts` (unsaved configurations) | `text_generators::check_supplied_generator`: a placeholder or secret reference reuses a stored secret only for the saved kind at the saved address (a reference only when it is the saved one); any other key goes only to the saved address or an official endpoint. Addresses compare by scheme, host, port and path, trailing `/` ignored, and fail closed. A hardening beyond TypeScript, used by server and client alike |

Regexes keep JavaScript's meaning: `\b` in a pattern without the `u` flag is ASCII
(`(?-u:\b)`), and `\s` is spelled out as JavaScript's class (it includes U+FEFF and
not U+0085) where the corpus reaches it: the legacy context markers and the message
phrase matcher. Other `\s` uses in `find_text`, `find_legacy` and `tool_output` still
use Rust's class.

For streaming clients, use `TerminalOutput::new(max_chars)`, `push` (decoded
strings) or `push_bytes` (streaming UTF-8), then `snapshot` when a display frame is
needed. `finish_bytes` flushes an incomplete final UTF-8 character as U+FFFD.
`push_units` also accepts JavaScript UTF-16 chunks. Feeding does not render or
clone retained scrollback. The live cursor window is 64 lines; output, styles,
escape carry and matcher caches are bounded. Immutable `append_terminal_output`
and `NativeTerminalOutputFold` JSON methods preserve the existing orchestration
port interface, but each call deserialises, copies and re-renders the whole state;
a long-lived host should keep one `TerminalOutput` per stream instead. The
historical `Native*` names mean Rust implementations; these types also work in Wasm.

A line keeps all its units in one buffer and TypeScript's runs as pen and length over
it. A write overwrites units in place, and splitting, merging or flattening runs only
changes lengths, so no write copies text it does not change, as with TypeScript's sliced and
concatenated strings. Carriage-return rewrites, a colour per character and a pen that
alternates in the middle of a 64K-unit line all stay linear in what is written.
`tests/terminal_work.rs` holds this by counting allocated bytes. Measured against
TypeScript under Node, the alternating case costs 3.6 to 4.2 µs per cycle at 1K and at 64K
one-byte units, so it is flat there too.

Run boundaries match TypeScript's exactly (a mid-line write leaves no two neighbours
sharing a pen), because the 2,048-run limit flattens by run count. SGR parameters are
JavaScript numbers: truecolour channels are `Number(...) & 255` (ToInt32), so 2^64 is 0,
not 255.

History byte caps and command-tail UTF-16 caps are separate. Use
`terminal_history::retained_terminal_history` for raw PTY replay. Never apply
the command fold to a PTY that will be replayed into a terminal emulator.

The rendered Markdown implementation uses CommonMark events and a bounded HTML
traversal, rather than the JavaScript HTML5 repair engine. Current source suites
and the recorded renderer cases are tested; arbitrary malformed HTML repair and
unsupported custom Markdown plugins are not a claim of renderer-wide equivalence.
Rust public strings replace unpaired UTF-16 surrogates with U+FFFD; terminal runs
retain the original units so subsequent writes can repair a split pair.

Tests execute the current TypeScript suites under Node, checking their original
assertions and replaying recorded helper calls in Rust, plus deterministic
generated control-sequence, byte-split and Unicode cases. Node and the repository's
installed TypeScript dependencies are required. Static package-09 fixtures also
prove the moved implementation preserves its earlier tests. Test subprocess I/O
is confined to native-only integration tests; the library itself performs none.

Run from `rust/`, with `CARGO_TARGET_DIR=D:\t3code-rust-target\shared`:

```text
cargo test -p t3-shared
cargo clippy -p t3-shared --all-targets -- -D warnings
cargo fmt -p t3-shared --check
cargo check -p t3-shared --target wasm32-unknown-unknown
```

Orchestration adoption is recorded in the shared orchestration seam brief;
server runtime injection and RPC routing are in the server seam brief.

Verified 2026-11-14 (review fixes): 27 native Rust tests pass, including 2,870
inherited fixture cases, 95 executed TypeScript source tests / 40,045 differential
operations (every append also replayed through the mutable feed) and the counted-work
test. Scoped format check, strict all-target Clippy and `wasm32-unknown-unknown`
check pass.
