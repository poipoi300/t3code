# t3-scheduler

Native, transport-neutral scheduled tasks over `t3-persistence::Database`, using
`t3-contracts::scheduled_task` wire types. No processes, server startup, live T3
home access, new tables, or migrations. Does not target wasm32.

## API and integration

`ScheduledTaskService::new(database, clock, orchestration, options).await` attempts
startup recovery, logging failures without preventing startup. `list`, `get`, `subscribe_list`, `upsert`,
`set_enabled`, `delete`, `delete_for_thread`, `move_thread_tasks`, `run_now`,
`rotate_webhook_token`, `trigger_webhook`, `list_webhook_deliveries`, and
`get_webhook_delivery` are service operations. Mutations return the shared task
type; `run_now` also returns its dispatched thread and dispatch-failure flag.
`subscribe_list` reads after subscribing, emits an initial list, and coalesces
dirty notifications. `subscribe_changes` exposes the lower-level watch receiver.

Implement `Orchestration::dispatch(DispatchRequest)` with the real orchestrator.
Requests carry the complete task, trigger, rendered prompt, stable command/message
IDs and fresh-context flag. An unbound task launches a thread with its stored
selection/workspace/modes; a bound task queues in its existing thread using that
thread's current provider. The adapter must deduplicate IDs and follow destination
project moves (TypeScript retries a moved destination twice). Optional trait
operations supply queued-occurrence checks, destination/policy validation,
single-use secret references, thread existence, and eventual provider failures.
They are real integration seams, not an in-crate simulation of orchestration.
`validate` must enforce archive/project/existence checks only when the destination
changes, enforce enabled unbound runtime policy, and apply scratch-project
workspace policy in the adapter. Webhook policy is also enforced directly through
`SchedulerOptions::webhook_policy_block`.

Create `Scheduler::new(clock)`, retain both registrations returned by registering
the service's `sources()`, then `activate()` **after all server startup recovery**.
Sources run initially and every five seconds, independently and without overlap.
Dropping a registration cancels that source and its in-flight work. Dropping the
scheduler cancels its sources. Drop/shutdown the service to cancel its webhook
workers and outcome followers; `shutdown().await` awaits their cleanup. Explicit
`run_due_tasks` / `dispatch_pending_webhooks` support deterministic driving.
`drain_background` awaits already admitted work (including work parked on an
orchestration adapter); it does not advance time or dispatch future windows.

Use `SystemClock` in production. Tests inject `TokioClock::new(utc_origin)` and
Tokio's paused clock. The calendar zone is explicit in `SchedulerOptions`, defaulting
to `TZ` or the host IANA zone. Relay origins are supplied; `relay_hook_base_url`
extracts the managed tunnel's endpoint key rather than using the environment ID.

`RelayDeliveryProof` accepts a supplied Ed25519 SPKI mint public key, issuer, and
environment ID. It verifies the relay JWT and its binding to hook, delivery ID,
received time, issuer, audience, expiry, and 25-hour age, with 60-second tolerance.
Only it can construct `VerifiedRelayDelivery` outside this crate. Without a valid
proof, pass `relay: None`; headers alone never establish relay identity. The HTTP
adapter must cap its streaming reader at `WEBHOOK_MAX_BODY_BYTES` (1 MiB), refuse
HEAD, retain exact signature bytes, and map `TriggerResult::http_status()` and
`outcome()` to the response and `x-t3-hook-outcome`. The service repeats size/method
checks; it derives body text from the supplied bytes.

## Ported behavior and source

- `apps/server/src/scheduledTasks/Schedule.ts`: elapsed intervals, fixed local
  times and weekday sets, one-shot offset times, structural equality, minimum
  interval clamp, and the strictly-greater-than-ten-minute missed-slot grace.
  There is **no cron expression parser or cron schedule variant in TypeScript**.
  The source uses Effect `DateTime`, not a cron library. This crate ports its
  calendar logic using `chrono`, `tz-rs` and `jiff-tzdb`, including compatible DST handling:
  earlier occurrence in a fold, forward shift across a gap. It does not use a
  cron crate with different DST semantics.
- `scheduledTasks/ScheduledTaskService.ts`: task CRUD, stable ordering/identity,
  unchanged-schedule preservation, run-state ownership, stale-snapshot guards,
  recurring queued-occurrence suppression, thread binding/cleanup, completion
  and subsequent provider failure. Existing `scheduled_tasks` columns store the
  latest run and count; per-run orchestration history belongs to the adapter.
  Overdue intervals/one-shots fire once; stale fixed-time slots fail and advance.
  Restart-left running rows fail and advance, with early manual one-shots retained.
  Undecodable running rows fail without recomputing their schedule.
- Webhook code in that service plus `webhookDedupe.ts`, `webhookVerification.ts`,
  `webhookFilter.ts`, `webhookTemplate.ts`, `RelayDeliveryProof.ts`: constant-time
  URL tokens, raw-body HMAC-SHA256 hex/base64 verification, token rotation,
  write-only secrets, age checks, scalar/wildcard filters, framed/inert template
  data, credential redaction, 120,000 UTF-16-unit prompt ceiling, UTF-8-safe body
  logs and bounded prompt logs. JavaScript numeric formatting, array-index object
  key ordering and header insertion order are preserved for template/dedupe use.
- Existing webhook delivery, pending, replay-key and relay-delivery tables:
  atomic admission, seven-day replay expiry, latest-payload coalescing, immutable
  expired windows, current prompt before dispatch / saved prompt after dispatch,
  restart recovery with stable IDs, release of all failed window members,
  completion-write retry, and retained claims after successful dispatch.
  Immediate deliveries are memory queues, as in TypeScript. Restart does not
  invent durable retries for immediate deliveries that never began dispatch.
  Work is FIFO per task, with one worker per task, independent tasks, a 20-item
  capacity, 60/minute admission and rejection windows, 10/minute suppression logs,
  and separate 50-run / 20-suppression / 20-filtered retention buckets. Duplicate,
  coalesced, rate-limited and oversized-prompt deliveries share suppression retention.
- `apps/server/src/scheduling/{Scheduler,DueHint}.ts`: activation barrier,
  isolated non-overlapping sources, scoped cancellation, versioned idle hints
  and their ten-minute backstop. Errors are typed and worker/source failures
  are logged through `tracing`.

Tests port behavioral cases from `Schedule.test.ts`, all three non-webhook
`ScheduledTaskService*.test.ts` files, `ScheduledTaskService.webhook.test.ts`,
`webhook{Filter,Template,Verification}.test.ts`, and `scheduling/Scheduler.test.ts`.
They use real temporary migrated SQLite databases, a recording orchestration
adapter, deterministic events/gates, failure-injection SQLite triggers and paused
time. No test uses a real sleep. Higher-level thread launch/provider policy and
HTTP/router tests await their owning adapter crates; they are not claimed as
end-to-end ports here.

## Integration boundaries and known differences

- HTTP/WS/MCP registration, metrics/trace parenting, managed-policy/secret-store
  loading and real orchestrator operations remain outside this crate, corresponding
  to `webhookRoute.ts` and service imports from `auth/`, `policy/`, `secrets/`,
  `project/`, and `orchestration-v2/`. The adapter must normalize worktree requests
  for scratch projects; this crate has no project store to do that itself.
- Chrono's representable years are narrower than JavaScript Date's outermost
  range. Unrepresentable interval occurrences have no next run. Ordinary dates,
  extended-year ordering and all source DST fixtures are supported. Parsing
  corrupt/legacy freeform `next_run_at` values is ISO-oriented, not JavaScript's
  implementation-dependent `Date.parse` grammar. Valid one-shot contracts require
  explicit offsets and UTC years 0000–9999, and are validated here.
- Calendar calculations use the published `jiff-tzdb` 0.1.9 data (IANA 2026e,
  released 2026-09-29), parsed by `tz-rs`; `chrono-tz` 0.10.4, the latest published
  release checked on 2026-11-13, supplies the existing zone-name API only. Its
  older rules are never used for scheduling. This keeps Windows and Unix on the
  same bundled rules without vendoring a third-party crate or reading host files.
  Effect uses the Node runtime's Intl/ICU data; differing releases can still
  disagree on later rule changes. Release maintenance must check IANA and
  `jiff-tzdb` updates, upgrade the dependency, advance the dated minimum-version
  test, and run the real-TypeScript calendar comparison against the supported
  Node runtime. Rebuild and restart to ship new rules; startup recalculates future
  calendar slots and overdue slots that no longer match the saved wall-clock
  schedule. Overdue slots valid under the current rules retain their missed-run grace.
  No build or running server fetches time-zone data. A service keeps its configured
  zone until recreated.
- JSON body placeholders and filters retain serde_json's 128-level recursion
  limit. Deeper JSON is treated as unparseable, so structured body paths are
  missing; raw `{{body}}` and `{{request}}` access remains available. TypeScript's
  `JSON.parse` accepts deeper inputs.
- Rust strings cannot retain lone UTF-16 surrogates inside JSON placeholder
  values, or a half-surrogate at the UTF-16 prompt-log truncation boundary. Invalid
  JSON with lone surrogates falls back to raw-body access, and truncation keeps a
  valid scalar. All persisted wire timestamp strings otherwise retain spelling
  until an operation intentionally computes a new timestamp.

No local types need moving into `t3-contracts`; the added types are native runtime
seams and service results. POSIX execution is unverified; no platform-specific
process behavior is used.

From `rust/`, with `CARGO_TARGET_DIR` set to
`C:\Users\Administrator\t3code-rust\rust\target\agents\t3-scheduler`:

```text
cargo fmt -p t3-scheduler
cargo clippy -p t3-scheduler --all-targets -- -D warnings
cargo test -p t3-scheduler
```
