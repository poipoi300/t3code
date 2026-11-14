# ACP and replay package status

Package 12 has 176 populated and 6 empty mapped leaves. Native provider and
host-composition seams still block source-complete integration suites. Nonempty
mapped files are not automatically complete ports. The exact 182-row audit is external at
`C:/Users/Administrator/t3code-rust-notes/briefs/orch-e-remaining.md`.

## Registering Replay in the composed runtime

Build with `--features acp-adapter`. Add
`adapters::acp::dev_replay_registration(clock.clone(), ids.clone(), true)` to the
`Vec<AdapterRegistration>` passed to `AdapterRegistry::new`, then supply that
registry as `RuntimePorts.providers`. The helper registers the canonical
`devReplay` instance, driver `acpRegistry`, enabled, without credential gates or
an external process. Pass `false` for delays in deterministic tests.

Configure the same instance in host settings as
`{"driver":"acpRegistry","displayName":"Replay (dev)","enabled":true}`.
Use `{"instanceId":"devReplay","model":"multi-turn"}` for the composed-runtime
milestone: it completes without an approval answer in supervised mode.
`showcase`, `todo-list`, `approval`, `question` and `multi-turn` are the available
models, also exposed by `testing::replay::DEV_SCENARIOS`. Showcase deliberately
asks for permission in supervised mode; full-access policy approves it.

The provider implements the normal ensure/resume/start/subscribe/interrupt/close
seam. Restore the existing native thread ID and identity marker through
`OpenSession` and pass the persisted provider thread to `resume_thread` after a
restart. Replay reloads that native ID; orchestration retains the durable
conversation history. This matches the TypeScript dev agent's restart behavior.

## Verified continuation work

- Replay registration, complete turn, streamed text, approval interrupt, native
  thread serialization, new registry/session and resumed next turn all pass
  through the real registry and ACP seam. This is adapter restart recovery;
  the integrator owns the composed durable-runtime milestone.
- A registry lifecycle regression found stale model selection: the ACP client
  does not update its negotiated model cache after `set_model`. The adapter now
  remembers successful selections and current-model notifications. Switching
  from multi-turn back to showcase now selects the correct recording.
- All 70 mapped fixture-input modules are ported: 76 builders and 35 constants
  compare to the actual TypeScript exports. All 76 materialized command lists,
  IDs and ordered barriers compare exactly to TypeScript. The oracle includes
  193 input/shared exports. Registry metadata matches all 97 scenarios and
  160 provider variants, independently of the 171 raw recording files.
- Deterministic fixture IDs reproduce Effect's ISAAC random UUID sequence for
  seeds 0, 1, 0x12345678 and 0xffffffff across generation boundaries. The clock
  begins at Effect TestClock's Unix epoch. Fixture metadata allocates canonical
  derived IDs with the same random consumption order.
- Runtime-instruction rewriting matches the TypeScript algorithm on all 171
  source recordings, including model changes, prompt success/failure, slash
  compaction, explicit compaction updates and Kiro compact responses. Its
  authoritative text builder is injected from the provider-core host.
- Cursor's SDK replay runner ports strict normalized outbound frames, dynamic
  handoff-summary normalization, streamed deltas, wait, steer acknowledgments,
  cancellation, messages, close, sticky failures and completion checks. A real
  recorded simple turn plus concurrency/error tests pass. Seven Cursor-specific
  projection assertion modules are ported; their native integration is waiting
  on the provider-Cursor factory.
- OpenCode SDK/SSE replay ports subset/wildcard matching, message-ID binding and
  reference retargeting, concurrent response/event ordering, gates, cancellation,
  delays, runtime exits and completion. Real simple recording and ordering tests
  pass. OpenCode2 HTTP routing matches the real TS `operationOf` over 180 route/
  method combinations. Native HTTP response/connection adaptation is a named seam.
- Pi and Muse use an in-memory JSONL controller that holds reordered writes
  within the current turn, respects process/host ordinals, rebinds client request
  IDs and preserves Muse server-request IDs. All 8 Pi + 8 Muse recordings consume
  completely. Restart, turn-boundary rejection, UTF-8 chunk splitting, Muse command
  minting and workspace substitution have focused coverage. These do not implement
  native provider mapping or launch a process.
- Shared projection assertions include root/run/provider/item/request foreign
  references, ordered/visible items, statuses/text/roles/labels, and native-child
  root activity and nested wake ordering. All 68 mapped output modules
  (72 assertion entry points) are translated and
  wired through output-name dispatch. They need native integration validation
  when the requested factories are provided. The shared base assertion accepts
  positive, strictly increasing application-log sequences with gaps and preserves
  exact stored/domain event counts and IDs; the old contiguous-from-one source
  assumption is incompatible with project events and setup watermarks.
- Scenario runner now supports materialized fixture steps, held finish receipts,
  child-item waits, shell background-work waits, reached gates and targeted
  background clearing, event-observed session release, relative clock movement,
  and deterministic insertion-order dispatch error collection. No wait polls.
- CommandOutputRecorder records before forwarding append, counts chunks and
  clones snapshots using an injected immutable terminal fold. `run_with_command_output`
  attaches its snapshot. The concrete transport CommandOutputHub now implements
  the append trait; shared ANSI fold and ingestion wiring remain named seams.
- NDJSON decoding trims JavaScript whitespace before parsing, including BOMs,
  and rejects NEL-only lines. The dev player retains its source behavior of
  checking blank lines without trimming the JSON it parses. Both mismatches
  were reproduced by focused regressions before the correction.
- Pull-request shell fixture construction uses canonical contract types and UTC
  date normalization. Both offset-date/optional-link cases match actual TypeScript.

The focused suite contains 49 passing tests, including six actual TypeScript
fixture compatibility oracles plus usage, diff and HTTP-route oracles. The final
verification and package-wide blockers are recorded below. Set `CARGO_TARGET_DIR` to
`D:/t3code-rust-target/orch-e`; the resumed round uses two Cargo jobs. The external focused
harness is `C:/Users/Administrator/AppData/Local/Temp/t3-orch-e-checks-lpp4ep26`.
It includes the actual owned test files against the real library, isolating
unrelated package test compilation failures. No live provider is used.

## Native factory and composition boundaries

Exact requests and marked local shims are under dated Package 12 headings in
`C:/Users/Administrator/t3code-rust-notes/briefs/orchestration-seam-requests.md`.
The original brief excludes per-provider packages from this checklist.

- Cursor: `CursorAdapterFactory` injects the SDK replay runner into its native
  adapter, with isolated HOME. Cursor recording entry point also needs that SDK.
- OpenCode: `OpenCodeAdapterFactory` injects the SDK controller into an external
  in-memory server connection. OpenCode2 uses `OpenCode2AdapterFactory`, its
  native client/HTTP response conversion, hooks and borrower/process lifetime.
- Grok/Kiro/fx: `AcpFlavorAdapterFactory` preserves the strict injected ACP
  transport and implements vendor policy/mapping. Grok needs initialize/cancel
  metadata and deferred completion; fx needs cancel behavior/deferred receipts;
  Kiro needs compact/model/effort/permission/fork/subagent behavior. Their mapped
  testkits reject wrong drivers and call the factory; they are not renamed
  generic ACP adapters.
- Pi/Muse: native RPC/SDK adapter factories inject the owned JSONL controller.
  Muse needs initialize/initialized handshake and granted-capability mapping.
- Command-output recorder: the concrete transport CommandOutputHub append bridge
  is connected. Shared ANSI screen fold/normalizer/plain-text operations remain
  an explicit host dependency. The client raw-chunk terminal
  buffer is a different implementation and cannot substitute for that fold.
- Full replay fixture harness: `scenario_services_for_runtime` now uses admitted
  dispatch and the real composed worker. `start_fixture_runtime` calls the marked
  `ReplayFixtureHostFactory` for production checkpoint/workspace/policy/attachment/
  continuation bindings and settings. Native factories and the shared ANSI fold
  remain required; fixture metadata and owned output dispatch are already ported.
- Codex native replay is now supplied by package 10. Its full mapped fixture suite
  still needs the host binding above and provider-batch transaction observations
  for the source assertion of 6 writes, with no batch larger than 3.

## Generic ACP implementation and remaining gaps

`adapters/acp.rs` implements ProviderAdapter/SessionRuntime over injected ACP
transport and client-effects seams, with ordered notification acknowledgment,
permissions using preceding tracked tools, live approvals/questions, model
selection, text/tool/plan events and interrupt. RejectClientEffects rejects all
client filesystem/terminal/extension effects by default. `testing/replay.rs`
provides NDJSON decode/matching, progress, gates, runtime exit cursors and dev
recording semantics without requiring ACP types in the default build.

The adapter now maps declared prompt usage to final provider turns, including
cache/reasoning clamps and unavailable usage; the separately exported helper
also reproduces Gemini counter conventions. Snippet/omitted/native-hunk diff
preview rules and bounded formatter calls match the actual source/jsdiff oracle.
The native old/new-text diff engine is the explicit `AcpDiffFormatter` boundary.

The generic adapter remains incomplete for native history projection,
attachments, full update/context-usage/compaction mapping, option/reasoning policies,
runtime instruction delivery, vendor extensions and capability negotiation.
Snapshots reflect in-process observed messages. The fixture registry has all
171 raw recordings, typed inputs, complete source registry metadata, and all
owned output assertion dispatch. Full native scenario execution is not ported
without the named native/host bindings. Check the audit instead of treating
populated modules as completion.

The takeover inherited the adapter, replay player, raw index, basic scenario
runner, coalescer and 13 tests. Earlier continuation added SQLite projection
replay verification/rebuild (paged logs, rollback, position recovery), strict ACP
harnesses and permission/identity regressions. Replaced inherited incorrect
sandbox-type fallback, writable-root containment, legacy item-identity restore,
scenario cancellation cleanup and JavaScript numeric matching. No other owner's
files were changed.

Provider order confirmed by the coordinator: generic ACP/registry, Cursor,
OpenCode, Grok, Kiro, Pi, Muse, fx. The six empty leaves and every partial suite
are individually identified in the external audit; the Claude leaf is reserved
to its concurrent owner. No controller test is counted as full native integration.

Known source behavior retained: OpenCode response/event waits at transcript EOF
wait for advancement/cancellation; Pi/Muse runtime-exit status is ignored by their
controllers; Cursor recording does not bounds-check steer index (native recording
entry point still waits on its SDK boundary). Muse/OpenCode2 permission tests
use a comparator returning 1 for equal timestamps; the Rust assertions preserve
source insertion order for those ties explicitly.

## Latest round (2026-11-13)

Seven previously empty mapped leaves now contain source conformance execution:
fx model/mode scripts; Grok flavor/launch/native questions; fx/Grok/Kiro fixture
tables; Codex table/batch observations; OpenCode2 pending-snapshot evidence.
Native entry points are compiled but await their actual bindings; they are not
counted as passing recorded-provider integrations. The three added executable
tests cover actual-TS fx script equivalence, Grok generic policy and evidence
snapshot rules. The existing registry lifecycle also exercises the bounded,
policy-preserving raw event tap through completion, interrupt and restart.

The common fixture path now creates temporary workspace files and migrated
SQLite, starts the actual registered adapter/composed runtime, creates the
materialized project, runs source output/checkpoint checks, and closes runtime
before checking transcript completeness. The host shim explicitly carries the
continuation-worker flag and provider-batch observer. CodexFixtureFactory binds
the existing real native CodexReplayAdapter under codex-adapter, including strict
completion/shutdown and outbound-only exact workspace placeholders; no fake
native adapter replaces it. ACP's generic bridge preserves its strict transport.

Latest completed focused run: 49 passed, 0 failed, 0 skipped, with acp-adapter and
codex-adapter, under D:/t3code-rust-target/orch-e. Source conformance functions
that require factories remain unexecuted. Verification resumed after the service
exit uses CARGO_BUILD_JOBS=2; package compilation and final Clippy results are
being checked. The earlier http_test.rs:22 blocker is repaired by its owner,
09-transport (explicit EnvironmentId). No package-12 edit was made to that file.

The six empty leaves are KiroAdapterV2.test, the concurrently owned Claude replay
suite, restart-background-note, provider-switch, thread-fork and merge-back.
Kiro now has an exact callback shim for cancel-held/decided/written milestones;
its full native cases still need the native factory and extension mappings.
Exact per-file stopping points and requested owner signatures are external in
orch-e-remaining.md and orchestration-seam-requests.md. Cursor remains first in
the confirmed native-integration order. At the latest inspection its native
factory and the common host factory still have no implementation, so a complete
ACP provider cannot yet be integrated. The authoritative ANSI fold remains an
external dependency; no second implementation is added.

No live provider, live profile, dev server, commit, manifest or frozen seam edit
is used. All types added here are test/harness interfaces rather than replacement
wire contracts. Source quirks and the earlier inherited corrections remain as
recorded above.

## ISAAC redistribution notices

The deterministic UUID generator ports the algorithm in Effect `Random.ts`.

This is a derivative work copyright (c) 2025 Effectful Technologies Inc, under MIT license.
This is a derivative work copyright (c) 2018, William P. "Mac" McMeans, under BSD license.
Redistribution and use in source and binary forms, with or without modification, are permitted provided that the following conditions are met:
1. Redistributions of source code must retain the above copyright notice, this list of conditions and the following disclaimer.
2. Redistributions in binary form must reproduce the above copyright notice, this list of conditions and the following disclaimer in the documentation and/or other materials provided with the distribution.
3. Neither the name of isaacCSPRNG nor the names of its contributors may be used to endorse or promote products derived from this software without specific prior written permission.
THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS "AS IS" AND ANY EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT LIMITED TO, THE IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS FOR A PARTICULAR PURPOSE ARE DISCLAIMED. IN NO EVENT SHALL THE COPYRIGHT HOLDER OR CONTRIBUTORS BE LIABLE FOR ANY DIRECT, INDIRECT, INCIDENTAL, SPECIAL, EXEMPLARY, OR CONSEQUENTIAL DAMAGES (INCLUDING, BUT NOT LIMITED TO, PROCUREMENT OF SUBSTITUTE GOODS OR SERVICES; LOSS OF USE, DATA, OR PROFITS; OR BUSINESS INTERRUPTION) HOWEVER CAUSED AND ON ANY THEORY OF LIABILITY, WHETHER IN CONTRACT, STRICT LIABILITY, OR TORT (INCLUDING NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY OUT OF THE USE OF THIS SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF SUCH DAMAGE.
Original work copyright (c) 2012 Yves-Marie K. Rinquin, under MIT license.
https://github.com/rubycon/isaac.js
