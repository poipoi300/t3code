# t3-claude-cli

Native protocol client for Claude Code's bidirectional NDJSON stdio channel.
It does not launch or supervise processes, read Claude's store, resolve settings,
or project T3 orchestration events. The compatibility baseline is defined once
in `PROTOCOL_BASELINE` in `src/lib.rs`; an explicit installed executable may differ.
`Client::cli_version()` reports the version observed in initialize or system/init.

`Session::connect(reader, writer, options)` accepts any owned Tokio `AsyncRead`
and `AsyncWrite`. Keep the session alive and continuously drain its `Stream` or
`next_event()`. Clone `Client` for concurrent `request`, `initialize`,
`send_user`, `interrupt`, and `send_mcp_message` calls. Call initialize before
sending user input. Control replies expose the complete response and a generic
typed `decode`; initialize returns a typed lossless response directly.

`Event::Request` contains a typed `PendingRequest` and a consuming `Responder`.
Permission answers automatically carry the CLI's actual `toolUseID`. Responders
expose a cancellation token; cancelled requests reject late answers. Dropping an
unanswered responder sends an error. Unsupported controls and malformed callback
bodies receive errors automatically and surface as raw unknown messages with a
non-fatal diagnostic. The four remote tool controls are exposed but left
unanswered, matching the SDK. A reply slot is reserved before publishing each
callback, so dropping an unanswered responder can enqueue an error even under
backpressure. Concurrent prompts retain independent IDs and actual tool-use IDs.

Register an `McpServer` to dispatch nested JSON-RPC automatically, or use
`register_pending_mcp_server` to answer it through the event stream. Register
servers before advertising their names in initialize / mcp_set_servers. Servers
receive cancellation and retain responsibility for JSON-RPC method semantics.
`McpServer::handle` awaits requests; `notify` synchronously delivers notifications
and responses without requiring a JSON-RPC response from the server.
The bridge validates response IDs, scopes correlation by server, wraps replies
in `mcp_response`, and emits the SDK's id-zero acknowledgement for one-way
deliveries (notifications, null IDs and JSON-RPC responses).
Unknown servers receive an unsupported error. Host-originated MCP messages use
`send_mcp_message`; the caller decides when initialize has advertised the server.
Manifest pre-capture is an unused SDK optimization and is not implemented.

Dropping a request future attempts control cancellation. `end_input` half-closes
stdin and allows remaining output to drain. `Session::shutdown` allows five
seconds to flush writes, then
cancels blocked I/O, callbacks and controls and joins the reader/writer tasks.
`shutdown_with_cancellation` uses a caller token to bound the flush instead. Dropping
the session cancels both I/O tasks. These operations never kill a process; the
owner must drain stderr and supervise the child. There is no implicit request
deadline; wrapping a request in a caller-owned deadline cancels it on expiry.
Event and write queues each hold at most 32 entries; full queues backpressure
the reader or producer. Callback reply reservations also bound automatically
dispatched MCP handlers. Drain events concurrently with client operations.
Outbound-future cancellation is best effort if the write queue is full. A stdin
failure rejects new writes while previously written controls and stdout continue
to drain until their replies or EOF.
Incoming line size is bounded separately and permits base64 media by default.

`Message`, `SystemMessage`, `StreamEvent`, `Delta`, and `ContentBlock` expose known
variants and explicit unknown variants. A message owns one shared raw JSON
snapshot (`Message::Unknown` holds `Arc<Value>`); nested known bodies use
`Typed<T>::get()` without additional raw snapshots. `Decoded<T>` provides
immutable typed access through `get()` and exact JSON through `raw()`, preserving
unknown fields and absent versus null values at the message boundary. Malformed
known shapes become unknown values without closing the session.
`Event::Diagnostic` carries a redacted error, shared raw value and, for repaired
or invalid JSON, the original bytes. Unknown fields, types and suggestion kinds
produce non-fatal diagnostics. UTF-8 decodes lossily; lone UTF-16 surrogate
escapes become U+FFFD in Rust strings, with the original wire bytes retained in
the diagnostic. Large numeric literals remain raw numbers, and deserialization
grows its stack for deeply nested JSON instead of imposing serde's default
nesting limit. Object-looking invalid JSON produces a protocol diagnostic;
other non-JSON stdout reports only its length and is skipped by default.
Permission suggestions remain raw JSON values and never grant permission.
Keep-alive and transcript-mirror frames are exposed as
housekeeping messages without implying turn completion. CLI error *results* stay
stream messages; control error verdicts produce `Error::Cli`.

`describe_spawn` is pure. Supply the environment, cwd, explicit executable or
resolved optional package candidates, JavaScript runtime, and options. The result
contains command/argv/environment/cwd and the separate initialize payload.
`resolve_executable` consumes a supplied Windows PATH result and existence predicate;
`t3_environment` consumes inherited and explicit environments and an already
resolved home. Neither reads files, expands paths, or accesses authentication.
Trace propagation, process scopes, attachment paths/budgets, prompt composition,
and dynamic HTTP MCP credential registration remain caller responsibilities.
`image_block` and `pdf_block` encode supplied bytes only; callers validate media,
document pages and retained-conversation budgets before embedding them.

The decoder replays all 39 existing TypeScript recordings (2,261 inbound entries)
and checks typed members, the frozen typed sequence and exact JSON preservation.
Separately marked constructed SDK samples cover all 15 modeled top-level types
and 32 modeled system subtypes. The recordings contain post-SDK messages,
not raw CLI control captures. Scripted Tokio pipes cover
every T3 outbound control and all five inbound families, concurrent correlation,
cancellation, EOF, framing, and input across results. Protocol-level behaviors
ported from TypeScript tests include stdin-only dynamic MCP credentials,
priority-now steering, media shapes, permission payload casing, Windows native/JS
resolution, home isolation, and resume MCP environment behavior. Projection,
history, settings-policy and attachment-budget tests belong to their owning crates.

Run from `rust/`:

```text
cargo fmt -p t3-claude-cli --check
cargo clippy -p t3-claude-cli --all-targets -- -D warnings
cargo test -p t3-claude-cli
```

The ignored `cli_handshake` test is an explicit opt-in. It discovers a CLI on
PATH, starts it in a disposable crate-local directory with a cleared environment
and isolated home/config locations, initializes, and waits for system/init. It
never sends a prompt. The test may fail if that CLI only emits system/init after
prompt consumption; the initialize control response is independently checked.
It never resumes a session or inherits provider credentials.

Source comparisons with inventory `03c-claude.md`:

- Section 5.1's settings flag shorthand uses `--settings=<JSON/path>`; the SDK
  emits separate `--settings`, value arguments. MCP config and extra-argument
  values use the same split formatter, except values beginning with a dash.
- The environment description groups defaults under a case-insensitive presence
  check. Only the session-state flag has that check on this path; entrypoint
  and SDK-version defaults use exact-case, falsey property checks. A falsey
  exact-case value is replaced, whereas a differently cased key is preserved.
- The SDK emits `conversation_reset` as a top-level type, rather than a system
  subtype; the inventory's miscellaneous list does not specify its placement.
  Both forms are decoded for tolerance.
- An omitted system-prompt option becomes `systemPrompt:[""]` in initialize;
  the Claude Code preset omits that field. This clarifies the inventory's
  qualification that probe defaults may add metadata.

Initialize redelivery dispatches only matching permission and dialog subtypes,
as the pinned SDK does; elicitation metadata remains raw. Set
`replay_pending_callbacks` false to retain metadata without replay.
Non-initialize responses do not replay callbacks. Error Display/Debug and Debug
for payload-bearing public types redact all caller and peer values; inspect raw
fields explicitly when content is needed. JSON errors expose only category and
line/column, never serde messages that can quote input. Protocol-specific types
remain local to this crate (candidates for t3-contracts if other crates need them).

The private transcript-fork graph rewrite and complete settings resolver remain
unimplemented, as identified as untraced in the inventory. They are disk/policy
algorithms, not controls; no substitute fork or simplified settings merge is
silently performed by this crate.
