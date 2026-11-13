#![cfg(all(feature = "orchestration", feature = "local-services"))]
#![allow(clippy::unwrap_used)]
use futures::StreamExt;
use serde_json::{Value, json};
use std::sync::Arc;
use t3_contracts::{auth::AuthEnvironmentScope, base_schemas::AuthSessionId, rpc::*};
use t3_orchestration::{
    api::{CommitGuard, CommitRequest, Decision, EventCommitter},
    projections::store::ProjectionStore,
    runtime::{commit::EventSink, worker::OutboxSignals},
    testing::{ManualClock, TEST_TIME, TestDatabase, detached_event, project_event},
    transport::service::TransportService,
};
use t3_rpc::{Cause, Exit};
use t3_server::{
    adapters::{
        config::{ConfigInputs, LocalServices, ServerFlags, resolve},
        orchestration::{
            OrchestrationAdapter, iso_from_unix_millis, protocol_header_accepted,
            protocol_mismatch_body,
        },
    },
    handlers::orchestration::{register_unavailable, register_with},
    registry::{Context, HandlerRegistry, Response},
    services::{Domains, Lifecycle, ServiceRegistry},
};
use tokio_util::{sync::CancellationToken, task::TaskTracker};

const SERVED: [&str; 7] = [
    "orchestration.subscribeThread",
    "orchestration.getThreadProjection",
    "orchestration.getTurnItem",
    "orchestration.searchThread",
    "orchestration.getArchivedShellSnapshot",
    "orchestration.getThreadFamily",
    "orchestration.getWorktreeThreadIds",
];

struct Fixture {
    _database: TestDatabase,
    sink: EventSink,
    adapter: Arc<OrchestrationAdapter>,
    services: Arc<ServiceRegistry>,
    _home: tempfile::TempDir,
}
impl Fixture {
    async fn new() -> Self {
        let home = tempfile::tempdir().unwrap();
        let inputs = ConfigInputs {
            flags: ServerFlags {
                base_dir: Some(home.path().to_string_lossy().into_owned()),
                host: Some("127.0.0.1".into()),
                port: Some(0),
                ..Default::default()
            },
            env: Default::default(),
            home: home.path().into(),
            process_cwd: home.path().into(),
            bootstrap: None,
            headless: true,
            force_auto_bootstrap: Some(false),
            service_managed: false,
            cli_log_level: None,
            automatic_port: None,
            persisted_observability: Default::default(),
            static_dir: None,
        };
        let local = LocalServices::open(resolve(&inputs).unwrap(), 0)
            .await
            .unwrap();
        let database = TestDatabase::memory().unwrap();
        let projections = Arc::new(ProjectionStore::new(database.stores.clone()));
        let sink = EventSink::new(
            database.stores.clone(),
            projections.clone(),
            Arc::new(ManualClock::new(TEST_TIME)),
            OutboxSignals::default(),
        );
        let transport = Arc::new(TransportService::new(
            projections,
            Arc::new(sink.clone()),
            database.stores.clone(),
        ));
        let adapter = Arc::new(OrchestrationAdapter::new(transport, Arc::new(sink.clone())));
        let services = Arc::new(ServiceRegistry {
            handlers: HandlerRegistry::default(),
            shutdown: CancellationToken::new(),
            connections: TaskTracker::new(),
            lifecycle: Lifecycle::default(),
            domains: Domains::default(),
            local,
        });
        Self {
            _database: database,
            sink,
            adapter,
            services,
            _home: home,
        }
    }
    fn registry(&self) -> HandlerRegistry {
        let mut registry = HandlerRegistry::default();
        register_with(&mut registry, self.adapter.clone()).unwrap();
        registry
    }
    fn context(&self, scopes: &[AuthEnvironmentScope]) -> Context {
        Context {
            services: self.services.clone(),
            scopes: scopes.to_vec(),
            session_id: AuthSessionId("session".into()),
            secret_references: false,
            call: Default::default(),
        }
    }
    async fn commit(
        &self,
        events: Vec<t3_contracts::orchestration_v2::OrchestrationV2DomainEvent>,
    ) -> u64 {
        let result = self
            .sink
            .commit(CommitRequest {
                receipt: None,
                decision: Decision {
                    events,
                    ..Default::default()
                },
                guard: CommitGuard::None,
            })
            .await
            .unwrap();
        result.events.last().unwrap().sequence
    }
    /// A project row and one thread, created through the orchestration crate's own stores.
    async fn seed(&self) -> u64 {
        self._database
            .stores
            .transaction(|tx| {
                t3_orchestration::stores::projects::apply(
                    tx,
                    &project_event("project-create", "project")?,
                )
            })
            .await
            .unwrap();
        self.commit(vec![
            serde_json::from_value(json!({
                "id": "thread-create", "threadId": "thread", "type": "thread.created",
                "occurredAt": TEST_TIME, "payload": thread_json(),
            }))
            .unwrap(),
        ])
        .await
    }
    async fn call(&self, scopes: &[AuthEnvironmentScope], tag: &str, payload: Value) -> Exit {
        let work = self
            .registry()
            .dispatch(self.context(scopes), tag, payload)
            .unwrap_or_else(|exit| panic!("{tag} rejected before dispatch: {exit:?}"));
        futures::pin_mut!(work);
        loop {
            match work.next().await.expect("handler ended without an exit") {
                Response::Exit(exit) => return exit,
                Response::Chunk(_) => {}
            }
        }
    }
    async fn finish(self) {
        drop(self.registry());
        let Self { services, .. } = self;
        Arc::try_unwrap(services)
            .ok()
            .expect("service handles still held")
            .local
            .shutdown()
            .await
            .unwrap();
    }
}
fn thread_json() -> Value {
    json!({"id":"thread","projectId":"project","title":"Test","providerInstanceId":"codex","modelSelection":{"instanceId":"codex","model":"gpt-5"},"runtimeMode":"full-access","interactionMode":"default","branch":null,"worktreePath":null,"activeProviderThreadId":null,"lineage":{"parentThreadId":null,"relationshipToParent":null,"rootThreadId":"thread"},"forkedFrom":null,"createdBy":"user","creationSource":"web","createdAt":TEST_TIME,"updatedAt":TEST_TIME,"archivedAt":null,"settledOverride":null,"settledAt":null,"deletedAt":null})
}
fn success(exit: Exit) -> Value {
    match exit {
        Exit::Success { value: Some(value) } => value,
        other => panic!("expected success, got {other:?}"),
    }
}
fn failure(exit: Exit) -> Value {
    match exit {
        Exit::Failure { cause } => match cause.as_slice() {
            [Cause::Fail { error }] => error.clone(),
            other => panic!("expected one typed failure, got {other:?}"),
        },
        other => panic!("expected a typed failure, got {other:?}"),
    }
}
const READ: &[AuthEnvironmentScope] = &[AuthEnvironmentScope::OrchestrationRead];

#[test]
fn served_methods_are_registered_and_the_rest_stay_unimplemented_with_or_without_the_adapter() {
    let fixture = tokio::runtime::Runtime::new()
        .unwrap()
        .block_on(Fixture::new());
    let with = fixture.registry();
    let mut without = HandlerRegistry::default();
    register_unavailable(&mut without).unwrap();
    let owned: Vec<&str> = WS_METHODS
        .iter()
        .map(|m| m.tag)
        .filter(|tag| {
            tag.starts_with("orchestration.")
                || tag.starts_with("projectClone.")
                || tag.starts_with("projects.")
                    && !matches!(
                        *tag,
                        "projects.searchEntries"
                            | "projects.searchContents"
                            | "projects.listEntries"
                            | "projects.listIgnoredFiles"
                            | "projects.readFile"
                            | "projects.writeFile"
                    )
                || tag.starts_with("agentSessions.")
                || *tag == "subscribeProjectClones"
        })
        .collect();
    assert_eq!(owned.len(), 33, "{owned:?}");
    for tag in owned {
        assert_eq!(
            without.disposition(tag),
            Some(false),
            "{tag} without adapter"
        );
        assert_eq!(
            with.disposition(tag),
            Some(SERVED.contains(&tag)),
            "{tag} with adapter"
        );
    }
}

#[tokio::test]
async fn thread_projection_serves_the_seeded_thread_and_types_a_missing_one() {
    let fixture = Fixture::new().await;
    fixture.seed().await;
    let ok = success(
        fixture
            .call(
                READ,
                "orchestration.getThreadProjection",
                json!({"threadId":"thread"}),
            )
            .await,
    );
    assert_eq!(ok["thread"]["id"], "thread");
    assert_eq!(ok["thread"]["projectId"], "project");
    let missing = failure(
        fixture
            .call(
                READ,
                "orchestration.getThreadProjection",
                json!({"threadId":"missing"}),
            )
            .await,
    );
    assert_eq!(missing["_tag"], "OrchestrationV2GetThreadProjectionError");
    assert_eq!(missing["threadId"], "missing");
    assert_eq!(
        missing["message"],
        "Failed to load orchestration V2 thread missing"
    );
    fixture.finish().await;
}

#[tokio::test]
async fn reads_are_refused_before_the_handler_runs_without_the_read_scope() {
    let fixture = Fixture::new().await;
    fixture.seed().await;
    for scopes in [&[][..], &[AuthEnvironmentScope::OrchestrationOperate][..]] {
        for (tag, payload) in [
            (
                "orchestration.getThreadProjection",
                json!({"threadId":"thread"}),
            ),
            (
                "orchestration.subscribeThread",
                json!({"threadId":"thread"}),
            ),
            ("orchestration.getArchivedShellSnapshot", json!({})),
        ] {
            let outcome = fixture
                .registry()
                .dispatch(fixture.context(scopes), tag, payload);
            let exit = outcome
                .err()
                .unwrap_or_else(|| panic!("{tag} ran with {scopes:?}"));
            assert_eq!(
                failure(exit)["_tag"],
                "EnvironmentAuthorizationError",
                "{tag}"
            );
        }
    }
    fixture.finish().await;
}

#[tokio::test]
async fn turn_item_lookup_returns_null_for_an_unknown_item() {
    let fixture = Fixture::new().await;
    fixture.seed().await;
    let result = success(
        fixture
            .call(
                READ,
                "orchestration.getTurnItem",
                json!({"threadId":"thread","itemId":"absent"}),
            )
            .await,
    );
    assert_eq!(result, json!({"item": null}));
    fixture.finish().await;
}

#[tokio::test]
async fn shell_family_and_worktree_reads_come_from_the_shared_projection() {
    let fixture = Fixture::new().await;
    let sequence = fixture.seed().await;
    let archived = success(
        fixture
            .call(READ, "orchestration.getArchivedShellSnapshot", json!({}))
            .await,
    );
    assert_eq!(archived["projects"][0]["id"], "project");
    assert_eq!(archived["threads"], json!([]));
    assert!(archived["snapshotSequence"].as_u64().unwrap() >= sequence);
    let family = success(
        fixture
            .call(
                READ,
                "orchestration.getThreadFamily",
                json!({"threadId":"thread"}),
            )
            .await,
    );
    let ids: Vec<_> = family["threads"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["id"].clone())
        .collect();
    assert_eq!(ids, vec![json!("thread")]);
    let worktree = success(
        fixture
            .call(
                READ,
                "orchestration.getWorktreeThreadIds",
                json!({"worktreePath":"/nowhere"}),
            )
            .await,
    );
    assert_eq!(worktree, json!({"threadIds": []}));
    fixture.finish().await;
}

#[tokio::test]
async fn thread_stream_sends_snapshot_then_marker_then_each_committed_event_once() {
    let fixture = Fixture::new().await;
    fixture.seed().await;
    let work = fixture
        .registry()
        .dispatch(
            fixture.context(READ),
            "orchestration.subscribeThread",
            json!({"threadId":"thread","requestCompletionMarker":true}),
        )
        .unwrap();
    let mut work = Box::pin(work);
    async fn kind(work: &mut (impl futures::Stream<Item = Response> + Unpin)) -> (String, Value) {
        match work.next().await.unwrap() {
            Response::Chunk(mut values) => {
                assert_eq!(values.len(), 1);
                let value = values.remove(0);
                (value["kind"].as_str().unwrap().to_owned(), value)
            }
            Response::Exit(exit) => panic!("stream ended early: {exit:?}"),
        }
    }
    let (first, snapshot) = kind(&mut work).await;
    assert_eq!(first, "snapshot");
    assert_eq!(snapshot["projection"]["thread"]["id"], "thread");
    assert_eq!(kind(&mut work).await.0, "synchronized");
    let live = fixture
        .commit(vec![detached_event("live", "thread").unwrap()])
        .await;
    let (third, event) = kind(&mut work).await;
    assert_eq!(third, "event");
    assert_eq!(event["sequence"], live);
    // An event of another thread is never delivered on this thread's stream.
    let other = fixture
        .commit(vec![detached_event("other", "elsewhere").unwrap()])
        .await;
    let last = fixture
        .commit(vec![detached_event("last", "thread").unwrap()])
        .await;
    assert!(last > other);
    let (fourth, event) = kind(&mut work).await;
    assert_eq!(fourth, "event");
    assert_eq!(event["sequence"], last);
    drop(work);
    fixture.finish().await;
}

#[tokio::test]
async fn thread_stream_resumes_from_an_exact_cursor_without_a_snapshot() {
    let fixture = Fixture::new().await;
    let base = fixture.seed().await;
    let replayed = fixture
        .commit(vec![detached_event("replayed", "thread").unwrap()])
        .await;
    let work = fixture
        .registry()
        .dispatch(
            fixture.context(READ),
            "orchestration.subscribeThread",
            json!({"threadId":"thread","afterSequence":base,"requestCompletionMarker":true}),
        )
        .unwrap();
    let mut work = Box::pin(work);
    let Response::Chunk(first) = work.next().await.unwrap() else {
        panic!("stream ended early")
    };
    assert_eq!(first[0]["kind"], "event");
    assert_eq!(first[0]["sequence"], replayed);
    let Response::Chunk(second) = work.next().await.unwrap() else {
        panic!("stream ended early")
    };
    assert_eq!(second[0]["kind"], "synchronized");
    drop(work);
    fixture.finish().await;
}

#[tokio::test]
async fn an_unknown_thread_fails_the_stream_with_a_typed_error() {
    let fixture = Fixture::new().await;
    let work = fixture
        .registry()
        .dispatch(
            fixture.context(READ),
            "orchestration.subscribeThread",
            json!({"threadId":"missing"}),
        )
        .unwrap();
    futures::pin_mut!(work);
    let Response::Exit(exit) = work.next().await.unwrap() else {
        panic!("a missing thread produced a chunk")
    };
    let error = failure(exit);
    assert_eq!(error["_tag"], "OrchestrationV2GetThreadProjectionError");
    assert_eq!(error["threadId"], "missing");
    fixture.finish().await;
}

#[test]
fn clock_formats_like_to_iso_string() {
    for (millis, expected) in [
        (0, "1970-01-01T00:00:00.000Z"),
        (1_700_000_000_123, "2023-11-14T22:13:20.123Z"),
        (1_709_164_800_000, "2024-02-29T00:00:00.000Z"),
        (4_102_444_799_999, "2099-12-31T23:59:59.999Z"),
        (-1, "1969-12-31T23:59:59.999Z"),
    ] {
        assert_eq!(iso_from_unix_millis(millis).0, expected);
    }
}

#[test]
fn protocol_header_must_be_exactly_two() {
    assert!(protocol_header_accepted(Some("2")));
    for value in [
        None,
        Some("1"),
        Some("02"),
        Some(" 2"),
        Some("2.0"),
        Some(""),
    ] {
        assert!(!protocol_header_accepted(value), "{value:?}");
    }
    let body = protocol_mismatch_body();
    assert_eq!(body["code"], "orchestration_protocol_incompatible");
    assert_eq!(body["orchestrationProtocolVersion"], 2);
}
