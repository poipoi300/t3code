use super::remote_delegated_task_test::live_dispatcher;
use serde_json::{Value, json};
use t3_orchestration::{api::*, error::Result};

fn message(id: &str, text: &str) -> Value {
    json!({"type":"message.dispatch","commandId":id,"threadId":"thread:test","messageId":format!("message:{id}"),"text":text,"attachments":[],"modelSelection":{"instanceId":"codex","model":"queued-model"},"dispatchMode":{"type":"queue_after_active"},"createdBy":"user","creationSource":"web"})
}

#[tokio::test]
async fn selected_held_message_starts_once_preserves_selection_and_releases_the_remaining_queue()
-> Result<()> {
    let (h, dispatcher, _continuations) = live_dispatcher().await?;
    let workspace = tempfile::tempdir()?;
    let mut thread = t3_contracts::stored::to_value(
        &h.reader
            .thread(ThreadId("thread:test".into()))
            .await?
            .unwrap(),
    )?;
    thread["worktreePath"] = json!(workspace.path());
    h.write("thread.metadata-updated", thread).await?;
    for text in ["queued-first", "selected", "queued-last"] {
        dispatcher
            .dispatch(serde_json::from_value(message(text, text))?)
            .await?;
    }
    let before = h
        .reader
        .projection(ThreadId("thread:test".into()))
        .await?
        .unwrap();
    let selected = before
        .runs
        .iter()
        .find(|run| {
            t3_contracts::stored::to_value(&run.user_message_id).unwrap() == "message:selected"
        })
        .unwrap()
        .clone();
    for run in before.runs {
        let mut value = t3_contracts::stored::to_value(&run)?;
        if run.status == OrchestrationV2RunStatus::Queued {
            value["queueHeld"] = true.into();
        } else {
            value["status"] = "cancelled".into();
            value["completedAt"] = json!(h.clock.now());
        }
        h.write("run.updated", value).await?;
    }
    for turn in before.provider_turns {
        let mut value = t3_contracts::stored::to_value(&turn)?;
        value["status"] = "cancelled".into();
        value["completedAt"] = json!(h.clock.now());
        h.write("provider-turn.updated", value).await?;
    }
    let command = json!({"type":"queued-run.start","commandId":"start-selected","threadId":"thread:test","runId":selected.id});
    dispatcher
        .dispatch(serde_json::from_value(command.clone())?)
        .await?;
    dispatcher
        .dispatch(serde_json::from_value(command.clone())?)
        .await?;
    let after = h
        .reader
        .projection(ThreadId("thread:test".into()))
        .await?
        .unwrap();
    let selected_after = after.runs.iter().find(|run| run.id == selected.id).unwrap();
    assert_eq!(selected_after.status, OrchestrationV2RunStatus::Starting);
    assert_eq!(selected_after.model_selection, selected.model_selection);
    assert_eq!(
        after
            .messages
            .iter()
            .filter(|message| message.id == selected.user_message_id)
            .count(),
        1
    );
    let queued = after
        .runs
        .iter()
        .filter(|run| run.status == OrchestrationV2RunStatus::Queued)
        .collect::<Vec<_>>();
    assert_eq!(
        queued
            .iter()
            .map(|run| t3_contracts::stored::to_value(&run.user_message_id).unwrap())
            .collect::<Vec<_>>(),
        vec![json!("message:queued-first"), json!("message:queued-last")]
    );
    assert!(
        queued
            .iter()
            .all(|run| t3_contracts::stored::to_value(run).unwrap()["queueHeld"] != true)
    );
    let selected_id = t3_contracts::stored::to_value(&selected.id)?
        .as_str()
        .unwrap()
        .to_owned();
    let starts = h.database.stores.read(move|db|Ok(db.query_row("SELECT COUNT(*) FROM orchestration_v2_effect_outbox WHERE json_extract(payload_json,'$.type')='provider-turn.start' AND json_extract(payload_json,'$.runId')=?1",[selected_id],|row|row.get::<_,i64>(0))?)).await?;
    assert_eq!(starts, 1);
    let mut duplicate = command;
    duplicate["commandId"] = "start-selected-again".into();
    assert!(
        dispatcher
            .dispatch(serde_json::from_value(duplicate)?)
            .await
            .is_err()
    );
    Ok(())
}

#[tokio::test]
async fn maintenance_messages_cannot_be_promoted_and_remain_queued() -> Result<()> {
    let (h, dispatcher, _continuations) = live_dispatcher().await?;
    let workspace = tempfile::tempdir()?;
    let mut thread = t3_contracts::stored::to_value(
        &h.reader
            .thread(ThreadId("thread:test".into()))
            .await?
            .unwrap(),
    )?;
    thread["worktreePath"] = json!(workspace.path());
    h.write("thread.metadata-updated", thread).await?;
    for (index, text) in ["/compact", "/logout"].into_iter().enumerate() {
        let id = format!("maintenance:{index}");
        dispatcher
            .dispatch(serde_json::from_value(message(&id, text))?)
            .await?;
        let projection = h
            .reader
            .projection(ThreadId("thread:test".into()))
            .await?
            .unwrap();
        let queued = projection
            .runs
            .iter()
            .find(|run| {
                t3_contracts::stored::to_value(&run.user_message_id).unwrap()
                    == format!("message:{id}")
            })
            .unwrap();
        assert!(dispatcher.dispatch(serde_json::from_value(json!({"type":"queued-message.promote-to-steer","commandId":format!("promote:{index}"),"threadId":"thread:test","queuedRunId":queued.id,"targetRunId":"run:test"}))?).await.is_err());
        assert_eq!(
            h.reader
                .run(
                    ThreadId("thread:test".into()),
                    match &queued.id {
                        t3_contracts::orchestration_v2_id_table::IdOrIndex::Id(id) => id.clone(),
                        _ => panic!("expanded run"),
                    }
                )
                .await?
                .unwrap()
                .status,
            OrchestrationV2RunStatus::Queued
        );
    }
    Ok(())
}

fn scheduled_inputs(
    mut input: t3_orchestration::runtime::dispatch::ProjectionCommandInputs,
) -> std::sync::Arc<dyn t3_orchestration::runtime::dispatch::CommandInputs> {
    use super::provider_event_ingestor_test::{decode, fixture};
    use t3_orchestration::{
        ports::sessions_recovery::provider_adapter_registry::AdapterRegistry,
        provider::ThreadSnapshot, testing::FakeProviderAdapter,
    };
    let mut adapters: Vec<std::sync::Arc<dyn t3_orchestration::provider::ProviderAdapter>> =
        Vec::new();
    let mut active_adapter = None;
    for driver in ["codex", "claudeAgent"] {
        let mut session = fixture("OrchestrationV2ProviderSession");
        session["driver"] = driver.into();
        session["id"] = format!("session:{driver}").into();
        if driver == "codex" {
            session["id"] = "session:test".into();
        }
        session["providerInstanceId"] = driver.into();
        session["capabilities"]["turns"]["supportsActiveSteering"] = true.into();
        let session: OrchestrationV2ProviderSession = decode(session);
        let (adapter, _calls) = FakeProviderAdapter::new(
            t3_contracts::provider_instance::ProviderInstanceId(driver.into()),
            t3_contracts::provider_instance::ProviderDriverKind(driver.into()),
            session.capabilities.clone(),
            session,
            ThreadSnapshot {
                provider_thread: decode(fixture("OrchestrationV2ProviderThread")),
                provider_turns: vec![],
                messages: vec![],
                reasoning_message_ids: None,
                runtime_requests: vec![],
                provider_payload: None,
            },
        );
        let adapter = std::sync::Arc::new(adapter);
        if driver == "codex" {
            active_adapter = Some(adapter.clone());
        }
        adapters.push(adapter);
    }
    input.providers = std::sync::Arc::new(AdapterRegistry::from_adapters(adapters));
    std::sync::Arc::new(t3_orchestration::runtime::dispatch::RuntimeCommandInputs {
        base: input,
        sessions: super::provider_turn_start_service_test::sessions(
            active_adapter.expect("test value"),
        ),
        policy: std::sync::Arc::new(t3_orchestration::composition::BaseRuntimePolicy),
    })
}

async fn check_promoted_scheduled_delivery(pinned: bool) -> Result<()> {
    let (h, dispatcher, _continuations) =
        super::remote_delegated_task_test::live_dispatcher_with_inputs(scheduled_inputs).await?;
    let workspace = tempfile::tempdir()?;
    let mut thread = h
        .reader
        .thread(ThreadId("thread:test".into()))
        .await?
        .expect("test value");
    thread.worktree_path = Some(t3_contracts::base_schemas::TrimmedNonEmptyString(
        workspace.path().to_string_lossy().into_owned(),
    ));
    h.write(
        "thread.metadata-updated",
        t3_contracts::stored::to_value(&thread)?,
    )
    .await?;
    let mut command = message("scheduled", "scheduled");
    command["scheduledTaskId"] = "task:scheduled".into();
    if !pinned {
        command
            .as_object_mut()
            .expect("test value")
            .remove("modelSelection");
    }
    dispatcher
        .dispatch(serde_json::from_value(command)?)
        .await?;
    let before = h
        .reader
        .projection(ThreadId("thread:test".into()))
        .await?
        .expect("test value");
    let queued = before
        .runs
        .iter()
        .find(|run| run.status == OrchestrationV2RunStatus::Queued)
        .expect("test value");
    let queued_message = before
        .messages
        .iter()
        .find(|message| message.id == queued.user_message_id)
        .expect("test value");
    assert_eq!(
        t3_contracts::stored::to_value(queued_message)?.get("followsThreadSelection"),
        (!pinned).then_some(&json!(true))
    );
    let current =
        json!({"instanceId":"codex","model":"current-selection","options":{"effort":"medium"}});
    dispatcher.dispatch(serde_json::from_value(json!({"type":"thread.model-selection.set","commandId":"select-current","threadId":"thread:test","modelSelection":current}))?).await?;
    dispatcher.dispatch(serde_json::from_value(json!({"type":"queued-message.promote-to-steer","commandId":"promote-scheduled","threadId":"thread:test","queuedRunId":queued.id,"targetRunId":"run:test"}))?).await?;
    let effect=h.database.stores.read(|db| {
            let payload:String=db.query_row("SELECT payload_json FROM orchestration_v2_effect_outbox WHERE json_extract(payload_json,'$.type')='provider-turn.steer' ORDER BY rowid DESC LIMIT 1",[],|row|row.get(0))?;
            Ok(serde_json::from_str::<Value>(&payload)?)
        }).await?;
    assert_eq!(
        effect["modelSelection"],
        if pinned {
            t3_contracts::stored::to_value(&queued.model_selection)?
        } else {
            current
        }
    );
    Ok(())
}

#[tokio::test]
async fn promoted_scheduled_delivery_follows_current_selection() -> Result<()> {
    check_promoted_scheduled_delivery(false).await
}

#[tokio::test]
async fn promoted_scheduled_delivery_with_a_pin_keeps_its_pick() -> Result<()> {
    check_promoted_scheduled_delivery(true).await
}

async fn check_scheduled_goal_case(edit: bool) -> Result<()> {
    let (h, dispatcher, _continuations) =
        super::remote_delegated_task_test::live_dispatcher_with_inputs(scheduled_inputs).await?;
    let workspace = tempfile::tempdir()?;
    let mut thread = h
        .reader
        .thread(ThreadId("thread:test".into()))
        .await?
        .expect("test value");
    thread.worktree_path = Some(t3_contracts::base_schemas::TrimmedNonEmptyString(
        workspace.path().to_string_lossy().into_owned(),
    ));
    h.write(
        "thread.metadata-updated",
        t3_contracts::stored::to_value(&thread)?,
    )
    .await?;
    let mut command = message("scheduled", if edit { "scheduled" } else { "/goal pause" });
    command["scheduledTaskId"] = "task:scheduled".into();
    command
        .as_object_mut()
        .expect("test value")
        .remove("modelSelection");
    dispatcher
        .dispatch(serde_json::from_value(command)?)
        .await?;
    let before = h
        .reader
        .projection(ThreadId("thread:test".into()))
        .await?
        .expect("test value");
    let queued = before
        .runs
        .iter()
        .find(|run| run.status == OrchestrationV2RunStatus::Queued)
        .expect("test value");
    for run in before
        .runs
        .iter()
        .filter(|run| run.status != OrchestrationV2RunStatus::Queued)
    {
        let mut run = t3_contracts::stored::to_value(run)?;
        run["status"] = "interrupted".into();
        h.write("run.updated", run).await?;
    }
    for turn in before.provider_turns {
        let mut turn = t3_contracts::stored::to_value(&turn)?;
        turn["status"] = "interrupted".into();
        h.write("provider-turn.updated", turn).await?;
    }
    dispatcher.dispatch(serde_json::from_value(json!({"type":"thread.model-selection.set","commandId":"switch-claude","threadId":"thread:test","modelSelection":{"instanceId":"claudeAgent","model":"claude-test"}}))?).await?;
    if edit {
        let error=dispatcher.dispatch(serde_json::from_value(json!({"type":"queued-run.edit","commandId":"edit-goal","threadId":"thread:test","runId":queued.id,"text":"/goal pause"}))?).await.expect_err("test value");
        assert!(
            error
                .to_string()
                .contains("Claude goals cannot be paused or resumed"),
            "{error}"
        );
    } else {
        dispatcher
            .dispatch(serde_json::from_value(
                json!({"type":"queue.resume","commandId":"resume-goal","threadId":"thread:test"}),
            )?)
            .await?;
        dispatcher
            .resume_queue(ThreadId("thread:test".into()), None)
            .await?;
    }
    let after = h
        .reader
        .projection(ThreadId("thread:test".into()))
        .await?
        .expect("test value");
    assert_eq!(
        after
            .messages
            .iter()
            .find(|message| message.id == queued.user_message_id)
            .expect("test value")
            .text,
        if edit { "scheduled" } else { "/goal pause" }
    );
    assert_eq!(
        after
            .runs
            .iter()
            .find(|run| run.id == queued.id)
            .expect("test value")
            .status,
        if edit {
            OrchestrationV2RunStatus::Queued
        } else {
            OrchestrationV2RunStatus::Failed
        }
    );
    if !edit {
        let failure = after
            .turn_items
            .iter()
            .find_map(|item| match item {
                OrchestrationV2TurnItem::Error(item)
                    if item.run_id.as_ref() == Some(&queued.id) =>
                {
                    Some(item)
                }
                _ => None,
            })
            .expect("queued-start refusal must persist an error");
        assert_eq!(
            failure.failure.code.as_ref().map(|code| code.0.as_str()),
            Some("queued_start_failed")
        );
        assert!(
            failure
                .failure
                .message
                .0
                .contains("Claude goals cannot be paused or resumed"),
            "{:?}",
            failure.failure
        );
        let id = t3_contracts::stored::to_value(&queued.id)?
            .as_str()
            .expect("test value")
            .to_owned();
        let starts=h.database.stores.read(move |db|Ok(db.query_row("SELECT COUNT(*) FROM orchestration_v2_effect_outbox WHERE json_extract(payload_json,'$.type')='provider-turn.start' AND json_extract(payload_json,'$.runId')=?1",[id],|row|row.get::<_,i64>(0))?)).await?;
        assert_eq!(starts, 0);
    }
    Ok(())
}

#[tokio::test]
async fn scheduled_goal_edit_validates_the_threads_current_provider() -> Result<()> {
    check_scheduled_goal_case(true).await
}

#[tokio::test]
async fn scheduled_goal_start_validates_the_threads_current_provider() -> Result<()> {
    check_scheduled_goal_case(false).await
}
