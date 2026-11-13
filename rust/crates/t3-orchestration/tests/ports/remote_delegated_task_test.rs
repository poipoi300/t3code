use super::{
    provider_event_ingestor_test::{Harness, fixture},
    run_execution_service_test::root_state,
};
use async_trait::async_trait;
use serde_json::{Value, json};
use std::sync::Arc;
use t3_orchestration::{
    api::*,
    decision::engine::PureDecisionEngine,
    error::Result,
    ports::sessions_recovery::{
        provider_adapter_registry::{AdapterRegistration, AdapterRegistry},
        provider_continuation_service::ContinuationRequest,
    },
    runtime::dispatch::{CommandInputs, DispatchServices, Dispatcher, ProjectionCommandInputs},
    testing::SequenceIds,
};
struct Recovered;
#[async_trait]
impl RecoveryService for Recovered {
    async fn recover(&self) -> Result<RecoverySummary> {
        Ok(RecoverySummary::default())
    }
    async fn reconcile(&self, _thread: Option<ThreadId>) -> Result<RecoverySummary> {
        Ok(RecoverySummary::default())
    }
    async fn prepare_for_shutdown(&self) -> Result<()> {
        Ok(())
    }
}
pub async fn live_dispatcher() -> Result<(
    Harness,
    Dispatcher,
    tokio::sync::mpsc::Receiver<ContinuationRequest>,
)> {
    live_dispatcher_with_inputs(|input| Arc::new(input)).await
}
pub async fn live_dispatcher_with_inputs(
    wrap: fn(ProjectionCommandInputs) -> Arc<dyn CommandInputs>,
) -> Result<(
    Harness,
    Dispatcher,
    tokio::sync::mpsc::Receiver<ContinuationRequest>,
)> {
    let h = Harness::new().await?;
    let state = root_state();
    for (kind, value) in [
        (
            "provider-thread.updated",
            t3_contracts::stored::to_value(&state.provider_thread)?,
        ),
        ("run.created", t3_contracts::stored::to_value(&state.run)?),
        (
            "run-attempt.created",
            t3_contracts::stored::to_value(&state.attempt)?,
        ),
        (
            "node.updated",
            t3_contracts::stored::to_value(&state.root_node)?,
        ),
    ] {
        h.write(kind, value).await?;
    }
    let mut turn = fixture("OrchestrationV2ProviderTurn");
    turn["id"] = "turn:test".into();
    turn["providerThreadId"] = "pt:test".into();
    turn["nodeId"] = "node:test".into();
    turn["runAttemptId"] = "attempt:test".into();
    turn["status"] = "running".into();
    h.write("provider-turn.updated", turn).await?;
    let (adapter, _calls) = h.fake();
    let registry = Arc::new(AdapterRegistry::new(vec![AdapterRegistration {
        adapter: Arc::new(adapter),
        continuation_key: "codex:default".into(),
        enabled: true,
        credential_access: vec![],
    }]));
    let dispatcher = Dispatcher::new(
        DispatchServices {
            database: h.database.stores.clone(),
            inputs: wrap(ProjectionCommandInputs {
                reader: h.reader.clone(),
                providers: registry,
                clock: h.clock.clone(),
                ids: Arc::new(SequenceIds::new("remote")),
            }),
            engine: Arc::new(PureDecisionEngine),
            committer: h.sink.clone(),
            recovery: Arc::new(Recovered),
            clock: h.clock.clone(),
        },
        None,
    );
    let (sender, receiver) = tokio::sync::mpsc::channel(16);
    dispatcher.set_continuation_requests(sender)?;
    Ok((h, dispatcher, receiver))
}
fn request() -> Value {
    json!({"type":"delegated_task.remote.request","commandId":"delegate","parentThreadId":"thread:test","parentRunId":"run:test","parentNodeId":"node:test","task":"Run the suite there.","driver":"remote","modelSelection":{"instanceId":"codex","model":"m"},"remoteChild":{"environmentId":"environment:box","threadId":"thread:on-the-box","label":"Box"},"completionWake":"always"})
}
async fn dispatch(dispatcher: &Dispatcher, command: Value) -> Result<()> {
    dispatcher
        .dispatch(serde_json::from_value(command)?)
        .await?;
    Ok(())
}
async fn get_task(h: &Harness) -> Result<Value> {
    let projection = h
        .reader
        .projection(ThreadId("thread:test".into()))
        .await?
        .unwrap();
    assert_eq!(projection.subagents.len(), 1);
    Ok(t3_contracts::stored::to_value(&projection.subagents[0])?)
}
async fn open_tasks(h: &Harness) -> Result<Vec<Value>> {
    h.reader
        .open_remote_delegated_tasks()
        .await?
        .iter()
        .map(|task| Ok(t3_contracts::stored::to_value(task)?))
        .collect()
}
#[tokio::test]
async fn remote_tasks_are_recorded_once_without_a_local_child_and_can_be_archived() -> Result<()> {
    let (h, dispatcher, _offers) = live_dispatcher().await?;
    dispatch(&dispatcher, request()).await?;
    dispatch(&dispatcher, request()).await?;
    let task = get_task(&h).await?;
    assert!(task["childThreadId"].is_null());
    assert_eq!(task["status"], "running");
    assert_eq!(task["remoteChild"], request()["remoteChild"]);
    assert!(
        h.reader
            .shell(ThreadId("thread:on-the-box".into()))
            .await?
            .is_none()
    );
    let projection = h
        .reader
        .projection(ThreadId("thread:test".into()))
        .await?
        .unwrap();
    let item = projection
        .turn_items
        .iter()
        .map(t3_contracts::stored::to_value)
        .collect::<std::result::Result<Vec<_>, _>>()?
        .into_iter()
        .find(|item| item["type"] == "subagent")
        .unwrap();
    assert_eq!(item["remoteChild"], task["remoteChild"]);
    let mut other = fixture("OrchestrationV2AppThread");
    other["id"] = "thread:other".into();
    other["threadId"] = "thread:other".into();
    other["projectId"] = "project:test".into();
    other["deletedAt"] = Value::Null;
    other["archivedAt"] = Value::Null;
    h.write("thread.created", other).await?;
    let index = h
        .database
        .stores
        .read(t3_orchestration::projections::read::active_shell_index)
        .await?;
    assert!(
        index
            .iter()
            .find(|entry| entry.id.0 == "thread:test")
            .unwrap()
            .has_remote_tasks
    );
    assert!(
        !index
            .iter()
            .find(|entry| entry.id.0 == "thread:other")
            .unwrap()
            .has_remote_tasks
    );
    dispatch(
        &dispatcher,
        json!({"type":"thread.archive","commandId":"archive","threadId":"thread:test"}),
    )
    .await?;
    assert!(
        h.reader
            .shell(ThreadId("thread:test".into()))
            .await?
            .unwrap()
            .archived_at
            .is_some()
    );
    Ok(())
}
#[tokio::test]
async fn stale_stop_acknowledgements_cannot_clear_a_newer_remote_stop_or_overwrite_its_result()
-> Result<()> {
    let (h, dispatcher, _offers) = live_dispatcher().await?;
    dispatch(&dispatcher, request()).await?;
    let id = get_task(&h).await?["id"].clone();
    let complete = |command: &str| json!({"type":"delegated_task.remote.complete","commandId":command,"parentThreadId":"thread:test","taskId":id,"status":"completed","result":"Original result.","stoppedThere":"ran-out"});
    dispatch(&dispatcher, complete("result")).await?;
    for count in [1, 2] {
        dispatch(&dispatcher,json!({"type":"delegated_task.remote.stop","commandId":format!("stop:{count}"),"parentThreadId":"thread:test","taskId":id})).await?;
    }
    for count in [None, Some(1)] {
        let mut command = complete(&format!("ack:{count:?}"));
        command["stoppedThere"] = "stopped".into();
        if let Some(count) = count {
            command["stopCount"] = count.into();
        }
        assert!(dispatch(&dispatcher, command).await.is_err());
        assert_eq!(get_task(&h).await?["remoteChild"]["stoppedThere"], false);
        assert_eq!(
            open_tasks(&h).await?,
            vec![json!({"parentThreadId":"thread:test","taskId":id})]
        );
    }
    let mut command = complete("ack:2");
    command["stoppedThere"] = "stopped".into();
    command["stopCount"] = 2.into();
    dispatch(&dispatcher, command).await?;
    let task = get_task(&h).await?;
    assert_eq!(task["result"], "Original result.");
    assert_eq!(task["status"], "completed");
    assert_eq!(task["remoteChild"]["stoppedThere"], true);
    assert!(open_tasks(&h).await?.is_empty());
    Ok(())
}
#[tokio::test]
async fn remote_completion_claims_delivery_once_and_offers_the_live_parent_its_result() -> Result<()>
{
    let (h, dispatcher, mut offers) = live_dispatcher().await?;
    dispatch(&dispatcher, request()).await?;
    let id = get_task(&h).await?["id"].clone();
    assert_eq!(
        open_tasks(&h).await?,
        vec![json!({"parentThreadId":"thread:test","taskId":id})]
    );
    let complete = |command: &str, result: &str| json!({"type":"delegated_task.remote.complete","commandId":command,"parentThreadId":"thread:test","taskId":id,"status":"completed","result":result,"stoppedThere":"ran-out"});
    dispatch(
        &dispatcher,
        complete("result", "All 412 tests pass on the box."),
    )
    .await?;
    assert!(
        dispatch(&dispatcher, complete("late", "A late different answer."))
            .await
            .is_err()
    );
    let task = get_task(&h).await?;
    assert_eq!(task["result"], "All 412 tests pass on the box.");
    assert_eq!(task["completionDelivery"]["state"], "claimed");
    assert!(open_tasks(&h).await?.is_empty());
    let projection = h
        .reader
        .projection(ThreadId("thread:test".into()))
        .await?
        .unwrap();
    let projected = t3_contracts::stored::to_value(&projection)?;
    assert!(
        projected["turnItems"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["type"] == "subagent"
                && item["subagentId"] == id
                && item["status"] == "completed")
    );
    assert_eq!(
        projected["runs"]
            .as_array()
            .unwrap()
            .iter()
            .find(|run| run["id"] == "run:test")
            .unwrap()["delegatedCompletion"]["delivery"]["taskIds"],
        json!([id])
    );
    let offered = offers.recv().await.unwrap();
    assert_eq!(offered.thread_id.0, "thread:test");
    assert!(offers.try_recv().is_err());
    dispatch(&dispatcher,json!({"type":"delegated_task.remote.complete","commandId":"stop-after-result","parentThreadId":"thread:test","taskId":id,"status":"interrupted","result":"Stopped with its parent."})).await?;
    let task = get_task(&h).await?;
    assert_eq!(task["status"], "completed");
    assert_eq!(task["result"], "All 412 tests pass on the box.");
    assert_eq!(task["remoteChild"]["stoppedThere"], false);
    assert_eq!(task["remoteChild"]["stopCount"], 1);
    assert_eq!(
        open_tasks(&h).await?,
        vec![json!({"parentThreadId":"thread:test","taskId":id})]
    );
    Ok(())
}
