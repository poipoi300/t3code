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

For streaming clients, use `TerminalOutput::new(max_chars)`, `push` (decoded
strings) or `push_bytes` (streaming UTF-8), then `snapshot` when a display frame is
needed. `finish_bytes` flushes an incomplete final UTF-8 character as U+FFFD.
`push_units` also accepts JavaScript UTF-16 chunks. Feeding does not render or
clone retained scrollback. The live cursor window is 64 lines; output, styles,
escape carry and matcher caches are bounded. Immutable `append_terminal_output`
and `NativeTerminalOutputFold` JSON methods preserve the existing orchestration
port interface. The historical `Native*` names mean Rust implementations; these
types also work in Wasm.

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

Verified 2026-11-14: 12 native Rust tests pass, including 2,870 inherited fixture
cases, 95 executed TypeScript source tests / 33,294 differential operations, and
12,378 additional mutable-feed comparisons. Scoped format check, strict all-target
Clippy and `wasm32-unknown-unknown` check pass. Builds use the shared target pool,
the workspace's two-job limit and sccache without a wrapper bypass.
