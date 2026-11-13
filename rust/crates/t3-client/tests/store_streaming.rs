//! Streaming cost of the store: a few thousand token deltas into one thread of a store that holds
//! 2,000 threads, under the three ways a client reads it. Timings are printed, never asserted;
//! the assertions pin what each delta copies, which is what decides whether a delta's cost grows
//! with the size of the store or of the thread.
//!
//! `cargo test -p t3-client --release --test store_streaming -- --nocapture` prints the report.
#![allow(clippy::unwrap_used, clippy::print_stdout)]

use serde_json::{Value, json};
use std::rc::Rc;
use std::time::Instant;
use t3_client::{ClientStore, state::selectors, types::*};
use t3_contracts::orchestration_v2::*;

const THREADS: usize = 2_000;
const ROWS: u64 = 3_000;
const DELTAS: u64 = 3_000;
const NOW: &str = "2026-06-20T00:00:00.000Z";

fn thread_json(id: &str) -> Value {
    json!({"id":id,"projectId":"project","title":"Thread","providerInstanceId":"codex","modelSelection":{"instanceId":"codex","model":"gpt-5.4"},"runtimeMode":"full-access","interactionMode":"default","branch":null,"worktreePath":null,"activeProviderThreadId":null,"lineage":{"rootThreadId":id,"parentThreadId":null,"relationshipToParent":null},"forkedFrom":null,"createdBy":"user","creationSource":"web","createdAt":NOW,"updatedAt":NOW,"archivedAt":null,"settledOverride":null,"settledAt":null,"lastVisitedAt":null,"deletedAt":null})
}

fn item(thread: &str, index: u64, output: &str) -> Value {
    json!({"id":format!("item-{index}"),"type":"command_execution","threadId":thread,"runId":null,"nodeId":null,"providerThreadId":null,"providerTurnId":null,"nativeItemRef":null,"parentItemId":null,"ordinal":index+1,"status":"completed","title":format!("Command {index}"),"input":"cargo test --workspace","output":output,"exitCode":0,"startedAt":NOW,"completedAt":NOW,"updatedAt":NOW})
}

fn thread_shell(id: &str, title: &str) -> Value {
    let mut thread = thread_json(id);
    thread["title"] = json!(title);
    thread.as_object_mut().unwrap().extend(json!({"latestRunId":null,"activeRunId":null,"status":"idle","pendingRuntimeRequest":null,"latestVisibleMessage":null,"latestUserMessageAt":null,"hasActionableProposedPlan":false,"itemCount":0,"visibleItemCount":0}).as_object().unwrap().clone());
    thread
}

fn shell_snapshot() -> OrchestrationV2ShellStreamItem {
    let threads: Vec<Value> = (0..THREADS - 1)
        .map(|index| thread_shell(&format!("thread-{index}"), "Thread"))
        .chain([thread_shell("hot", "Thread")])
        .collect();
    serde_json::from_value(json!({"kind":"snapshot","snapshot":{"schemaVersion":1,"snapshotSequence":1,"threads":threads,"archivedThreads":[],"projects":[{"id":"project","title":"Project","workspaceRoot":"/workspace/project","repositoryIdentity":null,"defaultModelSelection":null,"scripts":[],"createdAt":NOW,"updatedAt":NOW}]}}))
    .unwrap()
}

fn shell_delta(sequence: u64) -> OrchestrationV2ShellStreamItem {
    serde_json::from_value(json!({"kind":"thread.updated","sequence":sequence,"location":"active","thread":thread_shell("hot", &format!("Thread {sequence}"))})).unwrap()
}

fn snapshot(thread: &str, rows: u64) -> OrchestrationV2ThreadStreamItem {
    let items: Vec<Value> = (0..rows)
        .map(|index| item(thread, index, &"line of command output\n".repeat(8)))
        .collect();
    let visible: Vec<Value> = items
        .iter()
        .enumerate()
        .map(|(position, item)| json!({"position":position,"visibility":"local","sourceThreadId":thread,"sourceItemId":item["id"],"item":item}))
        .collect();
    serde_json::from_value(json!({
        "kind":"snapshot","snapshotSequence":1,
        "projection":{"thread":thread_json(thread),"runs":[],"attempts":[],"nodes":[],"subagents":[],"providerSessions":[],"providerThreads":[],"providerTurns":[],"runtimeRequests":[],"messages":[],"plans":[],"turnItems":items,"checkpointScopes":[],"checkpoints":[],"contextHandoffs":[],"contextTransfers":[],"visibleTurnItems":visible,"updatedAt":NOW}
    }))
    .unwrap()
}

fn delta(thread: &str, sequence: u64) -> OrchestrationV2ThreadStreamItem {
    let output = "token ".repeat(sequence as usize % 400);
    serde_json::from_value(json!({
        "kind":"event","sequence":sequence,
        "event":{"type":"turn-item.updated","id":format!("event-{sequence}"),"threadId":thread,"occurredAt":NOW,"payload":item(thread, ROWS - 1, &output)}
    }))
    .unwrap()
}

struct Fixture {
    store: ClientStore,
    session: Generation,
    hot: ThreadKey,
}

fn fixture() -> Fixture {
    let store = ClientStore::default();
    let environment = EnvironmentId("env".into());
    let session = store.begin_session(environment.clone());
    for index in 0..THREADS - 1 {
        let id = format!("thread-{index}");
        let key = ThreadKey {
            environment: environment.clone(),
            thread: ThreadId(id.clone()),
        };
        store
            .apply_thread(key, session, vec![snapshot(&id, 4)])
            .unwrap();
    }
    assert_eq!(
        store.apply_shell(&environment, session, vec![shell_snapshot()]),
        SyncAction::Applied
    );
    let hot = ThreadKey {
        environment,
        thread: ThreadId("hot".into()),
    };
    store
        .apply_thread(hot.clone(), session, vec![snapshot("hot", ROWS)])
        .unwrap();
    Fixture {
        store,
        session,
        hot,
    }
}

#[derive(Debug, Default)]
struct Copies {
    projections: u64,
    shells: u64,
    environments: u64,
}

/// Addresses that change only when the store copies: the row buffers, not their `Rc`s. A held
/// weak view lets `Rc::make_mut` move a projection's fields to a new allocation without copying
/// a row, and that move is not a copy worth counting.
fn identities(store: &ClientStore, key: &ThreadKey) -> [usize; 3] {
    let snapshot = store.snapshot();
    let environment = &snapshot.environments[&key.environment];
    let thread = &environment.threads[&key.thread];
    [
        Rc::as_ptr(environment) as usize,
        environment
            .shell
            .snapshot
            .as_ref()
            .map_or(0, |shell| shell.threads.as_ptr() as usize),
        thread
            .projection
            .as_ref()
            .map_or(0, |p| p.turn_items.as_ptr() as usize),
    ]
}

enum Delta {
    Thread(OrchestrationV2ThreadStreamItem),
    Shell(OrchestrationV2ShellStreamItem),
}

/// Streams the deltas; `read` runs after each one and may keep what it read until the next.
fn stream(shell: bool, mut read: impl FnMut(&ClientStore, &ThreadKey)) -> (Copies, f64) {
    let Fixture {
        store,
        session,
        hot,
    } = fixture();
    let deltas: Vec<_> = (2..DELTAS + 2)
        .map(|sequence| {
            if shell {
                Delta::Shell(shell_delta(sequence))
            } else {
                Delta::Thread(delta("hot", sequence))
            }
        })
        .collect();
    let mut copies = Copies::default();
    let mut elapsed = 0.0;
    for delta in deltas {
        let before = identities(&store, &hot);
        let started = Instant::now();
        let action = match delta {
            Delta::Thread(delta) => store
                .apply_thread(hot.clone(), session, vec![delta])
                .unwrap(),
            Delta::Shell(delta) => store.apply_shell(&hot.environment, session, vec![delta]),
        };
        read(&store, &hot);
        elapsed += started.elapsed().as_secs_f64();
        assert_eq!(action, SyncAction::Applied);
        let after = identities(&store, &hot);
        copies.environments += u64::from(before[0] != after[0]);
        copies.shells += u64::from(before[1] != after[1]);
        copies.projections += u64::from(before[2] != after[2]);
    }
    let snapshot = store.snapshot();
    let thread = selectors::thread(&snapshot, &hot)
        .and_then(|thread| thread.projection.clone())
        .unwrap();
    assert_eq!(
        thread.visible_turn_items.last().map(|row| row.position),
        Some(ROWS - 1)
    );
    assert_eq!(
        selectors::shell(&snapshot, &hot.environment)
            .and_then(|shell| shell.snapshot.as_ref())
            .map(|shell| shell.threads.len()),
        Some(THREADS)
    );
    (copies, elapsed * 1e6 / DELTAS as f64)
}

fn report(name: &str, copies: &Copies, micros: f64) {
    println!(
        "{name:<40} {micros:>9.1} µs/delta  copies: projection {:>5}  shell {:>5}  environment {:>5}",
        copies.projections, copies.shells, copies.environments
    );
}

#[test]
fn streaming_copies_neither_the_store_nor_the_thread() {
    // Nobody reads: the store mutates the hot thread in place.
    let (copies, micros) = stream(false, |_, _| {});
    report("thread deltas, no reader", &copies, micros);
    assert_eq!((copies.projections, copies.environments), (0, 0));

    // The UI pattern (t3-ui `StoreSignals::thread`): a memo keeps the latest shared view.
    let mut held = None;
    let (copies, micros) = stream(false, |store, key| held = store.shared_thread(key));
    report("thread deltas, UI holds shared_thread", &copies, micros);
    assert!(held.is_some());
    assert_eq!(copies.projections, 0, "a held view must not force a copy");
    assert_eq!(copies.environments, 0);

    // Shell deltas (thread status, titles) while the UI holds the sidebar's shared shell.
    let mut held = None;
    let (copies, micros) = stream(true, |store, key| {
        held = store.shared_shell(&key.environment)
    });
    report("shell deltas, UI holds shared_shell", &copies, micros);
    assert!(held.is_some());
    assert_eq!(copies.shells, 0, "a held view must not force a copy");
    assert_eq!(copies.environments, 0);

    // What the views cost before they linked to the store weakly: a held strong `Rc` to the
    // projection or the shell snapshot, so every delta deep-copied the rows.
    let mut pinned = None;
    let (copies, micros) = stream(false, |store, key| {
        pinned = selectors::thread(&store.snapshot(), key).and_then(|t| t.projection.clone());
    });
    report("thread deltas, strong view (before fix)", &copies, micros);
    assert!(pinned.is_some());
    assert_eq!(copies.projections, DELTAS - 1);
    let mut pinned = None;
    let (copies, micros) = stream(true, |store, key| {
        pinned = selectors::shell(&store.snapshot(), &key.environment)
            .and_then(|shell| shell.snapshot.clone());
    });
    report("shell deltas, strong view (before fix)", &copies, micros);
    assert!(pinned.is_some());
    assert_eq!(copies.shells, DELTAS - 1);

    // A reader that keeps a whole store snapshot between deltas pays for everything it pins:
    // the environment's thread map (one entry per thread) and the hot projection's rows.
    let mut pinned = None;
    let (copies, micros) = stream(false, |store, _| pinned = Some(store.snapshot()));
    report("thread deltas, reader pins snapshot()", &copies, micros);
    assert!(pinned.is_some());
    assert_eq!(copies.environments, DELTAS - 1);
    assert_eq!(copies.projections, DELTAS - 1);
}
