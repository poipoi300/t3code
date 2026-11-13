use super::provider_event_ingestor_test::{decode, fixture};
use serde_json::{Value, json};
use t3_orchestration::ports::runs_ingestion::{
    idle_compaction::*, idle_compaction_service::eligible, provider_event_ingestor::epoch_millis,
};
fn records() -> Value {
    let mut p = fixture("OrchestrationV2ThreadProjection");
    let mut thread = fixture("OrchestrationV2AppThread");
    let mut run = fixture("OrchestrationV2Run");
    let mut message = fixture("OrchestrationV2ConversationMessage");
    let mut provider = fixture("OrchestrationV2ProviderThread");
    let mut turn = fixture("OrchestrationV2ProviderTurn");
    thread["activeProviderThreadId"] = provider["id"].clone();
    thread["providerInstanceId"] = "claudeAgent".into();
    thread["settledAt"] = Value::Null;
    thread["settledOverride"] = Value::Null;
    thread["archivedAt"] = Value::Null;
    thread["deletedAt"] = Value::Null;
    thread["lineage"] =
        json!({"rootThreadId":thread["id"],"parentThreadId":null,"relationshipToParent":null});
    run["threadId"] = thread["id"].clone();
    run["providerThreadId"] = provider["id"].clone();
    run["providerInstanceId"] = "claudeAgent".into();
    run["userMessageId"] = message["id"].clone();
    run["status"] = "completed".into();
    run["completedAt"] = "2026-10-19T12:00:00.000Z".into();
    message["text"] = "Explain this code".into();
    provider["driver"] = "claudeAgent".into();
    provider["providerInstanceId"] = "claudeAgent".into();
    provider["pendingBackgroundTasks"] = json!([]);
    turn["providerThreadId"] = provider["id"].clone();
    turn["tokenUsage"] = json!({"usedTokens":250000,"promptCacheExpiresAt":"2026-10-19T13:00:00.000Z","updatedAt":"2026-10-19T12:00:00.000Z"});
    p["thread"] = thread;
    p["runs"] = json!([run]);
    p["messages"] = json!([message]);
    p["providerThreads"] = json!([provider]);
    p["providerTurns"] = json!([turn]);
    for key in ["turnItems", "runtimeRequests", "contextTransfers"] {
        p[key] = json!([]);
    }
    p
}
#[test]
fn eligibility_skips_small_unknown_switched_compacted_archived_and_side_chat_contexts() {
    let now = epoch_millis("2026-10-19T12:00:00.000Z").unwrap();
    let p = records();
    let window = idle_compaction_window(&decode(p.clone()), now)
        .unwrap()
        .unwrap();
    assert_eq!(window.end_ms - now, 3_600_000);
    assert_eq!(window.end_ms - window.start_ms, 120_000);
    for (path, value) in [
        (
            vec!["providerTurns", "0", "tokenUsage", "usedTokens"],
            json!(199999),
        ),
        (
            vec!["providerTurns", "0", "tokenUsage", "promptCacheExpiresAt"],
            Value::Null,
        ),
        (vec!["providerThreads", "0", "driver"], json!("codex")),
        (vec!["thread", "providerInstanceId"], json!("codex")),
        (vec!["messages", "0", "text"], json!(" /CoMpAcT ")),
        (
            vec!["thread", "archivedAt"],
            json!("2026-10-19T12:00:00.000Z"),
        ),
        (vec!["thread", "lineage", "sideChat"], json!(true)),
    ] {
        let mut candidate = p.clone();
        let mut target = &mut candidate;
        for segment in path {
            target = if let Ok(index) = segment.parse::<usize>() {
                &mut target[index]
            } else {
                &mut target[segment]
            };
        }
        *target = value;
        assert!(
            idle_compaction_window(&decode(candidate), now)
                .unwrap()
                .is_none()
        );
    }
    let mut p = p;
    let mut cancelled = p["runs"][0].clone();
    cancelled["id"] = "run:cancelled".into();
    cancelled["ordinal"] = 999.into();
    cancelled["status"] = "cancelled".into();
    cancelled["startedAt"] = Value::Null;
    p["runs"].as_array_mut().unwrap().push(cancelled);
    assert!(
        idle_compaction_window(&decode(p.clone()), now)
            .unwrap()
            .is_some()
    );
    p["runs"][1]["startedAt"] = "2026-10-19T12:00:00.000Z".into();
    assert!(idle_compaction_window(&decode(p), now).unwrap().is_none());
}
#[test]
fn settings_policy_matches_global_instance_and_project_overrides() {
    let thread = json!({"providerInstanceId":"claudeAgent","projectId":"project"});
    for (mode, override_mode, enabled, project_enabled, driver, expected) in [
        ("onReturn", "default", true, true, "claudeAgent", false),
        ("off", "default", true, true, "claudeAgent", false),
        (
            "beforeCacheExpires",
            "default",
            true,
            true,
            "claudeAgent",
            true,
        ),
        (
            "beforeCacheExpires",
            "off",
            true,
            true,
            "claudeAgent",
            false,
        ),
        (
            "onReturn",
            "beforeCacheExpires",
            true,
            true,
            "claudeAgent",
            true,
        ),
        (
            "beforeCacheExpires",
            "default",
            true,
            false,
            "claudeAgent",
            false,
        ),
        (
            "beforeCacheExpires",
            "default",
            false,
            true,
            "claudeAgent",
            false,
        ),
        ("beforeCacheExpires", "default", true, true, "codex", false),
    ] {
        let settings = json!({"idleCompaction":mode,"providerInstances":{"claudeAgent":{"driver":driver,"enabled":enabled,"config":{"idleCompaction":override_mode}}},"projectSettingsOverrides":if !project_enabled{json!({"project":{"providerInstances":{"claudeAgent":{"enabled":false}}}})}else{json!({})}});
        assert_eq!(eligible(&settings, &thread), expected, "{settings}");
    }
    assert!(eligible(
        &json!({"idleCompaction":"beforeCacheExpires","providerInstances":{}}),
        &thread
    ));
    assert!(!eligible(
        &json!({"idleCompaction":"beforeCacheExpires","providerInstances":{}}),
        &json!({"providerInstanceId":"custom-claude","projectId":"project"})
    ));
    assert!(eligible(
        &json!({"idleCompaction":"beforeCacheExpires","providerInstances":{"custom-claude":{"driver":"claudeAgent","config":{}}}}),
        &json!({"providerInstanceId":"custom-claude","projectId":"project"})
    ));
}

struct TraceClock {
    inner: std::sync::Arc<t3_orchestration::testing::ManualClock>,
    reads: tokio::sync::mpsc::UnboundedSender<()>,
}
impl t3_orchestration::api::Clock for TraceClock {
    fn now(&self) -> t3_orchestration::api::IsoDateTime {
        let now = t3_orchestration::api::Clock::now(self.inner.as_ref());
        let _ = self.reads.send(());
        now
    }
}
#[tokio::test(start_paused = true)]
async fn a_late_earlier_completion_retargets_the_timer_to_the_latest_completed_run()
-> t3_orchestration::error::Result<()> {
    use super::provider_event_ingestor_test::Harness;
    use std::sync::Arc;
    use t3_orchestration::stores::receipts::{AggregateId, CommandReceipt, ReceiptStatus};
    use t3_orchestration::{
        api::*, composition::StaticSettings,
        ports::runs_ingestion::idle_compaction_service::IdleCompactionService,
        testing::FakeCommandDispatcher,
    };
    let h = Harness::new().await?;
    h.clock.set("2026-10-19T12:00:00.000Z");
    let mut p = records();
    p["thread"]["id"] = "thread:test".into();
    p["thread"]["activeProviderThreadId"] = "pt:idle".into();
    p["providerThreads"][0]["id"] = "pt:idle".into();
    p["providerThreads"][0]["appThreadId"] = "thread:test".into();
    p["providerTurns"][0]["providerThreadId"] = "pt:idle".into();
    let mut first = p["runs"][0].clone();
    first["id"] = "run:first".into();
    first["threadId"] = "thread:test".into();
    first["providerThreadId"] = "pt:idle".into();
    first["userMessageId"] = "message:first".into();
    first["ordinal"] = 1.into();
    let mut latest = first.clone();
    latest["id"] = "run:latest".into();
    latest["userMessageId"] = "message:latest".into();
    latest["ordinal"] = 2.into();
    h.write("thread.created", p["thread"].clone()).await?;
    h.write("run.created", first.clone()).await?;
    h.write("run.created", latest.clone()).await?;
    for (id, run) in [
        ("message:first", "run:first"),
        ("message:latest", "run:latest"),
    ] {
        let mut message = p["messages"][0].clone();
        message["id"] = id.into();
        message["threadId"] = "thread:test".into();
        message["runId"] = run.into();
        h.write("message.updated", message).await?;
    }
    h.write("provider-thread.updated", p["providerThreads"][0].clone())
        .await?;
    let mut turn = p["providerTurns"][0].clone();
    turn["threadId"] = "thread:test".into();
    h.write("provider-turn.updated", turn).await?;
    let mut settings =
        t3_contracts::stored::to_value(&t3_contracts::settings::ServerSettings::default())?;
    settings["idleCompaction"] = "beforeCacheExpires".into();
    let dispatcher = Arc::new(FakeCommandDispatcher::default());
    dispatcher.dispatch.push(Ok(CommandReceipt {
        command_id: CommandId("server:idle-compact:thread:test:run:latest".into()),
        aggregate: AggregateId::Thread(ThreadId("thread:test".into())),
        accepted_at: h.clock.now().0,
        command_type: "message.dispatch".into(),
        result_sequence: 0,
        status: ReceiptStatus::Accepted,
        error: None,
    }));
    let (reads, mut read) = tokio::sync::mpsc::unbounded_channel();
    let service = Arc::new(IdleCompactionService {
        projections: h.reader.clone(),
        settings: Arc::new(StaticSettings::new(serde_json::from_value(settings)?)),
        dispatcher: dispatcher.clone(),
        clock: Arc::new(TraceClock {
            inner: h.clock.clone(),
            reads,
        }),
    });
    let cancellation = tokio_util::sync::CancellationToken::new();
    let task = service.start(h.sink.as_ref(), cancellation.clone());
    h.write("run.updated", latest).await?;
    read.recv().await.unwrap();
    h.write("run.updated", first).await?;
    read.recv().await.unwrap();
    h.clock.set("2026-10-19T12:59:00.000Z");
    tokio::time::advance(std::time::Duration::from_secs(59 * 60)).await;
    let command = dispatcher.dispatch.next_call().await.unwrap();
    assert_eq!(
        t3_contracts::stored::to_value(&command)?["commandId"],
        "server:idle-compact:thread:test:run:latest"
    );
    cancellation.cancel();
    task.await.unwrap();
    Ok(())
}
