use super::provider_event_ingestor_test::{decode, fixture};
use serde_json::json;
use t3_orchestration::{api::*, ports::runs_ingestion::notification_mailbox::*};

#[test]
fn mailbox_steers_detect_ended_runs_and_latest_continuation_but_exclude_initial_input() {
    let mut message = fixture("OrchestrationV2ConversationMessage");
    message["id"] = "steer".into();
    message["runId"] = "run:test".into();
    message["delegatedCompletion"] = json!({"parentRunId":"parent","generation":1,"taskIds":[]});
    let mut run = fixture("OrchestrationV2Run");
    run["id"] = "run:test".into();
    run["userMessageId"] = "initial".into();
    run["activeAttemptId"] = "attempt:test".into();
    let mut turn = fixture("OrchestrationV2ProviderTurn");
    turn["runAttemptId"] = "attempt:test".into();
    turn["status"] = "completed".into();
    turn["ordinal"] = 1.into();
    let message_id = MessageId("steer".into());
    for status in [
        "completed",
        "failed",
        "interrupted",
        "cancelled",
        "rolled_back",
    ] {
        run["status"] = status.into();
        assert!(is_undelivered_mailbox_steer(
            &[decode(message.clone())],
            &[decode(run.clone())],
            &[],
            &message_id
        ));
    }
    run["status"] = "running".into();
    assert!(!is_undelivered_mailbox_steer(
        &[decode(message.clone())],
        &[decode(run.clone())],
        &[],
        &message_id
    ));
    assert!(is_undelivered_mailbox_steer(
        &[decode(message.clone())],
        &[decode(run.clone())],
        &[decode(turn.clone())],
        &message_id
    ));
    let mut continuation = turn.clone();
    continuation["ordinal"] = 2.into();
    continuation["status"] = "running".into();
    assert!(!is_undelivered_mailbox_steer(
        &[decode(message.clone())],
        &[decode(run.clone())],
        &[decode(turn.clone()), decode(continuation)],
        &message_id
    ));
    run["userMessageId"] = "steer".into();
    assert!(!is_undelivered_mailbox_steer(
        &[decode(message.clone())],
        &[decode(run)],
        &[decode(turn)],
        &message_id
    ));
    message
        .as_object_mut()
        .unwrap()
        .shift_remove("delegatedCompletion");
    assert!(!is_undelivered_mailbox_steer(
        &[decode(message)],
        &[],
        &[],
        &message_id
    ));
}

#[test]
fn buffering_requires_provider_authored_notification_and_excludes_delegated_completions() {
    let mut message = fixture("OrchestrationV2ConversationMessage");
    message["createdBy"] = "agent".into();
    message["creationSource"] = "provider".into();
    message["notification"] = json!({"source":{"kind":"delegated_task","taskIds":[]},"outcome":"completed","summary":"Finished"});
    message
        .as_object_mut()
        .unwrap()
        .shift_remove("delegatedCompletion");
    let decoded: OrchestrationV2ConversationMessage = decode(message.clone());
    assert!(is_buffered_provider_notification(&decoded));
    for (key, value) in [
        ("createdBy", json!("user")),
        ("creationSource", json!("mcp")),
        (
            "delegatedCompletion",
            json!({"parentRunId":"parent","generation":1,"taskIds":[]}),
        ),
    ] {
        let mut candidate = message.clone();
        candidate[key] = value;
        assert!(!is_buffered_provider_notification(&decode(candidate)));
    }
    message
        .as_object_mut()
        .unwrap()
        .shift_remove("notification");
    assert!(!is_buffered_provider_notification(&decode(message)));
}

struct QuestionDispatch(std::sync::Mutex<Vec<serde_json::Value>>);
#[async_trait::async_trait]
impl CommandDispatcher for QuestionDispatch {
    async fn dispatch(
        &self,
        command: OrchestrationV2ServerCommand,
    ) -> t3_orchestration::error::Result<t3_orchestration::stores::receipts::CommandReceipt> {
        use t3_orchestration::stores::receipts::{AggregateId, CommandReceipt, ReceiptStatus};
        let command = t3_contracts::stored::to_value(&command)?;
        self.0.lock().unwrap().push(command.clone());
        Ok(CommandReceipt {
            command_id: serde_json::from_value(command["commandId"].clone())?,
            aggregate: AggregateId::Thread(ThreadId("thread:test".into())),
            command_type: command["type"].as_str().unwrap().into(),
            accepted_at: t3_orchestration::testing::TEST_TIME.into(),
            result_sequence: 0,
            status: ReceiptStatus::Accepted,
            error: None,
        })
    }
}
async fn child_question(
    capability: &str,
) -> t3_orchestration::error::Result<(
    super::provider_event_ingestor_test::Harness,
    serde_json::Value,
    serde_json::Value,
    OrchestrationV2DomainEvent,
)> {
    let h = super::provider_event_ingestor_test::Harness::new().await?;
    let state = super::run_execution_service_test::root_state();
    let parent = t3_contracts::stored::to_value(&state.run)?;
    h.write("run.created", parent.clone()).await?;
    let mut child = fixture("OrchestrationV2AppThread");
    child["id"] = "child".into();
    child["lineage"] = json!({"rootThreadId":"thread:test","parentThreadId":"thread:test","relationshipToParent":"subagent"});
    child["forkedFrom"] = json!({"type":"node","nodeId":"task"});
    h.write("thread.created", child).await?;
    let mut task = fixture("OrchestrationV2Subagent");
    task["id"] = "task".into();
    task["threadId"] = "thread:test".into();
    task["runId"] = "run:test".into();
    task["origin"] = "app_owned".into();
    task["childThreadId"] = "child".into();
    task["status"] = "running".into();
    task["completionWake"] = "settled_only".into();
    task.as_object_mut().unwrap().remove("completionDelivery");
    h.write("subagent.updated", task).await?;
    let mut run = parent.clone();
    run["threadId"] = "child".into();
    run["id"] = "child-run".into();
    run["delegatedTaskId"] = "task".into();
    h.write("run.created", run).await?;
    let mut request = fixture("OrchestrationV2RuntimeRequest");
    request["id"] = "request:q".into();
    request["threadId"] = "child".into();
    request["kind"] = "user_input".into();
    request["status"] = "pending".into();
    request["responseCapability"] = json!({"type":capability,"providerSessionId":"session:child"});
    let event = h
        .write("runtime-request.updated", request.clone())
        .await?
        .remove(0)
        .event;
    Ok((h, parent, request, event))
}
#[tokio::test]
async fn a_blocking_child_question_waits_for_parent_settlement_while_message_questions_wake_immediately()
-> t3_orchestration::error::Result<()> {
    use std::sync::{Arc, Mutex};
    use t3_orchestration::ports::runs_ingestion::delegated_task_question_reactor::DelegatedTaskQuestionReactor;
    for capability in ["live", "message"] {
        let (h, _, _, event) = child_question(capability).await?;
        let dispatcher = Arc::new(QuestionDispatch(Mutex::new(vec![])));
        let reactor = DelegatedTaskQuestionReactor {
            projections: h.reader.clone(),
            dispatcher: dispatcher.clone(),
        };
        reactor.handle(&event).await?;
        let commands = dispatcher.0.lock().unwrap();
        assert_eq!(commands.len(), usize::from(capability == "message"));
        if capability == "message" {
            assert_eq!(
                commands[0]["messageId"],
                "message:delegated-task-question:thread:test:request:q"
            );
            assert!(
                commands[0]["text"]
                    .as_str()
                    .unwrap()
                    .contains("is still working")
            );
        }
    }
    Ok(())
}
#[tokio::test]
async fn parent_settlement_catches_up_the_child_question_and_resolution_cancels_its_queued_wake()
-> t3_orchestration::error::Result<()> {
    use std::sync::{Arc, Mutex};
    use t3_orchestration::ports::runs_ingestion::delegated_task_question_reactor::DelegatedTaskQuestionReactor;
    let (h, mut parent, mut request, _) = child_question("live").await?;
    parent["status"] = "waiting".into();
    let event = h.write("run.updated", parent).await?.remove(0).event;
    let dispatcher = Arc::new(QuestionDispatch(Mutex::new(vec![])));
    let reactor = DelegatedTaskQuestionReactor {
        projections: h.reader.clone(),
        dispatcher: dispatcher.clone(),
    };
    reactor.handle(&event).await?;
    assert_eq!(dispatcher.0.lock().unwrap().len(), 1);
    let mut wake = fixture("OrchestrationV2Run");
    wake["id"] = "wake".into();
    wake["threadId"] = "thread:test".into();
    wake["status"] = "queued".into();
    wake["userMessageId"] = "message:delegated-task-question:thread:test:request:q".into();
    h.write("run.created", wake).await?;
    request["status"] = "cancelled".into();
    let event = h
        .write("runtime-request.updated", request)
        .await?
        .remove(0)
        .event;
    reactor.handle(&event).await?;
    let commands = dispatcher.0.lock().unwrap();
    assert_eq!(commands.len(), 2);
    assert_eq!(commands[1]["type"], "queued-run.cancel");
    assert_eq!(commands[1]["runId"], "wake");
    Ok(())
}
