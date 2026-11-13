# t3-acp

Native-only, process-independent ACP client. The caller supplies any Tokio
`AsyncRead + Unpin + Send + 'static` and `AsyncWrite + Unpin + Send + 'static`
pair. It never spawns processes, touches T3 home, reads files, or grants permissions.

## SDK decision and compatibility evidence

Use **official schemas plus T3 transport**, not a schema port or the SDK's
connection driver. On 2026-11-13, the crates.io API and the Rust SDK repository
both reported `agent-client-protocol` **5.2.0**. Its published manifest requires
Rust 1.88 and exactly `agent-client-protocol-schema` **1.17.0**. This crate pins
5.2.0 with `unstable` and `unstable_protocol_v2` (the latter is separate from
`unstable`). Stable ACP is protocol **1**; protocol **2** is draft, not a new
stable protocol revision. The TypeScript generator separately pins
`schema-v1.21.0` and `schema-v2.0.0-alpha.3`; these schema package/release numbers
must not be confused with the wire's protocolVersion. Builds and tests run on Windows x86_64 MSVC, stable
Rust **1.99.1**.

- [crates.io release](https://crates.io/crates/agent-client-protocol/5.2.0)
- [published manifest](https://docs.rs/crate/agent-client-protocol/5.2.0/source/Cargo.toml)
- [SDK repository](https://github.com/agentclientprotocol/rust-sdk)
- [published SDK batch rejection test](https://docs.rs/crate/agent-client-protocol/5.2.0/source/tests/jsonrpc_batch.rs)

`tests/sdk_transport.rs` actually drives the official SDK with Tokio duplex
pipes through `tokio-util::compat`: arbitrary pipe transport works, but an array
of JSON-RPC requests receives one `-32600` error with null ID. T3's protocol
tests require dispatching all entries and returning a response array. T3 also
requires a mixed-generation initialize payload and shape-based generation
detection, malformed-JSON termination, tolerant frame routing and truncated EOF, and distinct
numeric/string IDs. Numeric correlation follows JavaScript Number equality, including
1 versus 1.0 and signed zero. Our transport retains these contracts.

`tests/schema.rs` checks all five content types, unknown variants, and unknown
fields. SDK structs generally ignore unknown fields on known variants, retaining
`_meta`; v2 `Other` variants retain future payloads, while v1 content rejects
unknown types. The client preserves complete JSON separately from normalized
fields and does not re-encode extension traffic through SDK structs.

### Schema coverage versus effect-acp

| TypeScript surface | Official schema / compatibility handling |
| --- | --- |
| initialize, authentication, new/load/list/fork/resume/close/delete, prompt/cancel | v1 and draft-v2 schemas; load/auth names translated by negotiated response shape |
| set_mode, set_model | v1 schema; draft-v2 exposes config options instead |
| set_config_option, providers/list/set/disable | official schemas; v2 configId/id and boolean/id normalization |
| permission requests | both schemas; draft subject normalized to toolCall without losing original fields |
| fs read/write and terminal create/output/wait/kill/release | official v1 schemas validate inbound requests; effects delegated to ClientHandler |
| message/thought/user chunks and full messages, tool calls/content, plans, commands, modes, config, usage, session metadata, terminal activity, compaction, notices, state | official generation-specific schemas plus raw JSON preservation; legacy variants such as v1 tool_call and plan retained |
| text, image, audio, resource link, embedded resource; tool diff and terminal content | official schemas; raw JSON retains structured v2 diff semantics and future content |
| elicitation/create and completion | official schemas; legacy session/elicitation and _session/elicitation aliases retained with nested action replies |
| mcp/connect, mcp/disconnect | **absent** from schema 1.17; older draft compatibility validates required IDs and dispatches raw payloads |
| mcp/message | schema 1.17 uses serverId/requestId; T3's older draft uses connectionId; both accepted without rewriting |
| nes/* and document/* from generated metadata | no matching official Rust schema; available through raw request/notify, not used by this client's session API |
| vendor methods/notifications, future discriminators and registry env_var authentication | opaque JSON escape hatches; unsupported auth kinds are retained in raw negotiation, not turned into agent login |

This is wire compatibility, not identical validation of every generated
TypeScript field. Notification validation checks framing and the fields consumed
by the client; complete original payloads remain available. Schema changes in
draft MCP are the concrete incompatible schema gap, not just missing feature flags.

## Public API and ownership

- `Client::connect(reader, writer, Arc<dyn ClientHandler>, ClientOptions)` returns
  a cloneable client and one ordered `Events` receiver. Drain events concurrently
  with requests and handle each notification serially. `Events::next` acknowledges
  the previous event; use `Events::acknowledge` after handling the last event before
  awaiting a dependent request. Agent requests and outgoing request completions wait
  for all preceding notifications to be acknowledged, including within mixed batches.
  The event capacity defaults to 4096; a full queue backpressures the reader.
  Dropping `Events` terminates the connection, also releasing handlers retaining a
  `Client` clone. Keep it alive even when using only request methods.
- `initialize` returns `Negotiated { generation, response, raw }`. Authentication,
  session setup/management, prompt, cancel, modes/models and config setters are
  methods on the client. `request`, `request_as` and `notify` expose arbitrary
  methods; `v1::schema` and `v2::schema` re-export the pinned SDK types.
- `ClientHandler::request(ClientRequest, RequestContext)` serves permissions,
  filesystem, terminal, elicitation, MCP and extensions. Payloads are JSON to
  retain draft/vendor fields; context contains the exact wire ID and method.
  `RequestId::identity` supplies collision-free T3 string identities. Core requests
  run concurrently, including batch members, after the notification barrier; extension
  handlers finish inline before routing the next wire message.
  Duplicate in-flight IDs are ignored with a diagnostic, matching Effect
  `rpc/RpcServer.ts:handleRequest` for core ACP methods. Application errors preserve code,
  message and data (including explicit null); a panicking handler gets `-32603`.
- `Events` includes session updates, unknown notifications, invalid-notification
  diagnostics and response-write acknowledgments. Unroutable frames emit a
  payload-free diagnostic; malformed replies fail only their matched request. `SessionUpdate` provides
  `kind`, `is_known`, `is_replay`, normalized `update` and original `raw` params.
  Child-session IDs and replay updates stay distinct for later adapter routing.
- `raw_notifications` is a separate best-effort diagnostic queue, replayable to
  a late reader: newest 32 / 16 Mi UTF-16 characters, always keeping the newest
  entry, with every member of a batch charged its whole line.
- `TextBatcher` emits the first text immediately, coalesces subsequent chunks
  into full snapshots at 50 ms, and flushes before tool/segment/idle boundaries.
  Drive `next_deadline` / `flush_due` with Tokio time. Call `end_turn` after a
  v1 prompt response or cancellation, and `finish_all` at disconnect. Named
  messages can reopen across tool calls; call `end_turn` to release that history.
- `session_state` exposes latest models, modes and config options. Pushed config
  updates win over stale write responses. Load has a 90 s deadline and 2 s
  replay-idle fallback, isolated by session. Replay activity never silently
  changes another session's state. Unsolicited updates never allocate cached session
  entries; successful close/delete removes entries, and termination clears the cache.
- `cancel` flushes the notification before returning. `cancel_and_wait` waits
  up to 15 s for the active prompt to settle; timeout leaves it registered.
  Dropping a request future removes correlation state without sending Effect's
  private interrupt RPC. `shutdown` and last-client drop stop owned tasks and
  close pipes; the caller owns agent process cleanup. V1 prompts may overlap; v2
  allows one active prompt per session. Cancellation waits for all active v1 prompts.
- `SpawnDescription` holds command, args, environment and cwd; it executes nothing.

## Tests and remaining boundary

The Rust tests port protocol behavior from `effect-acp`'s protocol/client/schema
tests and `provider-acp`'s runtimeModel tests, including streamed full turns,
permissions, extension request/notification passthrough, v2 early idle, cancellation,
load/resume, batches, ID collisions, handler panic isolation, UTF-8 fragmentation,
EOF, writer failure, config races, replay-idle, raw backlog limits and batching.
Eight unmodified transcript fixtures are copied from
`apps/server/src/orchestration-v2/testkit/fixtures/*/registry_transcript.ndjson`:
simple, multi_turn, todo_list, tool_call_read_only, turn_interrupt,
malformed_session_update, acp_elicitation and acp_titled_multiselect. The
Antigravity initialize fixture comes from `packages/effect-acp/test/fixtures/`.
Their original provenance headers remain intact. Replays script client effects;
they do not execute recorded commands or touch the recorded paths.

Provider launch/discovery/auth policy, process trees and stderr diagnostics,
filesystem containment, terminal execution/retention, MCP server bridging,
orchestration projection, subagent lineage, busy-prompt retries and provider
completion/steering quirks belong to later services/adapters. They are not
implemented or claimed as provider parity here. This crate preserves those
updates and extension messages for the adapter. It has no `t3-process` dependency.

From `rust/`, use `cargo fmt -p t3-acp --check`,
`cargo clippy -p t3-acp --all-targets -- -D warnings`, and `cargo test -p t3-acp`.
Set `CARGO_TARGET_DIR` to `rust/target/agents/t3-acp` (absolute path) for these
commands to avoid contention with other rewrite tasks.

## Transport limits and compatibility notes

`ClientOptions::max_line_bytes` defaults to 16 MiB (including a newline when
present). Oversized lines terminate with `Error::LineTooLong` before retaining
more than that limit. `max_concurrent_handlers` defaults to 64; excess requests get
`-32000` without invoking the handler. Limits are deliberate resource bounds beyond
the TypeScript implementation. Responses retain input batch order.

Missing `jsonrpc`, null notification IDs, fractional/large numeric IDs, empty
batches and scalar frames do not terminate the session. Missing raw results map
to JSON null (JavaScript uses undefined); typed session methods still validate
their required result fields. Invalid UTF-8 is decoded lossily. A valid final
unterminated frame is routed even if a handler finished during the partial read.
Before initialize negotiates a response shape, compatibility defaults to v2.

The review's elicitation-generation finding does not apply to the pinned schemas:
`schema-v1.gen.ts:6051` and `schema.gen.ts:6152` define the same form, URL and future
mode union. SDK v1 and v2 accept both scopes and all those shapes; a regression
test exercises each generation without changing this dispatch rule.

The TypeScript transport serves extension handlers inline, while core RPC handlers
run concurrently and ignore duplicate active IDs. This crate matches those paths;
separate extension calls can reuse an ID after the first finishes.

Replay tests compare normalized content with explicit SDK serialization defaults,
validate typed outgoing payloads and handler inputs, and verify fixture copies
against their TypeScript source. Security tests cover permission, filesystem and
all terminal methods across malformed params, panic, cancellation, drop, shutdown
and malformed transport input. No test executes a recorded command or path.
