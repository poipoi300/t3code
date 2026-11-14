#![allow(clippy::unwrap_used)]

use super::{
    orchestrator_receipt_race_test::Harness,
    thread_management_service_test::{emit, run, service, settings},
};
use serde_json::{Value, json};
use std::sync::Arc;
use t3_orchestration::{
    api::*,
    effects::TitleKind,
    error::{Error, Result},
    ports::thread_project::thread_title_regeneration_service::*,
    stores::{outbox, projects},
    testing::{FakeTitleGenerator, TEST_TIME},
};
use tokio_util::sync::CancellationToken;

struct BlockingTitle(tokio::sync::mpsc::UnboundedSender<CancellationToken>);
#[async_trait::async_trait]
impl TitleGenerator for BlockingTitle {
    async fn generate(
        &self,
        _: ThreadId,
        _: Vec<OrchestrationV2HistoricalMessage>,
        _: CancellationToken,
    ) -> Result<String> {
        Err(Error::Unsupported("legacy title input"))
    }
    async fn generate_with_input(
        &self,
        _: TitleGenerationInput,
        cancellation: CancellationToken,
    ) -> Result<String> {
        self.0.send(cancellation.clone()).unwrap();
        cancellation.cancelled().await;
        Err(Error::Cancelled)
    }
}

#[tokio::test]
async fn title_shutdown_cancels_backend_and_joins_active_generation_without_completing_marker()
-> Result<()> {
    let harness = Harness::new(None, false);
    harness.create("thread").await?;
    project(&harness).await?;
    emit(
        &harness,
        "message",
        "message.updated",
        message("message", "user", "Some work"),
    )
    .await?;
    arm(&harness, "request").await?;
    let settings = settings(json!({}));
    let (management, _, _) = service(&harness, settings.clone(), true);
    let (sender, mut entered) = tokio::sync::mpsc::unbounded_channel();
    let title = Arc::new(ThreadTitleRegenerationService::new(
        management,
        harness.database.stores.clone(),
        settings,
        Arc::new(BlockingTitle(sender)),
    ));
    let executing = tokio::spawn({
        let title = title.clone();
        async move {
            title
                .execute(
                    ThreadId("thread".into()),
                    CommandId("request".into()),
                    TitleKind::Regenerate,
                    CancellationToken::new(),
                )
                .await
        }
    });
    let backend_cancellation = entered.recv().await.unwrap();
    title.shutdown().await;
    assert!(backend_cancellation.is_cancelled());
    assert!(matches!(executing.await.unwrap(), Err(Error::Cancelled)));
    assert_eq!(
        json!(
            harness
                .reader
                .thread(ThreadId("thread".into()))
                .await?
                .unwrap()
        )["titleRegeneration"]["requestId"],
        "request"
    );
    assert!(matches!(
        title
            .execute(
                ThreadId("thread".into()),
                CommandId("request".into()),
                TitleKind::Regenerate,
                CancellationToken::new()
            )
            .await,
        Err(Error::Cancelled)
    ));
    title.shutdown().await;
    assert!(entered.try_recv().is_err());
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn initial_titles_retry_twice_while_cancellation_and_superseding_requests_stop_retries()
-> Result<()> {
    for mode in ["retry", "cancel", "superseded"] {
        let harness = Harness::new(None, false);
        harness.create("thread").await?;
        project(&harness).await?;
        emit(
            &harness,
            "message",
            "message.updated",
            message("message", "user", "Retry this title"),
        )
        .await?;
        arm(&harness, "initial").await?;
        let generator = Arc::new(FakeTitleGenerator::default());
        generator.generate_with_input.push(Err(Error::Service {
            operation: "TextGeneration.generateThreadTitle",
            detail: "temporary failure".into(),
        }));
        if mode == "retry" {
            generator.generate_with_input.push(Err(Error::Service {
                operation: "TextGeneration.generateThreadTitle",
                detail: "temporary failure".into(),
            }));
            generator
                .generate_with_input
                .push(Ok("Retried title".into()));
        }
        let service = titles(&harness, generator.clone());
        let cancellation = CancellationToken::new();
        let executing = tokio::spawn({
            let cancellation = cancellation.clone();
            async move {
                service
                    .execute(
                        ThreadId("thread".into()),
                        CommandId("initial".into()),
                        TitleKind::Initial {
                            message_id: MessageId("message".into()),
                        },
                        cancellation,
                    )
                    .await
            }
        });
        generator.generate_with_input.next_call().await.unwrap();
        if mode == "cancel" {
            cancellation.cancel();
            assert!(matches!(executing.await.unwrap(), Err(Error::Cancelled)));
        } else {
            if mode == "superseded" {
                arm(&harness, "new-request").await?;
            }
            tokio::time::advance(std::time::Duration::from_secs(2)).await;
            if mode == "retry" {
                generator.generate_with_input.next_call().await.unwrap();
                tokio::time::advance(std::time::Duration::from_secs(4)).await;
                generator.generate_with_input.next_call().await.unwrap();
            }
            executing.await.unwrap()?;
        }
        let thread = json!(
            harness
                .reader
                .thread(ThreadId("thread".into()))
                .await?
                .unwrap()
        );
        assert_eq!(
            thread["title"],
            if mode == "retry" {
                "Retried title"
            } else {
                "Before"
            }
        );
        if mode == "superseded" {
            assert_eq!(thread["titleRegeneration"]["requestId"], "new-request");
        }
        assert!(thread["lastTitleRegeneration"].is_null());
    }
    Ok(())
}

fn message(id: &str, role: &str, text: &str) -> Value {
    json!({"id":id,"threadId":"thread","runId":null,"nodeId":null,"role":role,"text":text,"attachments":[],"streaming":false,"createdBy":"user","creationSource":"web","createdAt":TEST_TIME,"updatedAt":TEST_TIME})
}

async fn project(harness: &Harness) -> Result<()> {
    harness.database.stores.transaction(|db| {
        projects::upsert(db, &serde_json::from_value(json!({"projectId":"project","title":"Project","workspaceRoot":"C:/repo","hasCustomTitle":false,"defaultModelSelection":null,"defaultThreadEnvMode":null,"autoPull":false,"faviconPath":null,"projectIcon":null,"scripts":[],"pinnedAt":null,"archivedAt":null,"createdAt":TEST_TIME,"updatedAt":TEST_TIME,"deletedAt":null}))?)
    }).await
}

async fn arm(harness: &Harness, request: &str) -> Result<()> {
    harness.dispatch(json!({"type":"thread.metadata.update","threadId":"thread","commandId":request,"regenerateTitle":true})).await?;
    Ok(())
}

fn titles(harness: &Harness, generator: Arc<FakeTitleGenerator>) -> ThreadTitleRegenerationService {
    let settings = settings(
        json!({"threadTitleInstructions":"Global instructions","projectSettingsOverrides":{"project":{"threadTitleInstructions":"Project instructions","textGenerationModelSelection":{"instanceId":"codex","model":"title-model"}}}}),
    );
    let (management, _, _) = service(harness, settings.clone(), true);
    ThreadTitleRegenerationService::new(
        management,
        harness.database.stores.clone(),
        settings,
        generator,
    )
}

#[test]
fn digest_pins_first_user_retains_recent_answers_and_bounds_utf16_and_attachments() -> Result<()> {
    let messages = [
        message(
            "first",
            "user",
            &format!("Ancient context that anchors the topic {}", "x".repeat(600)),
        ),
        message("answer", "assistant", &"y".repeat(6000)),
        message("last", "user", &"z".repeat(1500)),
        message("system", "system", "Do not include system instructions"),
    ]
    .into_iter()
    .map(serde_json::from_value)
    .collect::<std::result::Result<Vec<_>, _>>()?;
    let context = format_thread_title_context(&messages)?;
    assert!(
        context
            .message
            .contains("USER:\nAncient context that anchors the topic")
    );
    assert!(
        context
            .message
            .starts_with("[Earlier content truncated]\n\n")
    );
    assert!(context.message.contains(&"y".repeat(100)));
    assert!(
        !context
            .message
            .contains("Do not include system instructions")
    );
    assert!(context.message.encode_utf16().count() <= 8000);
    let truncated = limit_title_message(&"😀".repeat(6000), 2000);
    assert!(truncated.encode_utf16().count() <= 2000);
    assert!(!truncated.contains('\u{fffd}'));
    let messages = (0..8).map(|index| {
        let mut value = message(&format!("image:{index}"), "user", "");
        value["attachments"] = json!([{"type":"image","id":format!("image:{index}"),"name":format!("diagram-{index}.png"),"mimeType":"image/png","sizeBytes":10}]);
        serde_json::from_value(value)
    }).collect::<std::result::Result<Vec<_>, _>>()?;
    let context = format_thread_title_context(&messages)?;
    assert_eq!(
        context
            .attachments
            .iter()
            .map(|attachment| json!(attachment)["id"].clone())
            .collect::<Vec<_>>(),
        [
            json!("image:0"),
            json!("image:5"),
            json!("image:6"),
            json!("image:7")
        ]
    );
    Ok(())
}

#[test]
fn digest_expands_valid_assistant_quotes_and_preserves_invalid_links() -> Result<()> {
    let valid = "[Assistant quote](t3-citation://v1/environment/thread/message?text=Quoted+answer&start=0&end=13&prefix=&suffix=&comment=Use+this)";
    let invalid = "[Assistant quote](t3-citation://v1/environment/thread/message?text=Other&start=4&end=2&prefix=&suffix=)";
    let context = format_thread_title_context(&[serde_json::from_value(message(
        "quoted",
        "user",
        &format!("{valid}\n{invalid}"),
    ))?])?;
    assert!(context.message.contains("Quoted answer\nComment: Use this"));
    assert!(!context.message.contains(valid));
    assert!(context.message.contains(invalid));
    for (text, expected) in [
        ("\u{feff}subject\u{feff}", "USER:\nsubject"),
        ("\u{85}subject\u{85}", "USER:\n\u{85}subject\u{85}"),
    ] {
        assert_eq!(
            format_thread_title_context(&[serde_json::from_value(message(
                "whitespace",
                "user",
                text
            ))?])?
            .message,
            expected
        );
    }
    Ok(())
}

#[tokio::test]
async fn image_only_initial_title_omits_bootstrap_without_changing_persisted_message() -> Result<()>
{
    for prefix in ["", "Ultrathink:\n"] {
        let harness = Harness::new(None, false);
        harness.create("thread").await?;
        project(&harness).await?;
        let text = format!(
            "{prefix}[User attached one or more files without additional text. Respond using the conversation context and the attached files.]"
        );
        let mut value = message("image", "user", &text);
        value["attachments"] = json!([{"type":"image","id":"diagram","name":"diagram.png","mimeType":"image/png","sizeBytes":10}]);
        emit(&harness, "image", "message.updated", value.clone()).await?;
        arm(&harness, "initial").await?;
        let generator = Arc::new(FakeTitleGenerator::default());
        generator
            .generate_with_input
            .push(Ok("Generated title".into()));
        titles(&harness, generator.clone())
            .execute(
                ThreadId("thread".into()),
                CommandId("initial".into()),
                TitleKind::Initial {
                    message_id: MessageId("image".into()),
                },
                CancellationToken::new(),
            )
            .await?;
        let (call, _) = generator.generate_with_input.next_call().await.unwrap();
        assert_eq!(call.message, "");
        assert_eq!(json!(call.attachments), value["attachments"]);
        assert!(call.previous_title.is_none());
        let projection = harness
            .reader
            .projection(ThreadId("thread".into()))
            .await?
            .unwrap();
        assert_eq!(projection.messages[0].text, text);
        assert_eq!(projection.thread.title.0, "Generated title");
    }
    Ok(())
}

#[tokio::test]
async fn metadata_arms_cancels_and_supersedes_generation_without_late_renames() -> Result<()> {
    let harness = Harness::new(None, false);
    harness.create("thread").await?;
    project(&harness).await?;
    arm(&harness, "first").await?;
    let rows = harness
        .database
        .stores
        .read(|db| outbox::list_by_command(db, &CommandId("first".into())))
        .await?;
    assert_eq!(rows.len(), 1);
    assert!(matches!(
        rows[0].request,
        t3_orchestration::effects::EffectRequest::GenerateTitle {
            kind: TitleKind::Regenerate
        }
    ));
    harness.dispatch(json!({"type":"thread.metadata.update","threadId":"thread","commandId":"abandon","regenerateTitle":false})).await?;
    assert!(
        json!(
            harness
                .reader
                .thread(ThreadId("thread".into()))
                .await?
                .unwrap()
        )["titleRegeneration"]
            .is_null()
    );
    arm(&harness, "second").await?;
    let generator = Arc::new(FakeTitleGenerator::default());
    let before = harness.count_events().await;
    titles(&harness, generator)
        .execute(
            ThreadId("thread".into()),
            CommandId("first".into()),
            TitleKind::Regenerate,
            CancellationToken::new(),
        )
        .await?;
    assert_eq!(harness.count_events().await, before);
    assert_eq!(
        json!(
            harness
                .reader
                .thread(ThreadId("thread".into()))
                .await?
                .unwrap()
        )["titleRegeneration"]["requestId"],
        "second"
    );
    harness.dispatch(json!({"type":"thread.metadata.update","threadId":"thread","commandId":"manual","title":"Manual title"})).await?;
    let thread = json!(
        harness
            .reader
            .thread(ThreadId("thread".into()))
            .await?
            .unwrap()
    );
    assert_eq!(thread["title"], "Manual title");
    assert!(thread["titleRegeneration"].is_null());
    Ok(())
}

#[tokio::test]
async fn regeneration_uses_digest_project_settings_and_existing_worktree_or_project_root()
-> Result<()> {
    let directory = tempfile::tempdir().unwrap();
    for existing in [false, true] {
        let harness = Harness::new(None, false);
        harness.create("thread").await?;
        project(&harness).await?;
        let path = if existing {
            directory.path().to_path_buf()
        } else {
            directory.path().join("removed-worktree")
        };
        harness.dispatch(json!({"type":"thread.metadata.update","threadId":"thread","commandId":"worktree","worktreePath":path})).await?;
        emit(
            &harness,
            "message",
            "message.updated",
            message("message", "user", "Investigate the flaky login test"),
        )
        .await?;
        let mut streaming = message("streaming", "assistant", "Incomplete answer");
        streaming["streaming"] = json!(true);
        emit(&harness, "streaming", "message.updated", streaming).await?;
        arm(&harness, "request").await?;
        let generator = Arc::new(FakeTitleGenerator::default());
        generator.generate_with_input.push(Ok("Fresh title".into()));
        titles(&harness, generator.clone())
            .execute(
                ThreadId("thread".into()),
                CommandId("request".into()),
                TitleKind::Regenerate,
                CancellationToken::new(),
            )
            .await?;
        let (call, _) = generator.generate_with_input.next_call().await.unwrap();
        assert_eq!(call.previous_title.as_deref(), Some("Before"));
        assert_eq!(call.cwd, if existing { path } else { "C:/repo".into() });
        assert_eq!(call.project_title, "Project");
        assert_eq!(call.instructions, "Project instructions");
        assert_eq!(call.model_selection.model, "title-model");
        assert!(
            call.message
                .contains("USER:\nInvestigate the flaky login test")
        );
        assert!(!call.message.contains("Incomplete answer"));
        let thread = json!(
            harness
                .reader
                .thread(ThreadId("thread".into()))
                .await?
                .unwrap()
        );
        assert_eq!(thread["title"], "Fresh title");
        assert!(thread["titleRegeneration"].is_null());
        assert_eq!(thread["lastTitleRegeneration"]["status"], "renamed");
        assert_eq!(thread["lastTitleRegeneration"]["requestId"], "request");
    }
    Ok(())
}

#[tokio::test]
async fn fork_title_uses_inherited_messages_through_fork_point_excluding_rolled_back_work()
-> Result<()> {
    let harness = Harness::new(None, false);
    harness.create("source").await?;
    harness.create("thread").await?;
    project(&harness).await?;
    for (id, ordinal, status, text) in [
        ("discarded", 1, "rolled_back", "Rolled back attempt"),
        (
            "fork-point",
            2,
            "completed",
            "Move session storage from Redis to Postgres",
        ),
        ("later", 3, "completed", "Later work the fork never saw"),
    ] {
        let mut source_run = run(id, ordinal, status);
        source_run["threadId"] = json!("source");
        let mut user = message(&format!("{id}:user"), "user", text);
        user["threadId"] = json!("source");
        user["runId"] = json!(id);
        let mut assistant = message(
            &format!("{id}:assistant"),
            "assistant",
            "Sessions now persist in the Postgres sessions table.",
        );
        assistant["threadId"] = json!("source");
        assistant["runId"] = json!(id);
        let events = [("run.created", source_run), ("message.updated", user), ("message.updated", assistant)].into_iter().enumerate().map(|(index,(kind,payload))| serde_json::from_value(json!({"id":format!("{id}:{index}"),"threadId":"source","type":kind,"occurredAt":TEST_TIME,"payload":payload}))).collect::<std::result::Result<_,_>>()?;
        harness
            .sink
            .commit(CommitRequest {
                receipt: None,
                decision: Decision {
                    events,
                    ..Default::default()
                },
                guard: CommitGuard::None,
            })
            .await?;
    }
    let mut thread = json!(
        harness
            .reader
            .thread(ThreadId("thread".into()))
            .await?
            .unwrap()
    );
    thread["forkedFrom"] = json!({"type":"run","threadId":"source","runId":"fork-point"});
    emit(&harness, "fork", "thread.metadata-updated", thread).await?;
    emit(
        &harness,
        "message",
        "message.updated",
        message("message", "user", "Try SQLite instead"),
    )
    .await?;
    arm(&harness, "request").await?;
    let generator = Arc::new(FakeTitleGenerator::default());
    generator
        .generate_with_input
        .push(Ok("Generated title".into()));
    titles(&harness, generator.clone())
        .execute(
            ThreadId("thread".into()),
            CommandId("request".into()),
            TitleKind::Initial {
                message_id: MessageId("message".into()),
            },
            CancellationToken::new(),
        )
        .await?;
    let (call, _) = generator.generate_with_input.next_call().await.unwrap();
    assert_eq!(call.message, "Try SQLite instead");
    let source = call.fork_source.unwrap();
    assert_eq!(source.title, "Before");
    assert!(
        source
            .history
            .contains("Move session storage from Redis to Postgres")
    );
    assert!(
        source
            .history
            .contains("Sessions now persist in the Postgres")
    );
    assert!(!source.history.contains("Rolled back attempt"));
    assert!(!source.history.contains("Later work the fork never saw"));
    assert_eq!(
        harness
            .reader
            .thread(ThreadId("thread".into()))
            .await?
            .unwrap()
            .title
            .0,
        "Generated title"
    );
    Ok(())
}

#[tokio::test]
async fn failed_and_unchanged_generation_clear_marker_preserve_title_and_record_outcome()
-> Result<()> {
    for (result, expected) in [(Err(Error::Service { operation:"TextGeneration.generateThreadTitle", detail:"Codex CLI command failed: OpenAI Codex\nuser\nConversation text\nmodel unavailable\n".into() }), "failed"), (Ok("Before".into()), "unchanged"), (Ok(" New thread ".into()), "unchanged")] {
        let harness = Harness::new(None, false);
        harness.create("thread").await?;
        project(&harness).await?;
        emit(&harness, "message", "message.updated", message("message", "user", "Some conversation")).await?;
        arm(&harness, "request").await?;
        let generator = Arc::new(FakeTitleGenerator::default());
        generator.generate_with_input.push(result);
        titles(&harness, generator).execute(ThreadId("thread".into()), CommandId("request".into()), TitleKind::Regenerate, CancellationToken::new()).await?;
        let thread = json!(harness.reader.thread(ThreadId("thread".into())).await?.unwrap());
        assert_eq!(thread["title"], "Before");
        assert!(thread["titleRegeneration"].is_null());
        assert_eq!(thread["lastTitleRegeneration"]["status"], expected);
        if expected == "failed" { assert_eq!(thread["lastTitleRegeneration"]["message"], "model unavailable"); }
    }
    Ok(())
}
