# t3-client

Shared client logic for native and web. The shell supplies a `Platform` and polls local
futures; this crate does not start an executor or import a UI framework.

## Connect to a server

The entry point is **`t3_client::connection::connect`**:

```rust,ignore
use t3_client::{authorization::Sensitive, connection::{connect, ServerCredential}};

let (client, driver) = connect(
    platform.clone(),
    server_origin,
    ServerCredential::PairingToken(Sensitive(pairing_token)),
).await?;
// Use ServerCredential::Bearer(Sensitive(access_token)) for an existing credential.
```

`platform` is a cloneable native or browser `Platform`. `connect` validates the public
descriptor before exchanging the pairing token, stores the resulting bearer in secure
storage, obtains a WebSocket ticket, reads the destination's grants, opens RPC through
`PlatformSessionFactory`, and starts config and shell subscriptions. It returns when
RPC and config are ready. Poll the returned task to load the shell snapshot and follow
updates. Only the server's completion marker makes shell synchronization `Live`. Direct connections
do not require a cloud sign-in, relay configuration, or DPoP key.

Keep `client` in the UI's owner scope and **keep polling `driver`**. Native shells use their
local executor (for example, a tokio `LocalSet`); browsers use their event loop. The task is
deliberately not `Send`. The initial `connect` future drives startup internally; subsequent
updates, probes, foreground resubscription, and reconnects need the returned task to run.
The shell supplies I/O and executor integration; retry and synchronization policy stay here.

- `client.store()` exposes the `ClientStore`. Subscribe before reading its snapshot. The
  environment record contains `config`, `shell`, and `connection`; changes are coalesced.
- `client.session()` returns the current guarded RPC endpoint, or `None` during replacement.
- `client.reconnect()` requests a fresh session and ticket; readable cached rows survive.
- `client.ready().await` waits for a live shell while the driver runs concurrently.
- `client.close()` disconnects and ends the task. Await the task to finish shutdown.
- Dropping the last client handle cancels its scope; dropping the task releases its sockets,
  subscriptions, pending requests and timers. Dispose of both with the UI owner scope.

The runtime maps supervisor generations to store generations. Store disconnects advance the
store's generation independently; callers must not substitute one for the other.

`PlatformSessionFactory` is also available to callers assembling their own registry. Such
callers share one `RemoteAuthorization` with `EnvironmentRegistry::with_authorization` (or
`load_authorized` for restoration) and `CloudAccountService::new`. `authorize_attempt` captures
the live cloud identity. Sign-out and account switch revoke authorization synchronously;
cleanup runs in the account task and exposes retryable `SignOutFailed` state on failure.
Catalog removal revokes the bound authorization immediately and removes credentials, profiles,
DPoP tokens and routing grants atomically with the next catalog save.

The convenience entry point connects an explicit HTTP(S) origin using bearer or pairing-token
authorization. SSH provisioning and managed-relay discovery use their existing lower-level
services rather than this origin-based entry point.

## Building screens on this crate

This section is for the UI engineers. A screen reads the store through shared views, derives
what it shows with the pure selectors below, and changes server state only through the guarded
session and the operation services. `t3-ui`'s `runtime::store::StoreSignals` is the one adapter
from the store to signals; extend it rather than writing a second one.

### The composition root

`composition::ClientRuntime` is the one object the UI holds. Build it once with the app's
`Platform`, the `ClientStore`, the creation source (web or mobile) and a spawner that runs local
futures on the UI's executor (the crate owns none):

```rust,ignore
let runtime = ClientRuntime::new(platform, store.clone(), CreationSource::Web, Rc::new(|f| spawn(f)));
runtime.restore().await?;                    // reconnect every saved environment
let id = runtime.pair(pairing_input).await?; // validate, pair, save, connect
runtime.environments();                      // Vec<EnvironmentEntry { id, label, state }>
runtime.changes();                           // fires when the list or a connection state changes
runtime.open_thread(&key); runtime.close_thread(&key); // counted; follows on every session
runtime.send_message(&key, text).await?;     // durable outbox; Err(Disconnected | Rejected)
let diff = runtime.turn_diff(&key, 0, 2, false);  // QueryLease: diff.resource().snapshot()
runtime.remove(&id).await?;                  // ends follows and delivery, forgets data and credentials
```

`t3-ui`'s `runtime::backend::Backend` maps onto these one to one. `query::<M>(environment,
input, policy)` mounts any other server query: every lease of the same method and input shares
one driver, which stops with the last lease. `messages(environment)` is the environment's
`SendService` (pending messages and their states) and `client(environment)` its
`ConnectedClient`. Below the root, `ConnectedClient::open_thread(ThreadId) -> ThreadLease` is
the lease the root counts: the connection runtime follows every leased thread on every new
session, stops all follows when the session ends (disconnect, sign-out, removal), and never
follows a lease that was dropped meanwhile.

### The store

`client.store()` returns the one `ClientStore` for the app. It maps `EnvironmentId` to an
`EnvironmentState` holding the shell, the threads loaded so far (keyed by `ThreadId` within the
environment; thread ids are never global), the connection state, the server config and the
store's session generation.

- **Observe first, then read.** `store.subscribe()` is a `Stream` of `ChangeSet`s. A subscriber
  holds at most one pending change: changes coalesce, and its scopes are merged. The first item
  is `Scope::All`. Scopes: `Environment(env)` (connection, config or session; it implies that
  environment's shell and threads), `Shell(env)`, `Thread(ThreadKey)`, `All`. The revision is
  local and is not a server sequence.
- **`store.shared_shell(env)`** returns `Rc<SharedShellSnapshot>`: `projects`, `threads`,
  `archived_threads` as `Rc<[Rc<row>]>`, plus `snapshot_sequence` and `older_settled`. It
  changes only after a `Shell` (or wider) change. A row that did not change keeps its `Rc`, a
  list that did not change keeps its `Rc`, and an unchanged shell returns the same `Rc`.
  Compare by pointer.
- **`store.shared_thread(key)`** returns `Rc<SharedThreadProjection>`, the same idea for one
  thread: `visible_turn_items` (the timeline rows), `turn_items`, `runs`, `messages`, `plans`,
  `runtime_requests`, `checkpoints` and the rest, plus `thread`. A streamed token replaces
  exactly one `turn_items` row and one `visible_turn_items` row. `source()` rebuilds the whole
  wire projection, which copies every row when the view is no longer current. Use the row fields.
- **Status**: `state::selectors::{shell, thread, shell_summary}` read a `store.snapshot()`.
  Drop the snapshot when the read is done. `ShellState`/`ThreadState` carry `status`
  (`Empty`, `Cached`, `Synchronizing`, `Live`, `Deleted`) and `error`; `ThreadState.history`
  says whether earlier history exists and whether it is loading.
  `EnvironmentState.connection` is the `ConnectionState` (`phase`, `desired`, `last_failure`,
  `retry_at`) and `EnvironmentState.config` is the server config.
- Wire timestamps are parsed with `t3_client::time::epoch_ms` (JavaScript `Date.parse`
  semantics); the strings themselves are never rewritten.

Measured cost (`tests/store_streaming.rs`, release, 2,000 threads, one thread of 3,000 rows):
a streamed token costs about 12 µs while the UI holds the thread's shared view, and a shell row
update about 24 µs while it holds the shell view. A reader that keeps a `StoreSnapshot` between
changes costs about 4 ms per token, because the next change must copy everything that reader
pins.

### Selectors per area

Selectors are pure functions of shared rows. Recompute one when an input `Rc` changes; most
of them reuse unchanged output `Rc`s when given their previous result.

- **Sidebar.** `view_logic::shell::sidebar::presentation::sidebar_rows(environment,
  &shared_shell, groups, options, previous)` is the selector: `present_sidebar` (the port of
  `Sidebar.logic.ts`: working shelf, inbox order, folders) over `present_thread_shell`, returning
  `Rc<SidebarRow>`s that hold the store's shared thread row and keep their `Rc` while unchanged.
  `groups` come from `workspace::sidebar::build_sidebar_snapshots` (WP8 is the only grouping
  policy); pass none for a flat list. Re-run it at `next_snooze_wake`, the only timer it needs.
- **Thread route.** `view_logic::shell::navigation::route_render_state(&snapshot, &key,
  draft_exists)` answers loading, ready or missing as `ThreadRouteView` does.
- **Notifications.** `thread_views::threads::notifications::{NotificationGate,
  next_background_announcement}`: when a thread notifies, and what a screen reader announces.
- **Timeline.** `view_logic::session::derive_timeline_entries_from_visible_turn_items_with_state`
  takes the thread view's rows and the previous result, and returns entries whose unchanged
  members keep their `Rc`. `thread_views::threads::detail::ThreadDetailViews::update` derives the
  queue, checkpoint summaries, usage-limit notice and the retained "quiet" structure view.
- **Thread lists.** `thread_views::shelves` (inbox, working, snoozed and their return grace),
  `thread_views::threads::{sort, search, families}`, `thread_views::relationships`,
  `thread_views::requests`
  (pending approvals and questions).
- **Composer.** `thread_views::composer::send::SendService` (see Operations) and
  `view_logic::shell::models::composer_model_state` for the effective model and options.
- **Projects and source control.** `workspace::project_ops::{ProjectEntities, SourceControl,
  LiveVcs}` and `workspace::projects`.
- **Pull requests.** `workspace::{pr_list, pr_views}`. Several views need forge adapters that
  do not exist yet (see the list below).
- **App shell.** `view_logic::shell::{palette, keybindings, shortcuts, panels, navigation,
  ui_state, terminal_state, request_latency}` are plain state plus transitions. Each store has a
  `STORAGE_KEY` and `encode`/`decode`; `ui_state::UiPreferences::{load, save}` read and write the
  UI preferences, shelf expansion and sidebar width through `Platform` storage.
- **Onboarding.** `view_logic::onboarding`: the first-run decision and project import.

### Operations

- Every server call goes through `client.session()`, the guarded `SessionEndpoint`. It checks
  the destination session's grants at execution; `None` means no session is installed right now.
  Use `rpc::RpcEndpoint::call` and `subscribe` with the typed method from `t3-contracts`.
- Durable commands go through `operations::OperationQueue`, one per environment. `enqueue`
  writes the command to storage before it is delivered and adds its optimistic overlay;
  `dispatch` delivers it. `view(base, projection, apply)` layers the pending intents over the
  authoritative value. Poll `follow_store(store)` beside the queue: it retires an accepted
  overlay once the authoritative rows reach its receipt, and on a new snapshot lineage it marks
  pending commands for revalidation. A delivery whose outcome is unknown is never resent
  automatically: `resolve_receipt` and `resolve_entity` settle it.
- Sending messages goes through `SendService`. Clear the editor only after a successful durable
  submit, then append `pending_messages(key)` to the timeline. The service never writes into an
  authoritative projection.
- Thread lifecycle changes such as settle and archive are `optimistic::ThreadLifecycleIntent`s
  applied over shell rows.

### What a screen must never do

- Apply server events or call `apply_shell`, `apply_thread`, `update_shell` or `update_thread`.
  Those belong to the sync owners in `state::sync`.
- Keep its own copy of server state: copied rows, a cached projection, a private list of
  threads. Hold the shared `Rc`s and derive from them.
- Hold a `StoreSnapshot`, an `Rc<ThreadState>` or a projection taken from one past the read that
  needed it (see the measured cost above). In a UI test, `store.pinned()` after a frame settles
  lists any environment or thread a screen still holds; it should be empty.
- Reach into the connection: no raw RPC clients, credentials, tokens, `ConnectionEvent`s or
  supervisor state. `ConnectedClient` (`store`, `session`, `reconnect`, `close`, `ready`) is the
  surface.
- Retry a command, build storage keys or parse ids and timestamps by hand.
- Drop a service without flushing it: call `ServerConfigService::flush` before releasing it,
  except on environment removal, which calls `close`.

### Not exposed yet

1. **Pull-request forge, Markdown and anchor adapters** (`pr_views::forge::{ForgeViews,
   ThreadLinkViews, PullRequestAnchors}`, `pr_views::markdown::PullRequestMarkdownParser`).
2. **Command-row terminal output** (the `terminalOutput.ts` fold). Expanded command rows have
   no incremental ANSI screen yet.
3. **Timeline display rows** (`MessagesTimeline.logic.ts`: work groups, folds, assistant meta).
4. **Relay, SSH and cloud-account environments in the composition root.** It pairs and restores
   direct (bearer) environments; relay discovery and account sign-in are separate services.

## Required checks

A change here can break a platform arm that only compiles for mobile. Run the target checks in
[`t3-platform`'s README](../t3-platform/README.md#required-checks-before-a-change-to-t3-platform-or-t3-client)
(wasm, one Android and one Apple mobile target) as well as the tests.

## Real-server proof

From `rust/`, with an isolated `CARGO_TARGET_DIR`:

```sh
cargo test -p t3-platform --features integration-server --test client_connection -- --nocapture
```

The test starts the real Rust server CLI in a child process, with an ephemeral loopback port
and temporary server and client directories, and stops that child by its captured PID. With the
public entry point it pairs, receives config and the shell snapshot, probes through the guarded
RPC session, reconnects, and restores the saved bearer. Then, through `ClientRuntime` alone, it
restores the saved environment, loads the shell, creates a thread over the guarded session
(`orchestration.dispatchCommand`), opens it and receives its timeline live, and waits for the
shell to list it. No raw RPC request or direct snapshot load bypasses the connection runtime.

The server does not serve the `projects.*` methods yet, so the proof thread names a project id
directly. The lease lifecycle (reconnect, close, removal, a lease dropped during a reconnect) is
tested against an in-process server in `tests/connection_threads.rs` and
`tests/client_runtime.rs`.
