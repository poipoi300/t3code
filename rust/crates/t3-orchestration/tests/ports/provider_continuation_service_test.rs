use super::provider_runtime_recovery_service_test::{projection, seed, sink};
use async_trait::async_trait;
use serde_json::{Value, json};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use t3_contracts::{base_schemas::*, orchestration_v2::*, provider_instance::ProviderDriverKind};
use t3_orchestration::{
    api::*,
    error::{Error, Result},
    ports::sessions_recovery::provider_continuation_service::*,
    projections::store::ProjectionStore,
    stores::receipts::*,
    testing::*,
};
use tokio::sync::mpsc;

struct Dispatcher {
    commands: mpsc::UnboundedSender<Value>,
    fail: AtomicUsize,
    disabled: bool,
}
#[async_trait]
impl CommandDispatcher for Dispatcher {
    async fn dispatch(&self, command: OrchestrationV2ServerCommand) -> Result<CommandReceipt> {
        let command = serde_json::to_value(command)?;
        self.commands
            .send(command.clone())
            .expect("command receiver");
        if self
            .fail
            .try_update(Ordering::SeqCst, Ordering::SeqCst, |count| {
                count.checked_sub(1)
            })
            .is_ok()
        {
            return Err(Error::Service {
                operation: if self.disabled {
                    "ThreadManagementProviderInstanceDisabledError"
                } else {
                    "test failure"
                },
                detail: "disk unavailable".into(),
            });
        }
        Ok(CommandReceipt {
            command_id: serde_json::from_value(command["commandId"].clone())?,
            aggregate: AggregateId::Thread(ThreadId("thread:test".into())),
            command_type: command["type"].as_str().expect("type").into(),
            accepted_at: TEST_TIME.into(),
            result_sequence: 0,
            status: ReceiptStatus::Accepted,
            error: None,
        })
    }
}

struct Callbacks {
    cleared: AtomicUsize,
    invalidated: bool,
}
#[async_trait]
impl ContinuationCallbacks for Callbacks {
    async fn dispatch_if_current(
        &self,
        dispatcher: &dyn CommandDispatcher,
        command: OrchestrationV2ServerCommand,
    ) -> Result<Option<CommandReceipt>> {
        if self.invalidated {
            Ok(None)
        } else {
            dispatcher.dispatch(command).await.map(Some)
        }
    }
    async fn clear_if_current(&self) -> Result<()> {
        self.cleared.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

fn request(delegated: bool) -> ContinuationRequest {
    ContinuationRequest {
        thread_id: ThreadId("thread:test".into()),
        provider_thread_id: ProviderThreadId("provider-thread:test".into()),
        driver: ProviderDriverKind("codex".into()),
        detail: None,
        notification: None,
        delegated_completion: delegated.then_some(DelegatedContinuation {
            parent_run_id: RunId("run:test".into()),
            generation: 1,
            message_id: MessageId("completion".into()),
        }),
        delivery: ContinuationDelivery::AdapterBuffered,
        message_id: Some(MessageId("wake".into())),
        callbacks: None,
    }
}

fn delegated_projection() -> Result<OrchestrationV2ThreadProjection> {
    let mut projection = projection()?;
    projection.runs[0].delegated_completion = Some(Some(serde_json::from_value(
        json!({"disposition":"open","nextGeneration":2,"delivery":{"generation":1,"messageId":"completion","taskIds":["first","second"]}}),
    )?));
    Ok(projection)
}

async fn setup(
    projection: &OrchestrationV2ThreadProjection,
    failures: usize,
    disabled: bool,
) -> Result<(
    TestDatabase,
    ProviderContinuationService,
    mpsc::UnboundedReceiver<Value>,
)> {
    let database = TestDatabase::memory()?;
    let committer = Arc::new(sink(database.stores.clone()));
    seed(&committer, projection).await?;
    let (commands, received) = mpsc::unbounded_channel();
    let service = ProviderContinuationService::new(
        Arc::new(ProjectionStore::new(database.stores.clone())),
        Arc::new(Dispatcher {
            commands,
            fail: AtomicUsize::new(failures),
            disabled,
        }),
        committer,
        Arc::new(SequenceIds::new("continuation")),
    );
    Ok((database, service, received))
}

#[tokio::test]
async fn buffered_and_text_wakes_preserve_identity_notification_and_creation_source() -> Result<()>
{
    for delivery in [
        ContinuationDelivery::AdapterBuffered,
        ContinuationDelivery::MessageText,
    ] {
        let (_database, service, mut commands) = setup(&projection()?, 0, false).await?;
        let mut request = request(false);
        request.delivery = delivery;
        request.detail = Some("Monitor completed".into());
        request.notification = Some(serde_json::from_value(
            json!({"source":{"kind":"monitor"},"outcome":"updated","summary":"Monitor updated"}),
        )?);
        assert_eq!(
            service.process(&request).await?,
            ContinuationOutcome::Settled
        );
        let command = commands.recv().await.expect("command");
        assert_eq!(command["commandId"], "provider-continuation:wake");
        assert_eq!(command["messageId"], "wake");
        assert_eq!(
            command["creationSource"],
            if matches!(delivery, ContinuationDelivery::MessageText) {
                "server"
            } else {
                "provider"
            }
        );
        assert_eq!(command["text"], "Monitor completed");
        assert_eq!(command["notification"]["source"]["kind"], "monitor");
        assert_eq!(command["dispatchMode"]["type"], "queue_after_active");
        assert_eq!(command["createdBy"], "agent");
        assert!(commands.try_recv().is_err());
        service.shutdown().await;
    }
    Ok(())
}

#[tokio::test]
async fn current_delegated_completion_dispatches_one_server_owned_queued_message() -> Result<()> {
    let (_database, service, mut commands) = setup(&delegated_projection()?, 0, false).await?;
    assert_eq!(
        service.process(&request(true)).await?,
        ContinuationOutcome::Settled
    );
    let command = commands.recv().await.expect("command");
    assert_eq!(command["messageId"], "completion");
    assert_eq!(command["creationSource"], "server");
    assert_eq!(
        command["delegatedCompletion"],
        json!({"parentRunId":"run:test","generation":1,"taskIds":["first","second"]})
    );
    assert_eq!(
        command["text"],
        "Delegated tasks first, second reached terminal states. Use task_status with each taskId to read the results."
    );
    assert!(commands.try_recv().is_err());
    service.shutdown().await;
    Ok(())
}

#[tokio::test]
async fn disabled_delegation_disposes_all_tasks_once_and_clears_the_offer() -> Result<()> {
    let (_database, service, mut commands) = setup(&delegated_projection()?, 1, true).await?;
    let callbacks = Arc::new(Callbacks {
        cleared: AtomicUsize::new(0),
        invalidated: false,
    });
    let mut request = request(true);
    request.callbacks = Some(callbacks.clone());
    assert_eq!(
        service.process(&request).await?,
        ContinuationOutcome::Settled
    );
    assert_eq!(
        commands.recv().await.expect("wake")["type"],
        "message.dispatch"
    );
    for task in ["first", "second"] {
        let command = commands.recv().await.expect("dispose");
        assert_eq!(
            command["type"],
            "delegated_task.completion-delivery.dispose"
        );
        assert_eq!(command["taskId"], task);
    }
    assert_eq!(callbacks.cleared.load(Ordering::SeqCst), 1);
    assert!(commands.try_recv().is_err());
    service.shutdown().await;
    Ok(())
}

#[tokio::test]
async fn ordinary_dispatch_failures_reach_the_guard_and_release_its_latch() -> Result<()> {
    for disabled in [false, true] {
        let (_database, service, mut commands) = setup(&projection()?, 1, disabled).await?;
        let callbacks = Arc::new(Callbacks {
            cleared: AtomicUsize::new(0),
            invalidated: false,
        });
        let mut request = request(false);
        request.callbacks = Some(callbacks.clone());
        assert_eq!(service.process(&request).await.is_err(), !disabled);
        assert_eq!(callbacks.cleared.load(Ordering::SeqCst), 1);
        assert_eq!(
            commands.recv().await.expect("wake")["type"],
            "message.dispatch"
        );
        assert!(commands.try_recv().is_err());
        service.shutdown().await;
    }
    Ok(())
}

#[tokio::test]
async fn delegated_failures_back_off_but_recheck_the_current_delivery() -> Result<()> {
    let (database, service, mut commands) = setup(&delegated_projection()?, 10, false).await?;
    let request = request(true);
    for delay in [100, 200, 400, 800, 1600, 3200, 5000, 5000] {
        assert_eq!(
            service.process(&request).await?,
            ContinuationOutcome::RetryAfter(std::time::Duration::from_millis(delay))
        );
        commands.recv().await.expect("failed dispatch");
    }
    database.stores.transaction(|tx| { tx.execute("UPDATE orchestration_v2_projection_runs SET payload_json=json_set(payload_json,'$.delegatedCompletion.disposition','stopped') WHERE run_id='run:test'", [])?; Ok(()) }).await?;
    assert_eq!(
        service.process(&request).await?,
        ContinuationOutcome::Settled
    );
    assert!(commands.try_recv().is_err());
    service.shutdown().await;
    Ok(())
}

#[tokio::test]
async fn invalidated_and_archived_requests_do_not_dispatch() -> Result<()> {
    let callbacks = Arc::new(Callbacks {
        cleared: AtomicUsize::new(0),
        invalidated: true,
    });
    let mut request = request(false);
    request.callbacks = Some(callbacks.clone());
    let (_database, service, mut commands) = setup(&projection()?, 0, false).await?;
    assert_eq!(
        service.process(&request).await?,
        ContinuationOutcome::Settled
    );
    assert!(commands.try_recv().is_err());
    service.shutdown().await;
    let mut archived = projection()?;
    archived.thread.archived_at = Some(t3_contracts::wire::WireDateTime(TEST_TIME.into()));
    let (_database, service, mut commands) = setup(&archived, 0, false).await?;
    assert_eq!(
        service.process(&request).await?,
        ContinuationOutcome::Settled
    );
    assert_eq!(callbacks.cleared.load(Ordering::SeqCst), 1);
    assert!(commands.try_recv().is_err());
    service.shutdown().await;
    Ok(())
}

#[test]
fn delivered_and_stale_completions_are_rejected_but_unaccepted_mailbox_steers_recover() -> Result<()>
{
    let completion = request(true).delegated_completion.expect("completion");
    let mut projection = delegated_projection()?;
    for disposition in ["stopped", "disposed"] {
        let mut run = serde_json::to_value(&projection.runs[0])?;
        run["delegatedCompletion"]["disposition"] = json!(disposition);
        assert!(
            current_delivery(&[], &[serde_json::from_value(run)?], &[], &completion)?.is_none()
        );
    }
    let mut message: Value = serde_json::from_str(
        include_str!("../../../../fixtures/samples/OrchestrationV2ConversationMessage.jsonl")
            .lines()
            .next()
            .expect("fixture"),
    )?;
    message["id"] = json!("completion");
    message["runId"] = json!("run:test");
    message["delegatedCompletion"] = json!({"parentRunId":"run:test","generation":1,"taskIds":[]});
    projection.messages = vec![serde_json::from_value(message)?];
    assert!(current_delivery(&projection.messages, &projection.runs, &[], &completion)?.is_none());
    projection.runs[0].status = OrchestrationV2RunStatus::Interrupted;
    assert!(current_delivery(&projection.messages, &projection.runs, &[], &completion)?.is_some());
    let stale = DelegatedContinuation {
        generation: 2,
        ..completion
    };
    assert!(current_delivery(&projection.messages, &projection.runs, &[], &stale)?.is_none());
    Ok(())
}

struct TerminalDispatcher {
    sink: Arc<dyn EventCommitter>,
    run: Value,
}

#[async_trait]
impl CommandDispatcher for TerminalDispatcher {
    async fn dispatch(&self, command: OrchestrationV2ServerCommand) -> Result<CommandReceipt> {
        let command = serde_json::to_value(command)?;
        let mut queued = self.run.clone();
        queued["status"] = json!("queued");
        queued["completedAt"] = Value::Null;
        let receipt = CommandReceipt {
            command_id: serde_json::from_value(command["commandId"].clone())?,
            aggregate: AggregateId::Thread(ThreadId("thread:test".into())),
            command_type: "message.dispatch".into(),
            accepted_at: TEST_TIME.into(),
            result_sequence: 0,
            status: ReceiptStatus::Accepted,
            error: None,
        };
        let result = self.sink.commit(CommitRequest { receipt: Some(receipt), guard: CommitGuard::None,
            decision: Decision { events: vec![
                serde_json::from_value(json!({"id":"wake:created","threadId":"thread:test","type":"run.created","occurredAt":TEST_TIME,"payload":queued}))?,
                serde_json::from_value(json!({"id":"wake:ended","threadId":"thread:test","type":"run.updated","occurredAt":TEST_TIME,"payload":self.run}))?,
            ], ..Default::default() },
        }).await?;
        Ok(result.receipt.expect("dispatch receipt"))
    }
}

struct WatchCallbacks(mpsc::UnboundedSender<()>);

#[async_trait]
impl ContinuationCallbacks for WatchCallbacks {
    async fn clear_if_current(&self) -> Result<()> {
        Ok(())
    }
    fn watch_unstarted_run(&self) -> bool {
        true
    }
    async fn on_run_ended_without_starting(&self) -> Result<()> {
        self.0.send(()).expect("watch receiver");
        Ok(())
    }
}

#[tokio::test]
async fn already_ended_queued_runs_release_the_wake_latch_only_if_they_never_started() -> Result<()>
{
    for started in [false, true] {
        let database = TestDatabase::memory()?;
        let projection = projection()?;
        let sink = Arc::new(sink(database.stores.clone()));
        seed(&sink, &projection).await?;
        let mut run = serde_json::to_value(&projection.runs[0])?;
        run["id"] = json!("wake:run");
        run["ordinal"] = json!(2);
        run["userMessageId"] = json!("wake");
        run["rootNodeId"] = json!("wake:node");
        run["activeAttemptId"] = Value::Null;
        run["status"] = json!("cancelled");
        run["completedAt"] = json!(TEST_TIME);
        run["startedAt"] = if started {
            json!(TEST_TIME)
        } else {
            Value::Null
        };
        let (send, mut ended) = mpsc::unbounded_channel();
        let service = ProviderContinuationService::new(
            Arc::new(ProjectionStore::new(database.stores)),
            Arc::new(TerminalDispatcher {
                sink: sink.clone(),
                run,
            }),
            sink,
            Arc::new(SequenceIds::new("watch")),
        );
        let mut request = request(false);
        request.callbacks = Some(Arc::new(WatchCallbacks(send)));
        assert_eq!(
            service.process(&request).await?,
            ContinuationOutcome::Settled
        );
        if !started {
            assert_eq!(ended.recv().await, Some(()));
        }
        service.shutdown().await;
        assert!(ended.try_recv().is_err());
    }
    Ok(())
}
