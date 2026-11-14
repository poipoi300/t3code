use super::provider_event_ingestor_test::{Harness, decode, fixture, item};
use serde_json::json;
use t3_orchestration::{api::*, error::Result, ports::runs_ingestion::context_handoff_service::*};
#[test]
fn side_chat_context_is_bounded_keeps_recent_work_and_never_splits_unicode() {
    let digest = (0..100)
        .map(|index| format!("- User: {index} {}", "🚀".repeat(100)))
        .collect::<Vec<_>>();
    let context = make_side_chat_context(&ThreadId("parent".into()), &digest);
    assert!(context.encode_utf16().count() <= 8_000);
    assert!(context.contains("- Earlier items truncated."));
    assert!(!context.contains("- User: 0 "));
    assert!(context.contains("- User: 99 "));
    assert!(
        context
            .ends_with("Use t3_thread_read on thread parent to see the main thread's live state.")
    );
    assert_eq!(compact_text("  alpha \n beta ", 240), "alpha beta");
    assert_eq!(
        compact_text("\u{feff}alpha \u{0085} beta\u{feff}", 240),
        "alpha \u{0085} beta"
    );
    assert_eq!(
        compact_text(&format!("{}🚀end", "x".repeat(236)), 240),
        format!("{}...", "x".repeat(236))
    );
    let empty = make_side_chat_context(&ThreadId("parent".into()), &[]);
    assert!(empty.starts_with("You are in a side chat beside the main thread."));
    assert!(!empty.contains("Main thread activity"));
    let mut patch = item("file_change", "patch");
    patch["fileName"] = "src/a.ts".into();
    patch["changes"] = json!([{"operation":"update","path":"src/a.ts"},{"operation":"add","path":"src/b.ts"},{"operation":"update","path":"src/new.ts","oldPath":"src/old.ts"}]);
    assert_eq!(
        summarize_delta_item(&decode(patch)).unwrap().unwrap(),
        "- File change: src/a.ts, src/b.ts, src/new.ts"
    );
}
#[test]
fn legacy_import_summary_keeps_role_attribution_and_the_newest_oversized_unicode_tail() -> Result<()>
{
    let service = ContextHandoffService {
        ids: std::sync::Arc::new(t3_orchestration::testing::SequenceIds::new("handoff")),
        budget: std::sync::Arc::new(t3_orchestration::ports::runs_ingestion::context_handoff_delivery::PortableHandoffBudget),
        token_cap: 32_000,
    };
    for (text, expected) in [
        ("What did we decide?".to_owned(), "What did we decide?"),
        (
            format!("{} retained final words", "x".repeat(35_000)),
            "User:\n... retained final words",
        ),
        (
            format!("{}LATEST_SINGLE_TOKEN", "🧪".repeat(20_000)),
            "LATEST_SINGLE_TOKEN",
        ),
    ] {
        let mut user = item("user_message", "legacy:user");
        user["runId"] = serde_json::Value::Null;
        user["messageId"] = "message:user".into();
        user["createdBy"] = "user".into();
        user["creationSource"] = "server".into();
        user["inputIntent"] = "turn_start".into();
        user["attachments"] = json!([]);
        let oversized = text.encode_utf16().count() > 32_000;
        user["text"] = text.into();
        let mut assistant = item("assistant_message", "legacy:assistant");
        assistant["runId"] = serde_json::Value::Null;
        assistant["messageId"] = "message:assistant".into();
        assistant["text"] = "We decided to keep the migration lightweight.".into();
        assistant["streaming"] = false.into();
        assistant["attachments"] = json!([]);
        let handoff = service.prepare(PrepareHandoff {
            thread_id: ThreadId("thread:test".into()),
            target_run_id: RunId("run:first-v2".into()),
            transfer_id: None,
            from_provider_thread_ids: vec![],
            to_provider_thread_id: ProviderThreadId("pt:first-v2".into()),
            covered_run_ordinals: ContextHandoffCoveredRunOrdinals { r#from: 1, to: 1 },
            items: vec![decode(assistant), decode(user)],
            runs: vec![],
            created_at: t3_contracts::wire::WireDateTime(
                t3_orchestration::testing::TEST_TIME.into(),
            ),
            content: HandoffContent::LegacyImport,
        })?;
        assert_eq!(handoff.strategy, ContextHandoffStrategy::ManualContext);
        assert!(handoff.from_provider_thread_ids.is_empty());
        assert!(handoff.summary_text.contains(expected));
        assert!(handoff.summary_text.encode_utf16().count() <= 32_000);
        assert!(!handoff.summary_text.contains('\u{fffd}'));
        if oversized {
            assert!(
                handoff
                    .history
                    .as_ref()
                    .unwrap()
                    .as_ref()
                    .unwrap()
                    .omitted_item_ids
                    .as_ref()
                    .unwrap()
                    .as_ref()
                    .unwrap()
                    .contains(&TurnItemId("legacy:user".into()))
            );
        }
        if expected == "What did we decide?" {
            assert!(
                handoff
                    .summary_text
                    .contains("keep the migration lightweight")
            );
        }
    }
    Ok(())
}
#[test]
fn side_chat_summarizes_only_the_recent_items_that_can_fit() -> Result<()> {
    let mut summarized = 0;
    let items = (0..60).collect::<Vec<_>>();
    let text = make_side_chat_context_with(&ThreadId("parent".into()), &items, |index| {
        summarized += 1;
        Ok(Some(format!(
            "- Assistant: item-{index} {}😀{}",
            "x".repeat(228),
            "x".repeat(70)
        )))
    })?;
    assert!(
        summarized < items.len(),
        "summarized {summarized} discarded parent items"
    );
    assert!(text.contains("- Earlier items truncated."));
    assert!(!text.contains("item-0 "));
    assert!(text.contains("- Assistant: item-59 "));
    assert!(text.encode_utf16().count() <= 8_000);
    Ok(())
}
#[test]
fn visible_history_excludes_rolled_back_cancelled_queues_and_plain_superseded_interrupts()
-> Result<()> {
    let mut run = fixture("OrchestrationV2Run");
    run["id"] = "run:test".into();
    run["status"] = "rolled_back".into();
    let mut user = item("user_message", "user");
    user["messageId"] = "message:queued".into();
    user["createdBy"] = "user".into();
    user["creationSource"] = "web".into();
    user["text"] = "queued".into();
    user["attachments"] = json!([]);
    user["inputIntent"] = "queued_turn".into();
    assert!(
        visible_conversation_items(&[decode(run.clone())], &[], vec![decode(user.clone())])?
            .is_empty()
    );
    run["status"] = "cancelled".into();
    assert!(
        visible_conversation_items(&[decode(run.clone())], &[], vec![decode(user.clone())])?
            .is_empty()
    );
    user["inputIntent"] = "turn_start".into();
    let user: OrchestrationV2TurnItem = decode(user);
    assert_eq!(
        visible_conversation_items(&[decode(run.clone())], &[], vec![user.clone()])?.len(),
        1
    );
    let range = visible_delta_run_ordinals(&[decode(run)], &[user])?;
    assert_eq!(range.r#from, range.to);
    assert_eq!(visible_delta_run_ordinals(&[], &[])?.r#from, 1);
    let mut attempt = fixture("OrchestrationV2RunAttempt");
    attempt["runId"] = "run:test".into();
    attempt["rootNodeId"] = "node:test".into();
    attempt["status"] = "superseded".into();
    let attempt: OrchestrationV2RunAttempt = decode(attempt);
    let mut interrupted = item("run_interrupt_result", "interrupted");
    interrupted["message"] = "Interrupted for steering".into();
    let interrupted: OrchestrationV2TurnItem = decode(interrupted);
    assert!(
        visible_conversation_items(
            &[],
            std::slice::from_ref(&attempt),
            vec![interrupted.clone()]
        )?
        .is_empty()
    );
    assert_eq!(
        visible_conversation_items(&[], &[], vec![interrupted.clone()])?.len(),
        1
    );
    let mut stopped = item("run_interrupt_request", "stopped");
    stopped["message"] = "Stopped by user".into();
    assert_eq!(
        visible_conversation_items(&[], &[attempt], vec![interrupted, decode(stopped)])?.len(),
        2
    );
    Ok(())
}

#[test]
fn very_long_delta_keeps_its_entire_run_range_without_a_stack_sized_reduction() -> Result<()> {
    let mut first = fixture("OrchestrationV2Run");
    first["id"] = "early".into();
    first["ordinal"] = 2.into();
    let mut last = first.clone();
    last["id"] = "late".into();
    last["ordinal"] = 5.into();
    let mut values = Vec::new();
    for id in ["early", "late"] {
        let mut value = item("reasoning", "range");
        value["runId"] = id.into();
        value["text"] = "".into();
        value["streaming"] = false.into();
        values.push(decode::<OrchestrationV2TurnItem>(value));
    }
    let mut items = (0..150_000)
        .map(|index| values[index % 2].clone())
        .collect::<Vec<_>>();
    let mut runless = t3_contracts::stored::to_value(&values[0])?;
    runless["runId"] = serde_json::Value::Null;
    items.push(decode(runless));
    let range = visible_delta_run_ordinals(&[decode(first), decode(last)], &items)?;
    assert_eq!((range.r#from, range.to), (2, 5));
    Ok(())
}
#[tokio::test]
async fn portable_forks_inherit_only_visible_history_through_the_fork_point() -> Result<()> {
    let h = Harness::new().await?;
    for ordinal in [1, 2] {
        let mut run = fixture("OrchestrationV2Run");
        run["id"] = format!("run:{ordinal}").into();
        run["threadId"] = "thread:test".into();
        run["ordinal"] = ordinal.into();
        run["status"] = "completed".into();
        h.write("run.updated", run).await?;
        let mut message = item("assistant_message", &format!("item:{ordinal}"));
        message["runId"] = format!("run:{ordinal}").into();
        message["ordinal"] = ordinal.into();
        message["messageId"] = format!("message:{ordinal}").into();
        message["attachments"] = json!([]);
        message["text"] = format!("answer {ordinal}").into();
        message["streaming"] = false.into();
        h.write("turn-item.updated", message).await?;
    }
    let mut child = fixture("OrchestrationV2AppThread");
    child["id"] = "child".into();
    child["forkedFrom"] = json!({"type":"run","threadId":"thread:test","runId":"run:1"});
    child["lineage"] = json!({"rootThreadId":"thread:test","parentThreadId":"thread:test","relationshipToParent":"fork"});
    h.database.stores.transaction(|db|{db.execute("UPDATE orchestration_v2_projection_turn_items SET payload_json=json_set(payload_json,'$.id',NULL) WHERE turn_item_id='item:2'",[])?;Ok(())}).await?;
    let items = read_inherited_handoff_items(h.reader.as_ref(), &decode(child.clone())).await?;
    assert_eq!(items.len(), 1);
    assert_eq!(
        t3_contracts::stored::to_value(&items[0])?["text"],
        "answer 1"
    );
    let mut imported = t3_contracts::stored::to_value(
        &h.reader
            .thread(ThreadId("thread:test".into()))
            .await?
            .unwrap(),
    )?;
    imported["historyOrigin"] = "v1_import".into();
    let mut legacy = item("assistant_message", "legacy-runless");
    legacy["runId"] = serde_json::Value::Null;
    legacy["messageId"] = "message:legacy".into();
    legacy["text"] = "Imported answer".into();
    legacy["streaming"] = false.into();
    legacy["attachments"] = json!([]);
    h.write("turn-item.updated", legacy).await?;
    let imported = read_fork_history_items(h.reader.as_ref(), &decode(imported), 1).await?;
    assert_eq!(imported.len(), 2);
    assert!(
        imported
            .iter()
            .any(|item| t3_contracts::stored::to_value(item).unwrap()["text"] == "Imported answer")
    );
    child["lineage"]["sideChat"] = true.into();
    assert!(
        read_inherited_handoff_items(h.reader.as_ref(), &decode(child.clone()))
            .await?
            .is_empty()
    );
    child["lineage"]["sideChat"] = false.into();
    child["forkedFrom"] = json!({"type":"run","threadId":"missing-ancestor","runId":"missing-run"});
    assert!(
        read_inherited_handoff_items(h.reader.as_ref(), &decode(child))
            .await?
            .is_empty()
    );
    Ok(())
}
