# t3-codex-app-server

Native Tokio protocol client. It accepts any `AsyncRead`/`AsyncWrite` pair and
does not spawn or own processes. There is no dependency on `t3-process` or on
T3 orchestration contracts.

`Client::connect` returns a cloneable `Client` and one `Events` stream (also
usable through `recv`). Drain events concurrently with RPC calls. Notifications
include their raw envelope and decoded payload. Known malformed notifications
produce one payload-free diagnostic per method/error category; unknown methods
and item variants retain their JSON. `ClientOptions::event_capacity` bounds the
queue (64 by default); a full queue pauses the reader.

An RPC response waits until earlier events have been handled. Polling the next
event acknowledges the previous one. Call `Events::acknowledge()` explicitly
after handling an event if you need to await an RPC before polling again. This
also applies when an approval handle is handed to a separate task: acknowledge
the handoff, without waiting for the approval decision. Dropping `Events` stops
the connection and invalidates all pending handles.

`Client::initialize` serializes the initialize request across clones. After the
provider accepts the returned server version, call `Client::initialized` to
send the notification; both steps are idempotent across clones.
`InitializeParams::t3` supplies the adapter's capabilities;
`InitializeParams::probe` supplies discovery's smaller capability set. The
initialize result is `Some(server metadata)` normally, or `None` for the exact
`-32600 / Already initialized` interrupted-handshake recovery case. Runtime
version policy belongs to the provider layer.

Typed methods cover the inventory's entire outbound ledger: thread lifecycle,
history, compaction, goals, turn start/steer/interrupt, account/limits/reset
credits, models, skills, config, feedback, MCP and raw sample/background-terminal
extensions. `request` and `request_raw` remain available for extensions. A raw
`Params::Missing` differs from `Params::Present(Value::Null)`. Optional nullable
rate-limit update fields use `Field` to preserve missing/null/value separately.

`Event::Request` owns a boxed `ServerRequest` handle. Its read-only `params()`
can be inspected before `into_typed()` selects a `TypedServerRequest` variant.
Each `TypedRequest<P, R>::respond` accepts only that method's response DTO;
unknown extension requests use JSON responses. `reject` echoes the original
string or numeric ID. Dropping an unanswered handle sends method-not-found.
At most 32 requests are outstanding by default. A reused ID invalidates the old
handle and admits its replacement without ending the shared session.
`serverRequest/resolved` cancels its exact handle before typed notification
decoding, releases the slot, and makes later replies fail with `RequestResolved`.
`cancelled()` also wakes on termination. Numeric and string server request IDs
remain distinct; outbound correlation follows JavaScript's string-normalized
number IDs. Fractional and large numeric IDs/codes are supported.

A malformed routable reply fails only its matching request. Other bad frames
produce `FrameDecodeFailed` and are skipped; oversized frames are discarded up
to their newline while retaining the size bound. UTF-8 is decoded lossily.
Unencodable responses send an internal error for that request and return a
local error. Payload errors contain field paths and categories, without values.

Dropping an RPC future cancels its local waiter, without inventing a native
cancel request. A turn is interrupted explicitly with `turn_interrupt`.
`shutdown` flushes queued writes and closes the writer; there is no native
shutdown RPC. Dropping every client also stops both I/O tasks. Buffered events
remain readable before the terminal error. The process host must own stderr
draining, exit-status enrichment, containment and shutdown/kill deadlines.

`launch::app_server_launch` is pure. Inputs include ordered environment patches,
the host environment/home/platform and an optional result of PATH/PATHEXT
resolution. It returns argv, environment, optional cwd/inheritance, Windows batch
shell handling, shared Codex containment metadata and the two-second forced-exit
deadline. The shared live process leaves cwd absent; probes may specify it.
Executable discovery, caching and actual process execution remain host work.
Managed-runtime config/auth args can be supplied after the managed provider
resolves them; credential management is outside this crate.

## Protocol source and dependency decision

The single protocol pin and strategy are in `[package.metadata.codex-protocol]`
in `Cargo.toml`, matching T3's TypeScript generator. The official source is
OpenAI's `codex-rs/app-server-protocol`; its Apache-2.0 license is compatible with
reuse. The similarly named crates.io fork was not used.

The exact Git dependency was added and tested with stable Rust 1.99.1 on Windows.
`cargo check -p t3-codex-app-server` failed during resolution after 32.03 seconds
(including cache-lock/index/network waits), before compiling any upstream crate:

```text
codex-app-server-protocol -> codex-rollout -> codex-otel
codex-otel requires tokio-tungstenite ^0.28.0 with feature proxy
crates.io tokio-tungstenite 0.28.0 has no proxy feature
```

Codex relies on patches in its own workspace root, which a consuming workspace
does not inherit. Fixing that requires broader workspace changes/vendor policy.
An exact resolved dependency count and successful added compile time therefore
cannot be reported for reuse. A conservative traversal of upstream's checked-in
lockfile reaches 940 packages, including dev dependencies and 35 internal Codex
packages; that is **not** an actual normal-build dependency count.

This crate instead ports the used DTO subset from T3's source and the pinned
schema. It uses the shared serde/Tokio stack, `ryu-js` for JavaScript numeric ID
formatting, and `serde_path_to_error` for value-free payload paths.

Preserved compatibility differences include open plan/hook/effort/status/phase
strings, unknown native error values, `partial_answer`, image generation without
inline result, minimal thread metadata, and turn/start's `collaborationMode`,
`additionalContext` and `cyberAccessProgram`. Config/schema/MCP/permission/raw
Responses payloads remain JSON rather than a second closed schema universe.

When changing the pin, compare these DTOs with the new upstream schema and T3
generator overrides, then rerun the recorded corpus and opt-in probe. Protocol
v1/v2 namespaces coexist on one connection; initialize negotiates capabilities,
not a protocol version. Unknown methods do not terminate the transport, and
unsupported optional RPCs retain their native error codes for the provider to
handle. There is no automatic reconnect or prompt replay.

## Verification

Run from `rust/`:

```powershell
cargo fmt -p t3-codex-app-server --check
cargo clippy -p t3-codex-app-server --all-targets --all-features -- -D warnings
cargo clippy -p t3-codex-app-server --all-targets --all-features --features serde_json/preserve_order -- -D warnings
cargo test -p t3-codex-app-server
cargo test -p t3-codex-app-server --all-features --features serde_json/preserve_order
cargo test -p t3-codex-app-server --test live -- --ignored --nocapture
```

Consumers unify `serde_json/preserve_order`, which enlarges inline JSON values;
the second Clippy/test configuration checks that layout as well as the standalone one.

The ignored probe accepts `CODEX_BINARY`, otherwise uses the installed OpenAI
Windows binary or `codex` on PATH. It creates a temporary `CODEX_HOME`, initializes
and lists models only, drains stderr, and reaps its captured child. No turn is
started. On this machine it passed against 0.167.2 and returned nine models;
the handshake and model-list responses matched T3's expectations. Other runtime
RPCs were not probed against that installed version.

The replay tests read the TypeScript NDJSON corpus directly, without copying or
modifying it. All 28 transcripts (3,553 inbound and 216 outbound frames) compare
typed round trips against the recorded payloads, serialize outbound typed
requests and responses against recorded frames, and inspect nested catch-all
maps. Every permitted native extension has an exact type/field allow-list
entry with its reason. Numeric equality follows JavaScript JSON semantics;
missing and null keys remain distinct. Canonical event projection and provider
policy remain later work in the adapter crate.

`tests/schema_parity.rs` additionally checks 108 concrete types with 534 cases
built from the TypeScript declarations, including all structural union
alternatives and every optional nullable field. To regenerate after reviewing
schema drift, run `python rust/crates/t3-codex-app-server/tests/generate_schema_cases.py`
from the repository root, then format and test this crate. The normalized schema
fingerprint prevents stale generated cases from passing silently. These are
wire-preservation tests, not a duplicate of every Effect refinement.

[SCHEMA_AUDIT.md](SCHEMA_AUDIT.md) lists every audited type, the discrepancies
corrected, raw JSON boundaries and deliberate compatibility exceptions. No
`t3-contracts` definitions were changed; any later move of these Codex-specific
DTOs belongs in a dedicated Codex protocol module there. Client/transport and
launch types stay here.
