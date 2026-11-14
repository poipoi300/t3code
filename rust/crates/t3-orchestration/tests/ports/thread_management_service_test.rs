#![allow(clippy::unwrap_used)]

use super::orchestrator_receipt_race_test::Harness;
use async_trait::async_trait;
use serde_json::{Value, json};
use std::{
    sync::{Arc, Mutex},
    time::Duration,
};
use t3_orchestration::{
    api::*,
    error::{Error, Result},
    ports::thread_project::thread_management_service::*,
    stores::{
        outbox,
        receipts::{self, AggregateId, CommandReceipt, ReceiptStatus},
    },
    testing::TEST_TIME,
};
use tokio::sync::{mpsc, watch};
use tokio_util::sync::CancellationToken;

pub(super) struct Settings(pub watch::Sender<Arc<t3_contracts::settings::ServerSettings>>);
#[async_trait]
impl SettingsService for Settings {
    async fn get(&self) -> Result<t3_contracts::settings::ServerSettings> {
        Ok((**self.0.borrow()).clone())
    }
    fn subscribe(&self) -> Result<watch::Receiver<Arc<t3_contracts::settings::ServerSettings>>> {
        Ok(self.0.subscribe())
    }
}
pub(super) fn settings(value: Value) -> Arc<Settings> {
    Arc::new(Settings(
        watch::channel(Arc::new(serde_json::from_value(value).unwrap())).0,
    ))
}
pub(super) struct Recorder {
    pub commands: Mutex<Vec<Value>>,
    pub downstream: Option<Arc<dyn CommandDispatcher>>,
}
#[async_trait]
impl CommandDispatcher for Recorder {
    async fn dispatch(&self, command: OrchestrationV2ServerCommand) -> Result<CommandReceipt> {
        let value = serde_json::to_value(&command)?;
        self.commands.lock().unwrap().push(value.clone());
        if let Some(dispatcher) = &self.downstream {
            return dispatcher.dispatch(command).await;
        }
        Ok(CommandReceipt {
            command_id: serde_json::from_value(value["commandId"].clone())?,
            aggregate: AggregateId::Thread(serde_json::from_value(
                value
                    .get("threadId")
                    .or_else(|| value.get("parentThreadId"))
                    .unwrap()
                    .clone(),
            )?),
            command_type: value["type"].as_str().unwrap().into(),
            accepted_at: TEST_TIME.into(),
            result_sequence: 1,
            status: ReceiptStatus::Accepted,
            error: None,
        })
    }
}
pub(super) struct NoRestore;
#[async_trait]
impl ThreadWorktreeRestore for NoRestore {
    async fn restore(&self, _: OrchestrationV2AppThread) -> Result<()> {
        Ok(())
    }
}
struct Hydration(mpsc::UnboundedSender<ThreadId>);
#[async_trait]
impl TranscriptHydrator for Hydration {
    async fn ensure(&self, id: ThreadId) -> Result<()> {
        let _ = self.0.send(id);
        Ok(())
    }
}
pub(super) fn service(
    harness: &Harness,
    settings: Arc<dyn SettingsService>,
    downstream: bool,
) -> (
    ThreadManagementService,
    Arc<Recorder>,
    mpsc::UnboundedReceiver<ThreadId>,
) {
    let recorder = Arc::new(Recorder {
        commands: Mutex::new(Vec::new()),
        downstream: downstream
            .then(|| Arc::new(harness.dispatcher.clone()) as Arc<dyn CommandDispatcher>),
    });
    let (sender, receiver) = mpsc::unbounded_channel();
    (
        ThreadManagementService::new(ThreadManagementServices {
            reader: harness.reader.clone(),
            dispatcher: recorder.clone(),
            committer: harness.sink.clone(),
            database: harness.database.stores.clone(),
            settings,
            worktree_restore: Arc::new(NoRestore),
            hydrator: Arc::new(Hydration(sender)),
            signals: harness.sink.signals(),
            clock: harness.clock.clone(),
        }),
        recorder,
        receiver,
    )
}
pub(super) async fn emit(harness: &Harness, id: &str, kind: &str, payload: Value) -> Result<()> {
    harness.sink.commit(CommitRequest { receipt: None, decision: Decision { events: vec![serde_json::from_value(json!({"id":id,"threadId":"thread","type":kind,"occurredAt":TEST_TIME,"payload":payload}))?], ..Default::default() }, guard: CommitGuard::None }).await?;
    Ok(())
}
pub(super) fn sample(name: &str) -> Value {
    let source = std::path::Path::new(file!());
    let fixtures = if source.is_absolute() {
        source
            .parent()
            .unwrap()
            .join("../../../../fixtures/samples")
    } else {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/samples")
    };
    let path = fixtures.join(format!("{name}.jsonl"));
    serde_json::from_str(
        std::fs::read_to_string(path)
            .unwrap()
            .lines()
            .next()
            .unwrap(),
    )
    .unwrap()
}
pub(super) fn run(id: &str, ordinal: u64, status: &str) -> Value {
    let mut value = sample("OrchestrationV2Run");
    value.as_object_mut().unwrap().retain(|key, _| {
        !matches!(
            key.as_str(),
            "restartContinuationOfRunId" | "delegatedCompletion" | "queueHeld"
        )
    });
    value["id"] = json!(id);
    value["threadId"] = json!("thread");
    value["ordinal"] = json!(ordinal);
    value["status"] = json!(status);
    value["providerInstanceId"] = json!("codex");
    value["providerThreadId"] = json!("provider-thread");
    value["activeAttemptId"] = json!("attempt");
    value["rootNodeId"] = json!("node");
    value["userMessageId"] = json!(format!("message:{id}"));
    value["requestedAt"] = json!(TEST_TIME);
    value["startedAt"] = json!(TEST_TIME);
    value["completedAt"] = if status == "running" {
        Value::Null
    } else {
        json!(TEST_TIME)
    };
    value
}
#[tokio::test]
async fn provenance_overwrites_clients_and_removes_forged_link_origin() -> Result<()> {
    let harness = Harness::new(None, false);
    harness.create("thread").await?;
    let command = serde_json::from_value(
        json!({"type":"thread.create","commandId":"create","threadId":"new","projectId":"project","title":"Title","modelSelection":{"instanceId":"codex","model":"test"},"runtimeMode":"full-access","interactionMode":"default","branch":null,"worktreePath":null,"createdBy":"agent","creationSource":"mcp","linkOrigin":{"sessionId":"forged","label":"Elsewhere"},"delegatedFrom":{"environmentId":"environment","threadId":"parent","title":"Parent"}}),
    )?;
    let stamped = serde_json::to_value(with_creation_provenance(
        command,
        OrchestrationV2Actor::User,
        OrchestrationV2CreationSource::Web,
    )?)?;
    assert_eq!(stamped["createdBy"], "user");
    assert_eq!(stamped["creationSource"], "web");
    assert!(stamped.get("linkOrigin").is_none());
    assert!(stamped.get("delegatedFrom").is_none());
    Ok(())
}
#[tokio::test]
async fn project_disabled_dispatch_is_rejected_but_durable_receipt_replay_is_admitted() -> Result<()>
{
    let harness = Harness::new(None, false);
    harness.create("thread").await?;
    let (service, recorder, _) = service(
        &harness,
        settings(
            json!({"projectSettingsOverrides":{"project":{"providerInstances":{"codex":{"enabled":false}}}}}),
        ),
        false,
    );
    let command = || {
        serde_json::from_value(json!({"type":"message.dispatch","commandId":"disabled","threadId":"thread","messageId":"message","text":"Done","attachments":[],"dispatchMode":{"type":"queue_after_active"},"createdBy":"agent","creationSource":"server"})).unwrap()
    };
    assert!(matches!(
        service.dispatch(command()).await,
        Err(Error::Service {
            operation: "ThreadManagementProviderInstanceDisabledError",
            ..
        })
    ));
    assert!(recorder.commands.lock().unwrap().is_empty());
    harness
        .database
        .stores
        .transaction(|tx| {
            receipts::insert_if_absent(
                tx,
                &CommandReceipt {
                    command_id: CommandId("disabled".into()),
                    aggregate: AggregateId::Thread(ThreadId("thread".into())),
                    command_type: "message.dispatch".into(),
                    accepted_at: TEST_TIME.into(),
                    result_sequence: 1,
                    status: ReceiptStatus::Accepted,
                    error: None,
                },
            )?;
            Ok(())
        })
        .await?;
    service.dispatch(command()).await?;
    assert_eq!(recorder.commands.lock().unwrap().len(), 1);
    Ok(())
}

#[tokio::test]
async fn send_restores_before_start_but_live_steering_and_receipt_replay_survive_restore_failure()
-> Result<()> {
    struct Restore {
        calls: std::sync::atomic::AtomicUsize,
        commands: Arc<Recorder>,
        fails: bool,
        cancelled: bool,
    }
    #[async_trait]
    impl ThreadWorktreeRestore for Restore {
        async fn restore(&self, thread: OrchestrationV2AppThread) -> Result<()> {
            assert_eq!(json!(thread.worktree_path), "C:/worktrees/gone");
            assert!(self.commands.commands.lock().unwrap().is_empty());
            self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            if self.cancelled {
                return Err(Error::Cancelled);
            }
            if self.fails {
                return Err(Error::Service {
                    operation: "WorktreeRecreateError",
                    detail: "branch_checked_out_in_worktree".into(),
                });
            }
            Ok(())
        }
    }
    for (mode, running, fresh, replay, fails, cancelled, restores, dispatched) in [
        ("auto", false, false, false, true, false, 1, false),
        ("auto", true, false, false, true, false, 0, true),
        ("steer", true, false, false, true, false, 0, true),
        ("queue", true, false, false, true, false, 1, false),
        ("restart", true, false, false, true, false, 1, false),
        ("steer", true, true, false, true, false, 1, false),
        ("auto", false, false, true, true, false, 1, true),
        ("auto", false, false, false, false, false, 1, true),
        ("auto", false, false, true, false, true, 1, false),
    ] {
        let harness = Harness::new(None, false);
        harness.create("thread").await?;
        let mut thread = json!(
            harness
                .reader
                .thread(ThreadId("thread".into()))
                .await?
                .unwrap()
        );
        thread["worktreePath"] = json!("C:/worktrees/gone");
        thread["branch"] = json!("t3/gone");
        emit(&harness, "worktree", "thread.metadata-updated", thread).await?;
        if running {
            emit(&harness, "run", "run.created", run("active", 1, "running")).await?;
            let mut turn = sample("OrchestrationV2ProviderTurn");
            turn["id"] = json!("turn");
            turn["providerThreadId"] = json!("provider-thread");
            turn["runAttemptId"] = json!("attempt");
            turn["status"] = json!("running");
            emit(&harness, "turn", "provider-turn.updated", turn).await?;
        }
        if replay {
            harness
                .database
                .stores
                .transaction(|db| {
                    receipts::upsert(
                        db,
                        &CommandReceipt {
                            command_id: CommandId("send".into()),
                            aggregate: AggregateId::Thread(ThreadId("thread".into())),
                            command_type: "message.dispatch".into(),
                            accepted_at: TEST_TIME.into(),
                            result_sequence: 1,
                            status: ReceiptStatus::Accepted,
                            error: None,
                        },
                    )
                })
                .await?;
        }
        let commands = Arc::new(Recorder {
            commands: Mutex::new(Vec::new()),
            downstream: None,
        });
        let restore = Arc::new(Restore {
            calls: std::sync::atomic::AtomicUsize::new(0),
            commands: commands.clone(),
            fails,
            cancelled,
        });
        let service = ThreadManagementService::new(ThreadManagementServices {
            reader: harness.reader.clone(),
            dispatcher: commands.clone(),
            committer: harness.sink.clone(),
            database: harness.database.stores.clone(),
            settings: settings(json!({})),
            hydrator: Arc::new(NoLegacyTranscript),
            worktree_restore: restore.clone(),
            signals: harness.sink.signals(),
            clock: harness.clock.clone(),
        });
        let result=service.send_to_thread(serde_json::from_value(json!({"projectId":"project","threadId":"thread","commandId":"send","messageId":"new-message","text":"Reply with ok.","attachments":[],"mode":mode,"freshContext":fresh,"createdBy":"agent","creationSource":"mcp"}))?).await;
        let error = result.err().expect("recorder creates no durable message");
        if cancelled {
            assert!(matches!(error, Error::Cancelled));
        } else if dispatched {
            assert!(
                matches!(
                    error,
                    Error::Service {
                        operation: "ThreadManagementDurableRunProjectionError",
                        ..
                    }
                ),
                "{error}"
            );
        } else {
            assert!(
                matches!(&error,Error::Service{operation:"ThreadManagementWorktreeRestoreError",detail} if detail.contains("thread")&&detail.contains("branch_checked_out_in_worktree")),
                "{error}"
            );
        }
        assert_eq!(
            restore.calls.load(std::sync::atomic::Ordering::SeqCst),
            restores,
            "{mode}/{running}/{fresh}/{replay}"
        );
        assert_eq!(
            commands.commands.lock().unwrap().len(),
            usize::from(dispatched)
        );
        service.shutdown().await;
    }
    Ok(())
}
#[tokio::test]
async fn create_rename_archive_and_unarchive_commit_through_management() -> Result<()> {
    let harness = Harness::new(None, false);
    let (service, _, mut hydration) = service(&harness, settings(json!({})), true);
    let create = json!({"type":"thread.create","commandId":"create:thread","threadId":"thread","projectId":"project","title":"Before","modelSelection":{"instanceId":"codex","model":"a"},"runtimeMode":"approval-required","interactionMode":"default","branch":null,"worktreePath":null,"createdBy":"user","creationSource":"web"});
    service.dispatch(serde_json::from_value(create)?).await?;
    assert!(hydration.try_recv().is_err());
    let mut events = harness.sink.subscribe();
    service.dispatch(serde_json::from_value(json!({"type":"thread.metadata.update","commandId":"rename","threadId":"thread","title":"After"}))?).await?;
    assert!(matches!(
        events.recv().await.unwrap(),
        t3_contracts::application_event::ApplicationStoredEvent::Orchestration(_)
    ));
    assert_eq!(
        harness
            .reader
            .thread(ThreadId("thread".into()))
            .await?
            .unwrap()
            .title
            .0,
        "After"
    );
    for kind in ["thread.archive", "thread.unarchive"] {
        service
            .dispatch(serde_json::from_value(
                json!({"type":kind,"commandId":kind,"threadId":"thread"}),
            )?)
            .await?;
        assert_eq!(
            harness
                .reader
                .thread(ThreadId("thread".into()))
                .await?
                .unwrap()
                .archived_at
                .is_some(),
            kind == "thread.archive"
        );
    }
    Ok(())
}
#[tokio::test]
async fn waits_for_restart_successor_and_wakes_when_continuation_is_declined() -> Result<()> {
    for declined in [false, true] {
        let harness = Arc::new(Harness::new(None, false));
        harness.create("thread").await?;
        emit(
            &harness,
            "run:first",
            "run.created",
            run("first", 1, "cancelled"),
        )
        .await?;
        harness
            .database
            .stores
            .transaction(|tx| {
                outbox::enqueue(
                    tx,
                    &[t3_orchestration::effects::PendingEffect {
                        id: "continue".into(),
                        command_id: CommandId("continue".into()),
                        thread_id: ThreadId("thread".into()),
                        request: t3_orchestration::effects::EffectRequest::Continue {
                            source_run_id: RunId("first".into()),
                        },
                        available_at: None,
                    }],
                    TEST_TIME,
                )
            })
            .await?;
        let (service, _, mut hydration) = service(&harness, settings(json!({})), false);
        let waiting = tokio::spawn(async move {
            service
                .wait_for_thread(
                    WaitInput {
                        project_id: ProjectId("project".into()),
                        thread_id: ThreadId("thread".into()),
                        run_id: Some(RunId("first".into())),
                        timeout: Duration::from_secs(3600),
                        until_delegated_work_settled: false,
                    },
                    CancellationToken::new(),
                )
                .await
        });
        hydration.recv().await.unwrap();
        hydration.recv().await.unwrap();
        if !declined {
            let mut next = run("successor", 2, "running");
            next["restartContinuationOfRunId"] = json!("first");
            emit(&harness, "run:next", "run.created", next.clone()).await?;
            next["status"] = json!("completed");
            next["completedAt"] = json!(TEST_TIME);
            emit(&harness, "run:done", "run.updated", next).await?;
        }
        harness
            .database
            .stores
            .transaction(|tx| {
                outbox::cancel_unsettled(
                    tx,
                    &ThreadId("thread".into()),
                    &["provider-runtime.continue"],
                    "declined",
                    TEST_TIME,
                )?;
                Ok(())
            })
            .await?;
        harness.sink.signals().notify();
        let result = waiting.await.unwrap()?;
        assert!(!result.timed_out);
        assert_eq!(
            json!(result.run.unwrap().id),
            if declined { "first" } else { "successor" }
        );
        assert!(result.result_run.is_none());
    }
    Ok(())
}
#[tokio::test]
async fn wait_checks_membership_empty_threads_cancellation_and_terminal_rollback() -> Result<()> {
    let harness = Harness::new(None, false);
    harness.create("thread").await?;
    let (service, _, _) = service(&harness, settings(json!({})), false);
    let input = |run_id, delegated| WaitInput {
        project_id: ProjectId("project".into()),
        thread_id: ThreadId("thread".into()),
        run_id,
        timeout: Duration::from_secs(3600),
        until_delegated_work_settled: delegated,
    };
    assert!(
        service
            .wait_for_thread(input(None, false), CancellationToken::new())
            .await?
            .run
            .is_none()
    );
    assert!(
        service
            .wait_for_thread(
                input(Some(RunId("missing".into())), false),
                CancellationToken::new()
            )
            .await
            .is_err()
    );
    emit(
        &harness,
        "run:rollback",
        "run.created",
        run("rollback", 1, "rolled_back"),
    )
    .await?;
    let result = service
        .wait_for_thread(input(None, true), CancellationToken::new())
        .await?;
    assert!(!result.timed_out);
    assert_eq!(json!(result.result_run.unwrap().id), "rollback");
    emit(
        &harness,
        "run:active",
        "run.created",
        run("active", 2, "running"),
    )
    .await?;
    let cancel = CancellationToken::new();
    cancel.cancel();
    assert!(matches!(
        service.wait_for_thread(input(None, false), cancel).await,
        Err(Error::Cancelled)
    ));
    assert!(
        service
            .project_records(
                &ProjectId("other".into()),
                ThreadId("thread".into()),
                vec![],
                RecordFilter::default()
            )
            .await
            .is_err()
    );
    Ok(())
}

struct ExportHydrator {
    started: mpsc::UnboundedSender<String>,
    gates: std::collections::BTreeMap<String, Arc<tokio::sync::Semaphore>>,
}

#[tokio::test]
async fn shutdown_cancels_exports_and_waits_across_clones_and_rejects_late_mutations() -> Result<()>
{
    let harness = Harness::new(None, false);
    harness.create("thread").await?;
    emit(
        &harness,
        "active",
        "run.created",
        run("active", 1, "running"),
    )
    .await?;
    let (sender, mut started) = mpsc::unbounded_channel();
    let service = ThreadManagementService::new(ThreadManagementServices {
        reader: harness.reader.clone(),
        dispatcher: Arc::new(harness.dispatcher.clone()),
        committer: harness.sink.clone(),
        database: harness.database.stores.clone(),
        settings: settings(json!({})),
        worktree_restore: Arc::new(NoRestore),
        hydrator: Arc::new(ExportHydrator {
            started: sender,
            gates: [("thread".into(), Arc::new(tokio::sync::Semaphore::new(0)))]
                .into_iter()
                .collect(),
        }),
        signals: harness.sink.signals(),
        clock: harness.clock.clone(),
    });
    let exporting = tokio::spawn({
        let service = service.clone();
        async move {
            service
                .transcript(
                    ThreadId("thread".into()),
                    EnvironmentId("environment".into()),
                    10000,
                )
                .await
        }
    });
    assert_eq!(started.recv().await.as_deref(), Some("thread"));
    service.shutdown().await;
    assert!(matches!(exporting.await.unwrap(), Err(Error::Cancelled)));
    assert!(matches!(
        service
            .transcript(
                ThreadId("thread".into()),
                EnvironmentId("environment".into()),
                10000
            )
            .await,
        Err(Error::Cancelled)
    ));
    assert!(matches!(
        service
            .stop_thread(
                CommandId("late".into()),
                ThreadId("thread".into()),
                None,
                true
            )
            .await,
        Err(Error::Cancelled)
    ));
    service.shutdown().await;
    let (service, _, mut hydration) =
        super::thread_management_service_test::service(&harness, settings(json!({})), false);
    let waiting = tokio::spawn({
        let service = service.clone();
        async move {
            service
                .wait_for_thread(
                    WaitInput {
                        project_id: ProjectId("project".into()),
                        thread_id: ThreadId("thread".into()),
                        run_id: None,
                        timeout: Duration::from_secs(3600),
                        until_delegated_work_settled: false,
                    },
                    CancellationToken::new(),
                )
                .await
        }
    });
    hydration.recv().await.unwrap();
    service.shutdown().await;
    assert!(matches!(waiting.await.unwrap(), Err(Error::Cancelled)));
    Ok(())
}

#[async_trait]
impl TranscriptHydrator for ExportHydrator {
    async fn ensure(&self, id: ThreadId) -> Result<()> {
        self.started.send(id.0.clone()).unwrap();
        self.gates[&id.0].acquire().await.unwrap().forget();
        if id.0 == "a" {
            return Err(Error::Service {
                operation: "test.export",
                detail: "snapshot failed".into(),
            });
        }
        Ok(())
    }
}

#[tokio::test]
async fn transcript_export_limits_admission_and_releases_slots_on_errors_and_cancellation()
-> Result<()> {
    let harness = Harness::new(None, false);
    for id in ["a", "b", "c", "d", "e", "f"] {
        harness.create(id).await?;
    }
    let (sender, mut started) = mpsc::unbounded_channel();
    let gates: std::collections::BTreeMap<_, _> = ["a", "b", "c", "d", "e", "f"]
        .into_iter()
        .map(|id| (id.to_owned(), Arc::new(tokio::sync::Semaphore::new(0))))
        .collect();
    let service = ThreadManagementService::new(ThreadManagementServices {
        reader: harness.reader.clone(),
        dispatcher: Arc::new(harness.dispatcher.clone()),
        committer: harness.sink.clone(),
        database: harness.database.stores.clone(),
        settings: settings(json!({})),
        worktree_restore: Arc::new(NoRestore),
        hydrator: Arc::new(ExportHydrator {
            started: sender,
            gates: gates.clone(),
        }),
        signals: harness.sink.signals(),
        clock: harness.clock.clone(),
    });
    let export = |id: &'static str| {
        let service = service.clone();
        tokio::spawn(async move {
            service
                .transcript(
                    ThreadId(id.into()),
                    EnvironmentId("environment".into()),
                    1024 * 1024,
                )
                .await
        })
    };
    let a = export("a");
    let b = export("b");
    let first = [started.recv().await.unwrap(), started.recv().await.unwrap()];
    assert_eq!(
        first.into_iter().collect::<std::collections::BTreeSet<_>>(),
        ["a".to_owned(), "b".to_owned()].into_iter().collect()
    );
    let c = export("c");
    let d = export("d");
    let e = export("e");
    let f = export("f");
    c.abort();
    assert!(c.await.unwrap_err().is_cancelled());
    gates["a"].add_permits(1);
    assert!(a.await.unwrap().is_err());
    let next = started.recv().await.unwrap();
    assert!(["d", "e", "f"].contains(&next.as_str()));
    b.abort();
    assert!(b.await.unwrap_err().is_cancelled());
    let second = started.recv().await.unwrap();
    assert_ne!(next, second);
    gates[&next].add_permits(1);
    let last = started.recv().await.unwrap();
    assert_eq!(
        [next, second.clone(), last.clone()]
            .into_iter()
            .collect::<std::collections::BTreeSet<_>>(),
        ["d".to_owned(), "e".to_owned(), "f".to_owned()]
            .into_iter()
            .collect()
    );
    gates[&second].add_permits(1);
    gates[&last].add_permits(1);
    assert_eq!(d.await.unwrap()?.thread_id.0, "d");
    assert_eq!(e.await.unwrap()?.thread_id.0, "e");
    assert_eq!(f.await.unwrap()?.thread_id.0, "f");
    assert!(started.try_recv().is_err());
    Ok(())
}

#[tokio::test]
async fn wait_for_delegated_work_keeps_waiting_for_awaited_native_background_tasks() -> Result<()> {
    let harness = Harness::new(None, false);
    harness.create("thread").await?;
    emit(
        &harness,
        "completed",
        "run.created",
        run("completed", 1, "completed"),
    )
    .await?;
    let mut provider = sample("OrchestrationV2ProviderThread");
    provider["id"] = json!("provider-thread");
    provider["appThreadId"] = json!("thread");
    provider["pendingBackgroundTasks"] =
        json!([{"taskId":"tests","kind":"command","awaited":true}]);
    emit(
        &harness,
        "background",
        "provider-thread.updated",
        provider.clone(),
    )
    .await?;
    let (service, _, mut hydration) = service(&harness, settings(json!({})), false);
    let mut waiting = tokio::spawn(async move {
        service
            .wait_for_thread(
                WaitInput {
                    project_id: ProjectId("project".into()),
                    thread_id: ThreadId("thread".into()),
                    run_id: None,
                    timeout: Duration::from_secs(3600),
                    until_delegated_work_settled: true,
                },
                CancellationToken::new(),
            )
            .await
    });
    hydration.recv().await.unwrap();
    hydration.recv().await.unwrap();
    emit(
        &harness,
        "background-still-running",
        "provider-thread.updated",
        provider.clone(),
    )
    .await?;
    tokio::select! {
        id = hydration.recv() => assert_eq!(id.unwrap().0, "thread"),
        result = &mut waiting => panic!("returned while native work was still awaited: {result:?}"),
    }
    provider["pendingBackgroundTasks"] = json!([]);
    emit(
        &harness,
        "background-completed",
        "provider-thread.updated",
        provider,
    )
    .await?;
    let result = waiting.await.unwrap()?;
    assert!(!result.timed_out);
    assert_eq!(json!(result.result_run.unwrap().id), "completed");
    Ok(())
}

#[tokio::test]
async fn settle_after_run_rechecks_guarded_dispatch_races_and_followup_provenance() -> Result<()> {
    struct RacingSettle {
        recorder: Recorder,
        entered: mpsc::UnboundedSender<Value>,
        release: tokio::sync::Semaphore,
        attempts: std::sync::atomic::AtomicUsize,
        rejects: usize,
    }
    #[async_trait]
    impl CommandDispatcher for RacingSettle {
        async fn dispatch(&self, command: OrchestrationV2ServerCommand) -> Result<CommandReceipt> {
            let value = json!(command);
            let attempt = self
                .attempts
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            if attempt < self.rejects {
                self.entered.send(value.clone()).unwrap();
                self.release.acquire().await.unwrap().forget();
                return Err(Error::Rejected {
                    command_id: serde_json::from_value(value["commandId"].clone())?,
                    reason: "Thread is still running".into(),
                });
            }
            self.recorder.dispatch(command).await
        }
    }

    for (status, followup, settles) in [
        ("completed", "none", true),
        ("failed", "none", false),
        ("interrupted", "none", false),
        ("completed", "notification", true),
        ("completed", "rejected-notification", true),
        ("completed", "delegated", true),
        ("completed", "failed-notification", false),
        ("completed", "held-notification", true),
        ("completed", "user", false),
        ("completed", "steered-notification", false),
        ("completed", "new-watch", false),
    ] {
        let harness = Harness::new(None, false);
        harness.create("thread").await?;
        emit(
            &harness,
            "original",
            "run.created",
            run("original", 1, status),
        )
        .await?;
        let (entered, mut attempts) = mpsc::unbounded_channel();
        let dispatcher = Arc::new(RacingSettle {
            recorder: Recorder {
                commands: Mutex::new(Vec::new()),
                downstream: None,
            },
            entered,
            release: tokio::sync::Semaphore::new(0),
            attempts: std::sync::atomic::AtomicUsize::new(0),
            rejects: if followup == "none" {
                0
            } else if followup == "rejected-notification" {
                2
            } else {
                1
            },
        });
        let (hydration, _) = mpsc::unbounded_channel();
        let service = ThreadManagementService::new(ThreadManagementServices {
            reader: harness.reader.clone(),
            dispatcher: dispatcher.clone(),
            committer: harness.sink.clone(),
            database: harness.database.stores.clone(),
            settings: settings(json!({})),
            worktree_restore: Arc::new(NoRestore),
            hydrator: Arc::new(Hydration(hydration)),
            signals: harness.sink.signals(),
            clock: harness.clock.clone(),
        });
        let settling = tokio::spawn(async move {
            service
                .settle_after_run(
                    ProjectId("project".into()),
                    ThreadId("thread".into()),
                    RunId("original".into()),
                    CancellationToken::new(),
                )
                .await
        });
        if followup != "none" {
            let first = attempts.recv().await.unwrap();
            assert_eq!(first["expectedLatestRunId"], "original");
            let mut message = json!(super::command_policy_test::message("followup"));
            if followup.ends_with("notification") || followup == "new-watch" {
                message["notification"] = json!({"source":{"kind":"monitor"},"outcome":"updated","summary":"Check finished"});
            }
            if followup == "delegated" {
                message["delegatedCompletion"] =
                    json!({"parentRunId":"original","generation":1,"taskIds":["child"]});
            }
            emit(&harness, "followup-message", "message.updated", message).await?;
            let following_status = match followup {
                "failed-notification" => "failed",
                "held-notification" => "queued",
                _ => "completed",
            };
            let next = run("followup", 2, following_status);
            emit(&harness, "followup-run", "run.created", next.clone()).await?;
            if followup == "steered-notification" {
                let mut steer = json!(super::command_policy_test::message("followup"));
                steer["id"] = json!("user-steer");
                emit(&harness, "steer", "message.updated", steer).await?;
            }
            if followup == "new-watch" {
                let mut thread = json!(
                    harness
                        .reader
                        .thread(ThreadId("thread".into()))
                        .await?
                        .unwrap()
                );
                thread["pullRequests"] = json!([{"host":"github.com","repository":"owner/repo","number":42,"url":"https://github.com/owner/repo/pull/42","source":"manual","linkedAt":TEST_TIME,"snapshot":null,"stack":null,"watch":{"startedAt":TEST_TIME,"headSha":null,"failedChecks":[],"passedChecks":[],"passed":false,"remarksThrough":TEST_TIME,"remarkIds":[],"conflicting":false,"wakes":0}}]);
                emit(&harness, "new-watch", "thread.metadata-updated", thread).await?;
            }
            dispatcher.release.add_permits(1);
            if followup == "rejected-notification" {
                let rejected_again = attempts.recv().await.unwrap();
                assert_eq!(rejected_again["expectedLatestRunId"], "followup");
                assert_ne!(rejected_again["commandId"], first["commandId"]);
                emit(&harness, "replayed-update", "run.updated", next).await?;
                dispatcher.release.add_permits(1);
            }
        }
        settling.await.unwrap()?;
        let commands = dispatcher.recorder.commands.lock().unwrap();
        assert_eq!(commands.len(), usize::from(settles), "{status}/{followup}");
        if settles {
            assert_eq!(commands[0]["type"], "thread.settle");
            assert_eq!(
                commands[0]["expectedLatestRunId"],
                if followup == "none" {
                    "original"
                } else {
                    "followup"
                }
            );
        }
    }
    Ok(())
}

#[tokio::test]
async fn settle_after_run_preserves_a_durable_rejection_and_uses_a_new_event_sequence() -> Result<()>
{
    struct Observed {
        downstream: Arc<dyn CommandDispatcher>,
        attempts: mpsc::UnboundedSender<(Value, bool)>,
    }
    #[async_trait]
    impl CommandDispatcher for Observed {
        async fn dispatch(&self, command: OrchestrationV2ServerCommand) -> Result<CommandReceipt> {
            let value = json!(command);
            let result = self.downstream.dispatch(command).await;
            self.attempts.send((value, result.is_ok())).unwrap();
            result
        }
    }
    let harness = Harness::new(None, false);
    harness.create("thread").await?;
    let completed = run("original", 1, "completed");
    emit(&harness, "original", "run.created", completed.clone()).await?;
    let sequence = harness
        .sink
        .latest_sequence(Some(ThreadId("thread".into())))
        .await?;
    let rejected_id = CommandId(format!("server:settle-after-run:original:{sequence}"));
    let rejected = CommandReceipt {
        command_id: rejected_id.clone(),
        aggregate: AggregateId::Thread(ThreadId("thread".into())),
        command_type: "thread.settle".into(),
        accepted_at: TEST_TIME.into(),
        result_sequence: sequence as i64,
        status: ReceiptStatus::Rejected,
        error: Some("Thread is still running".into()),
    };
    let stored = rejected.clone();
    harness
        .database
        .stores
        .transaction(move |db| receipts::upsert(db, &stored))
        .await?;
    let (attempts, mut attempted) = mpsc::unbounded_channel();
    let (hydration, _) = mpsc::unbounded_channel();
    let service = ThreadManagementService::new(ThreadManagementServices {
        reader: harness.reader.clone(),
        dispatcher: Arc::new(Observed {
            downstream: Arc::new(harness.dispatcher.clone()),
            attempts,
        }),
        committer: harness.sink.clone(),
        database: harness.database.stores.clone(),
        settings: settings(json!({})),
        worktree_restore: Arc::new(NoRestore),
        hydrator: Arc::new(Hydration(hydration)),
        signals: harness.sink.signals(),
        clock: harness.clock.clone(),
    });
    let settling = tokio::spawn(async move {
        service
            .settle_after_run(
                ProjectId("project".into()),
                ThreadId("thread".into()),
                RunId("original".into()),
                CancellationToken::new(),
            )
            .await
    });
    let (first, accepted) = attempted.recv().await.unwrap();
    assert!(!accepted);
    assert_eq!(first["commandId"], json!(rejected_id));
    assert!(matches!(
        harness
            .dispatcher
            .dispatch(serde_json::from_value(first)?)
            .await,
        Err(Error::Rejected { .. })
    ));
    emit(&harness, "completed-again", "run.updated", completed).await?;
    let (next, accepted) = attempted.recv().await.unwrap();
    assert!(accepted);
    assert_ne!(next["commandId"], json!(rejected_id));
    settling.await.unwrap()?;
    assert_eq!(
        harness
            .database
            .stores
            .read(move |db| receipts::get(db, &rejected_id))
            .await?,
        Some(rejected)
    );
    assert_eq!(
        json!(
            harness
                .reader
                .thread(ThreadId("thread".into()))
                .await?
                .unwrap()
                .settled_override
        ),
        "settled"
    );
    Ok(())
}

#[tokio::test]
async fn delegated_stop_walks_each_child_before_its_siblings_and_continues_after_read_failures()
-> Result<()> {
    for missing_child in [false, true] {
        let harness = Harness::new(None, false);
        for thread in ["thread", "child", "grandchild", "sibling"] {
            harness.create(thread).await?;
        }
        for (thread, tasks) in [
            (
                "thread",
                vec![
                    (
                        "first",
                        if missing_child { "missing" } else { "child" },
                        "app_owned",
                    ),
                    ("duplicate", "child", "app_owned"),
                    ("native", "native-thread", "provider_native"),
                    ("last", "sibling", "app_owned"),
                ],
            ),
            ("child", vec![("grandchild", "grandchild", "app_owned")]),
        ] {
            for (index, (id, child, origin)) in tasks.into_iter().enumerate() {
                let mut task = sample("OrchestrationV2Subagent");
                task["id"] = json!(id);
                task["threadId"] = json!(thread);
                task["childThreadId"] = json!(child);
                task["origin"] = json!(origin);
                task["startedAt"] = json!(format!("2026-08-27T00:00:0{index}.000Z"));
                harness.sink.commit(CommitRequest {
                    receipt: None,
                    decision: Decision { events: vec![serde_json::from_value(json!({"id":format!("{thread}:{id}"),"threadId":thread,"type":"subagent.updated","occurredAt":TEST_TIME,"payload":task}))?], ..Default::default() },
                    guard: CommitGuard::None,
                }).await?;
            }
        }
        let (service, recorder, _) = service(&harness, settings(json!({})), false);
        let result = service
            .stop_delegated_tasks(
                ThreadId("thread".into()),
                CommandId("walk".into()),
                Some("Parent stopped".into()),
                None,
            )
            .await;
        assert_eq!(result.is_err(), missing_child);
        let commands = recorder.commands.lock().unwrap();
        assert_eq!(
            commands
                .iter()
                .map(|command| command["threadId"].as_str().unwrap())
                .collect::<Vec<_>>(),
            if missing_child {
                vec!["missing", "child", "grandchild", "sibling"]
            } else {
                vec!["child", "grandchild", "sibling"]
            }
        );
        for command in commands.iter() {
            assert_eq!(command["type"], "thread.stop");
            assert_eq!(command["stopDelegatedTasks"], false);
            assert_eq!(command["reason"], "Parent stopped");
            assert_eq!(
                command["commandId"],
                format!("walk:stop:{}", command["threadId"].as_str().unwrap())
            );
        }
    }
    Ok(())
}

#[tokio::test]
async fn delegated_stop_ends_remote_work_without_overwriting_results_and_contains_sibling_failures()
-> Result<()> {
    struct Remote {
        recorder: Recorder,
        fail: bool,
    }
    #[async_trait]
    impl CommandDispatcher for Remote {
        async fn dispatch(&self, command: OrchestrationV2ServerCommand) -> Result<CommandReceipt> {
            let receipt = self.recorder.dispatch(command).await?;
            if self.fail && receipt.command_type == "delegated_task.remote.complete" {
                return Err(Error::Unsupported("remote completion failed"));
            }
            Ok(receipt)
        }
    }
    for (reason, fail) in [(None, false), (Some("Parent stopped"), false), (None, true)] {
        let harness = Harness::new(None, false);
        harness.create("thread").await?;
        harness.create("child").await?;
        let mut child = sample("OrchestrationV2Subagent");
        child["id"] = json!("local-child");
        child["threadId"] = json!("thread");
        child["childThreadId"] = json!("child");
        child["origin"] = json!("app_owned");
        child["startedAt"] = json!("2026-08-26T00:00:00.000Z");
        emit(&harness, "local-child", "subagent.updated", child).await?;
        for (index, result) in [Value::Null, json!("Original result")]
            .into_iter()
            .enumerate()
        {
            let mut task = sample("OrchestrationV2Subagent");
            task["id"] = json!(format!("remote:{index}"));
            task["threadId"] = json!("thread");
            task["childThreadId"] = Value::Null;
            task["origin"] = json!("app_owned");
            task["remoteChild"] =
                json!({"environmentId":"environment","threadId":"remote-thread","label":"Remote"});
            task["result"] = result;
            task["startedAt"] = json!(format!("2026-08-27T00:00:0{index}.000Z"));
            emit(
                &harness,
                &format!("remote:{index}"),
                "subagent.updated",
                task,
            )
            .await?;
        }
        let mut nested = sample("OrchestrationV2Subagent");
        nested["id"] = json!("nested-remote");
        nested["threadId"] = json!("child");
        nested["childThreadId"] = Value::Null;
        nested["origin"] = json!("app_owned");
        nested["remoteChild"] = json!({"environmentId":"environment","threadId":"nested-thread","label":"Nested remote"});
        nested["result"] = json!("Original nested result");
        harness.sink.commit(CommitRequest {
            receipt: None,
            decision: Decision { events: vec![serde_json::from_value(json!({"id":"nested-remote","threadId":"child","type":"subagent.updated","occurredAt":TEST_TIME,"payload":nested}))?], ..Default::default() },
            guard: CommitGuard::None,
        }).await?;
        let (service, local, _) = service(&harness, settings(json!({})), false);
        let remote = Arc::new(Remote {
            recorder: Recorder {
                commands: Mutex::new(Vec::new()),
                downstream: None,
            },
            fail,
        });
        let outcome = service
            .stop_delegated_tasks(
                ThreadId("thread".into()),
                CommandId("stop".into()),
                reason.map(str::to_owned),
                Some(remote.clone()),
            )
            .await;
        assert_eq!(outcome.is_err(), fail);
        let local = local.commands.lock().unwrap();
        assert_eq!(local.len(), 2);
        assert_eq!(local[0]["type"], "thread.stop");
        assert_eq!(local[0]["threadId"], "child");
        assert_eq!(local[1]["type"], "delegated_task.remote.stop");
        assert_eq!(local[1]["parentThreadId"], "child");
        let commands = remote.recorder.commands.lock().unwrap();
        assert_eq!(commands.len(), 2);
        assert_eq!(commands[0]["type"], "delegated_task.remote.complete");
        assert_eq!(commands[0]["status"], "interrupted");
        assert_eq!(
            commands[0]["result"],
            reason.map_or_else(
                || "Stopped with its parent.".into(),
                |reason| format!("Stopped with its parent: {reason}")
            )
        );
        assert_eq!(commands[1]["type"], "delegated_task.remote.stop");
        assert!(commands[1].get("result").is_none());
        for (index, command) in commands.iter().enumerate() {
            assert_eq!(command["commandId"], format!("stop:stop:remote:{index}"));
            assert_eq!(command["parentThreadId"], "thread");
        }
    }
    Ok(())
}

#[tokio::test]
async fn metadata_continuation_checks_current_provider_and_remains_one_atomic_command() -> Result<()>
{
    for (continuation, disabled, replay, accepted) in [
        (true, true, false, false),
        (false, true, false, true),
        (true, false, false, true),
        (true, true, true, true),
    ] {
        let harness = Harness::new(None, false);
        harness.create("thread").await?;
        let (service, commands, _) = service(
            &harness,
            settings(
                json!({"projectSettingsOverrides":{"project":{"providerInstances":{"codex":{"enabled":!disabled}}}}}),
            ),
            false,
        );
        if replay {
            harness
                .database
                .stores
                .transaction(|db| {
                    receipts::upsert(
                        db,
                        &CommandReceipt {
                            command_id: CommandId("handoff".into()),
                            aggregate: AggregateId::Thread(ThreadId("thread".into())),
                            command_type: "thread.metadata.update".into(),
                            accepted_at: TEST_TIME.into(),
                            result_sequence: 1,
                            status: ReceiptStatus::Accepted,
                            error: None,
                        },
                    )
                })
                .await?;
        }
        let mut command = json!({"type":"thread.metadata.update","commandId":"handoff","threadId":"thread","branch":"new-branch","worktreePath":"C:/new-worktree","expectedWorktreePath":null});
        if continuation {
            command["continuation"] =
                json!({"messageId":"continuation","text":"Continue in the new workspace."});
        }
        let outcome = service
            .dispatch(serde_json::from_value(command.clone())?)
            .await;
        if accepted {
            outcome?;
            assert_eq!(*commands.commands.lock().unwrap(), vec![command]);
        } else {
            assert!(matches!(
                outcome,
                Err(Error::Service {
                    operation: "ThreadManagementProviderInstanceDisabledError",
                    ..
                })
            ));
            assert!(commands.commands.lock().unwrap().is_empty());
        }
        service.shutdown().await;
    }
    Ok(())
}

#[tokio::test]
async fn project_listing_uses_lightweight_metadata_and_preserves_lineage_scope_and_locale_order()
-> Result<()> {
    let harness = Harness::new(None, false);
    for (id, project, relation, side, archived, settled) in [
        ("a", "project", Value::Null, false, false, true),
        ("Z", "project", Value::Null, false, false, false),
        ("child", "project", json!("subagent"), false, false, false),
        ("side", "project", json!("subagent"), true, false, false),
        ("archived", "project", Value::Null, false, true, false),
        ("other", "other", Value::Null, false, false, false),
    ] {
        harness.create(id).await?;
        let mut thread = json!(harness.reader.thread(ThreadId(id.into())).await?.unwrap());
        thread["projectId"] = json!(project);
        thread["lineage"]["relationshipToParent"] = relation;
        thread["lineage"]["sideChat"] = json!(side);
        if archived {
            thread["archivedAt"] = json!(TEST_TIME);
        }
        if settled {
            thread["settledOverride"] = json!("settled");
            thread["settledAt"] = json!(TEST_TIME);
        }
        harness.sink.commit(CommitRequest{receipt:None,decision:Decision{events:vec![serde_json::from_value(json!({"id":format!("meta-{id}"),"threadId":id,"type":"thread.metadata-updated","occurredAt":TEST_TIME,"payload":thread}))?],..Default::default()},guard:CommitGuard::None}).await?;
    }
    harness.database.stores.transaction(|db|{
        db.execute("UPDATE orchestration_v2_projection_threads SET payload_json=json_set(payload_json,'$.modelSelection',37) WHERE archived_at IS NULL",[])?;
        db.execute("DROP TABLE orchestration_v2_projection_turn_item_counts",[])?;Ok(())
    }).await?;
    let (service, _, _) = service(&harness, settings(json!({})), false);
    let ids = |rows: Value| {
        rows.as_array()
            .unwrap()
            .iter()
            .map(|row| row["id"].as_str().unwrap().to_owned())
            .collect::<Vec<_>>()
    };
    assert_eq!(
        ids(json!(
            service
                .list_project_threads(Some(ProjectId("project".into())), false)
                .await?
        )),
        ["Z", "a"]
    );
    assert_eq!(
        ids(json!(
            service
                .list_project_threads(Some(ProjectId("project".into())), true)
                .await?
        )),
        ["Z", "child", "a"]
    );
    assert_eq!(
        ids(json!(service.list_project_threads(None, false).await?)),
        ["Z", "other", "a"]
    );
    service.shutdown().await;
    Ok(())
}
