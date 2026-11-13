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
