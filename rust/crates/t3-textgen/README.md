# t3-textgen

Native, transport-neutral generated text and voice services. This crate is not a
`wasm32-unknown-unknown` dependency. It owns the ports of
`apps/server/src/textGeneration/`, `apps/server/src/speech/` and their pure helpers
in `packages/provider-core/src/server/textGeneration*.ts` and
`threadTitleContext.ts`.

## Host API

- `TextGeneration::generate` takes `GenerationContext`, a `GenerationInput`, an
  optional `RoutingState` snapshot and a cancellation token. Its six operations
  return typed `Generated` results. `generate_thread_title` returns the title and
  refinement flag; its `ThreadTitleInput` includes attachments, previous title,
  project title/instructions, inherited fork history and resolved link context.
  An orchestration `TitleGenerator` adapter can map the existing richer seam to
  it without losing fields. The optional `orchestration-host` feature supplies
  that adapter; the core API does not require orchestration.
- `TextRunner::run` is the provider seam: prompt, cwd, model selection, structured
  output schema, attachments and optional read-only investigation deadline in;
  text or a content-free `GenerationError` out. The host dispatches by selected
  instance, applies the installation gate and supplies current settings/usage
  snapshots. Dropping a runner future must stop its provider-owned work.
- `ApiTextRunner` implements the configured OpenAI-compatible and Anthropic HTTP
  generators. `GeneratorSecrets` resolves saved credentials. `ModelCache` caches
  by the complete configuration, bypasses on refresh/unsaved/test requests, and
  sends the tiny explicit connection-test prompt. Hosts resolve a redacted
  unsaved key against its saved configuration before constructing the runner.
- `prompts`, `text` and `title_context` expose the pure builders, sanitizers,
  bounded role-labelled context selection and two-link source-control lookup.
  Supply `TitleLinkResolver` from the authoritative forge registry.
- `post_processing::post_process` runs dictation cleanup in an empty temporary
  directory, supplies only the transcript/composer context and requested
  dictionary, and preserves the original transcript on empty generated output.
- `SpeechService` owns local speech selection, preparation, inference, exclusive
  stream/model operations, idle unloading, detached/coalesced downloads,
  cancellation, removal and linking. Supply `SpeechSettingsBackend`, an explicit
  `ModelStore` root and `SpeechEngineFactory`. `SpeechProtocol::receive` accepts
  commands/binary frames in arrival order and produces canonical contract events.
  Call `SpeechStream::close().await` for deterministic release; `Drop` starts
  bounded reset/cleanup if the owner disconnects.
- `CloudSpeech` uses `CloudCredentials` to authorize each upload afresh, rechecks
  credential identity immediately before send, and exposes the OpenRouter model
  catalog. `cloud::wave`, `speech::decode_request` and `speech::decode_pcm` are the
  upload/framing codecs.

The HTTP/WebSocket host still owns authentication, scope guards, body/frame
limits *before collecting request data*, the two-frame bounded receive queue,
the ten-minute socket lifetime, close codes and hiding `linkedFrom` from callers
without filesystem-read scope. Feed the protocol serially and close its owner
on disconnect. No HTTP server or provider processes are started on import.

## Wiring the runtime's title effect

Enable `t3-textgen`'s `orchestration-host` feature in the server host. Supply the
provider/API registry as `TextRunner`, and implement `TitleRoutingSource` over
the current settings/provider/usage snapshots. The routing source receives the
destination project ID: populate `project_enabled` from that project's settings,
preserve configured instance insertion order, and distinguish a pending probe
from a failed installation/auth probe. It is read anew for each generation;
the requested model remains the one selected by the orchestration title service.

The host dependency is `t3-textgen = { path = "../t3-textgen", features =
["orchestration-host"] }`. Keep that integration in the host; the optional
adapter depends on orchestration, so orchestration must not depend back on this
crate.

Before `OrchestrationRuntime::start(ports)`, install:

```rust,ignore
use std::sync::Arc;
use t3_orchestration::composition::OrchestrationRuntime;
use t3_textgen::{TextGeneration, host_titles::HostTitleGenerator,
    orchestration::OrchestrationTitleGenerator};

let titles = HostTitleGenerator::new(
    Arc::new(TextGeneration::new(text_runner)),
    title_routing_source,
).with_links(forge_title_link_resolver);
ports.titles = Some(Arc::new(OrchestrationTitleGenerator::new(titles)));
let runtime = OrchestrationRuntime::start(ports).await?;
```

The three injected values are respectively `Arc<dyn TextRunner>`,
`Arc<dyn TitleRoutingSource>`, and `Arc<dyn TitleLinkResolver>`. The forge resolver
must use the host's authoritative source-control registry. Omit `with_links`
only when that host has no source-control lookup implementation. The existing
`ApiTextRunner` is a real runner for a configured model API; native provider
runners still belong to their adapters and must enforce their installation gate
and restrictions.

The adapter implements the actual `t3_orchestration::api::TitleGenerator`.
`generate_with_input` maps every generation field, including project/worktree,
attachments, prior title, fork title/history, project instructions and model
selection. `HostTitleGenerator` then resolves routing, enriches at most two
recognized source-control links when context was not already supplied, and runs
the existing prompt/parser. Cancellation covers settings/usage reads, forge
lookups and inference; cancellation maps to orchestration `Error::Cancelled`,
other errors to a content-free `Error::Service`. The runtime owns effect receipts,
initial retries, stale-request guards, rename/unchanged outcomes and shutdown;
the adapter does not dispatch another command or mutate a projection itself.

This plugs into the `RuntimePorts::titles` row in
[runtime composition](../t3-orchestration/README.md#runtime-composition).
The runtime calls the rich input method. Older callers of `generate(thread,
history, token)` must also supply `with_legacy_context(Arc<dyn
LegacyTitleContextSource>)`: that older signature cannot identify cwd, settings
or project by itself. The resolver supplies only missing metadata; the adapter
bounds the supplied history with the existing title-context formatter, and never
guesses a cwd/model or reloads additional conversation text. `needsRefinement`
remains available on `HostTitleGenerator`'s result; the current orchestration port
returns only the title, as its TypeScript consumer does.

## Compatibility and privacy

`tools/extract-prompts.mjs` executes the original pure TypeScript builders using
Node's type stripper and inert schema shims. It extracts 50 templates rather
than retyping any model-facing prose. `fixtures/parity.json` contains executed
TypeScript prompt/correction examples; Rust compares its composed prompts and
dictionary/filler processing against them. `--check` also detects source drift.
The catalog is copied verbatim and compared against the TypeScript source.
Only fictional messages and public source text are in fixtures.

Generation preserves the TypeScript limits: commit summary/patch 6,000/40,000;
PR commits/stat/patch 12,000/12,000/40,000; PR template/reference 8,000/20,000;
additional instructions 20,000; branch message 8,000 and attachment metadata
4,000; title message/fork history 8,000 each; review description/settled review
8,000 each and preferences 4,000. Limits count UTF-16 code units. Initial titles
retain both message ends; regenerated titles retain recent contents. System and
reasoning messages are excluded from title history; at most four selected
attachments remain. The provider seam receives attachments only for branch and
title generation. Configured HTTP generators send only the composed prompt.
Normal provider requests time out at three minutes, API generators at two,
review investigation at fifteen, model listing at 25 seconds, linked lookups at
three seconds. There is no retry to another provider after a request failure.

`RoutingState` preserves default-driver fallback order (Codex, Claude, Cursor,
Grok, Muse, Pi, OpenCode, Antigravity), then settings insertion order, current
custom default models, project enablement, failed probes and exhausted usage.
Pending probes are not treated as failed installations. Review fallbacks require
Codex/Claude; API generators reject investigation. Driver defaults come from
`t3-contracts::model`, not a second model catalog. Native provider adapters still
own account-specific model aliases/fallbacks, options, CLI protocol, sandboxing,
tool restrictions and actual process execution from the TypeScript Codex/Claude/
Antigravity runners. They are intentionally not duplicated here.

Output decoding extracts the first balanced JSON object, including fenced or
prefaced replies, validates fields and applies the TypeScript sanitizers. Blank
subjects/titles retain the existing fallback strings; literal `title` is
rejected. Refusals that do not satisfy the schema fail, rather than becoming
titles. There is no newly invented refusal-text classifier. Commit limits
preserve grapheme clusters, branch normalization uses NFKD, and title truncation
does not split surrogate pairs. Provider request/reply bodies, filesystem
contents and credentials are absent from errors and logs. This implementation
also suppresses upstream API error bodies, which TypeScript may display.

## Speech engine and model files

The TypeScript dependency is **transcribe-cpp 0.4.1**, not whisper.cpp. Its
`catalog.json` includes multiple GGUF ASR families; the default is
`handy-computer/parakeet-unified-en-0.6b-gguf` (731,357,568 bytes). Substituting
whisper-rs would not load that model or preserve native streaming/vocabulary.
This crate instead uses the same engine's maintained, MIT-licensed
[Rust binding](https://github.com/handy-computer/transcribe.cpp/tree/main/bindings/rust/transcribe-cpp),
pinned along with `transcribe-cpp-sys` to **0.4.1**. Pinning sys prevents Cargo
silently selecting a different native-engine patch release. The registry
archives measured here are 72,893 and 4,479,034 bytes; the Windows CPU worker in
the workspace's no-debug development profile is 4,980,224 bytes (build-dependent).
No model is bundled or downloaded by the build.

Enable `native-speech` and build `t3-speech-worker`, then pass its absolute path to
`NativeSpeechFactory`. The safe Rust binding builds the C++ engine from source;
the [upstream prerequisites](https://github.com/handy-computer/transcribe.cpp/tree/main/bindings/rust/sys)
are a C++ toolchain and CMake, with vendored compression and generated FFI (no
zlib/vcpkg/libclang setup). On this Windows machine those tools were already
installed. CMake defaulted to unsupported `Visual Studio 18 2026`; setting
`CMAKE_GENERATOR=Visual Studio 17 2022` for the build used the installed toolchain
successfully. CPU builds need no new system installation. GPU builds require
their upstream SDKs and are not enabled or validated here. macOS/Linux builds
have not been exercised on this Windows host.

The native engine runs in a separate owned process via `t3-process`. Bounded
length-prefixed IPC, cancellation/drop and failure terminate that process rather
than destroying native objects during in-flight inference. Native logging is
disabled. The worker implements batch and native/buffered streaming, Silero VAD,
pre-roll/onset/tails, pause-aware bounded chunks, raw replay on VAD failure and
bounded recursive retries for truncated/overlong ASR input. Auto GPU inference
can replay once on CPU; explicitly selected devices never silently switch.

Local files live at `<stateDir>/speech/models/<catalog filename>`, with `.verified`
and `.source` sidecars. Downloads use pinned
`https://huggingface.co/<id>/resolve/<revision>/<filename>` URLs and verify exact
size and SHA-256 before atomic replacement. Three attempts resume transport
failures using validated HTTP ranges. Existing Hugging Face/Handy copies and
explicit links are verified; removing a link preserves its external target.
Startup cleans only UUID-named GGUF partials. The Silero helper is the same
1,241,056-byte pinned v6.2 model as TypeScript; acquisition is optional, bounded
to 15 seconds during recording/120 during installation, with a ten-minute
recording failure cooldown.

Input is mono **16 kHz little-endian Float32 PCM**, finite samples in [-1,1],
nonempty/aligned and capped at five minutes (19,200,000 bytes) before decoded
allocation. HTTP framing is a little-endian four-byte JSON-options length,
at most 128,000 UTF-8 bytes, then PCM. Stream chunks are at most 64,000 bytes;
the total recording cap also applies to replay buffers. Contract events are
`ready`, monotonically revised `update` with nullable committed/tentative text,
`finished`, and sanitized `error`. Options preserve language spelling advertised
by the native model, translation eligibility, vocabulary/aliases, filler removal,
correction cues and silence filtering; settings preserve acceleration and all
seven unload policies.

Cloud dictation matches `CloudSpeech.ts`: OpenAI
`/v1/audio/transcriptions` with `gpt-transcribe`; xAI `/v1/stt` with
`grok-voice-transcribe-2.0`; OpenRouter `/api/v1/audio/transcriptions` only for an
exact model from its transcription/ZDR catalog. WAV is PCM16. xAI's file is last
in multipart; OpenRouter receives base64 WAV and `provider: {zdr:true}`. Requests
have no redirects, tracing propagation or translation beyond the TypeScript
payloads. Cloud calls time out at two minutes; successful public catalog reads
cache for ten minutes, failures do not. API keys remain the host vault's concern.

## Boundaries and known differences

- This is a library/worker port, not the server composition root. The tested
  optional orchestration adapter is ready for `RuntimePorts::titles`; the server
  owner still installs it with the real runner, routing source and forge resolver.
  Server routes and secret/settings stores likewise inject their services. No
  files outside this crate were edited to wire those integrations.
- No acoustic/model inference or real provider turn was run: tests must never
  fetch models or call an external API. Windows native compilation, native
  discovery and worker error IPC are exercised with no model. GPU SDK packaging
  and acoustic parity remain unverified, not replaced with a fake production
  implementation. Paid model evals were not run; prompt bytes were preserved.
- Verification markers use a Rust file-identity fingerprint, not Node's textual
  stat tuple. Each implementation safely rehashes the other's marker once.
- API JSON replies have an 8 MiB bound; TypeScript does not impose that bound.
  API model sorting uses ICU root collation, avoiding Rust byte ordering.
- Pathological UTF-16 slicing in regenerated titles, link excerpts and composer
  selections is moved to scalar boundaries; Rust cannot hold lone surrogates.
  Locale-sensitive casing follows Rust Unicode casing rather than the host's
  JavaScript locale. Review results reuse the canonical contracts, which reject
  invalid positive line numbers/blank branded strings earlier than the loose
  provider output schema. These differences do not expand transmitted context.
- `SpeechProtocol` is transport-neutral, so auth/status-code mapping and real
  socket close/pipelining tests belong to the host adapter. It deliberately does
  not create a second WebSocket server implementation.
- `prompts::BranchNaming` is the local equivalent of TypeScript's
  `BranchNamingOptions`, which is absent from the Rust contracts. **To move to
  t3-contracts** when that contract is added; its semantic/static/custom serde
  shape is preserved here.
- Generation input/policy structs, provider routing snapshots and speech
  engine/worker structs are internal service types. Wire inputs/events/settings,
  attachments, model selections and review stops use `t3-contracts`.

## Verification

From `rust/`, with the assigned `CARGO_TARGET_DIR` and the shared Cargo job limit
unchanged:

```powershell
cargo fmt -p t3-textgen --check
cargo clippy -p t3-textgen --all-targets -- -D warnings
cargo test -p t3-textgen
$env:CMAKE_GENERATOR='Visual Studio 17 2022'
cargo clippy -p t3-textgen --all-targets --features native-speech -- -D warnings
cargo test -p t3-textgen --features native-speech
cargo clippy -p t3-textgen --all-targets --features orchestration-host -- -D warnings
cargo test -p t3-textgen --features orchestration-host
```

Tests port behavior from the TypeScript prompt/context/operation/utils,
text-generator API, speech audio/model/language/service/store/cloud/stream
suites: executed-source golden prompts and correction corpora, fake provider
timeouts/cancellation, Unicode boundaries, routing, real loopback HTTP request
shapes/cache/pagination, temporary-file hashing/linking/resumed transfers,
stream ownership/replay, paused-clock idle unloading and native-worker IPC.
All credentials, audio and model files in tests are synthetic and temporary.

Windows verification, 2026-11-13: formatting and Clippy with `-D warnings` pass
in both configurations; 22 default tests and 23 native-speech tests pass. The
executed TypeScript corpus covers 100 prompts, 11 dictionary cases and 48 filler
cases. Native runtime transcript: CPU device discovery succeeds with no GPU
devices; loading a nonexistent temporary model returns `SpeechError::Inference`
without exposing the path or native error text. No model download or external
API call was made by verification.

The host-title continuation also tests current project routing, enriched rich
inputs, cancellation during routing/forge reads, the real optional seam, and a
composed runtime with migrated temporary SQLite. The runtime proof dispatches a
title request through the real effect worker, persists the generated title with
project settings, and verifies shutdown cancels a hung runner without completing
the pending title marker. Its model runner is a fake; no paid turn is started.
