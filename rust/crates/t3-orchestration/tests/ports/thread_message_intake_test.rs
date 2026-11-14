use super::provider_event_ingestor_test::{Harness, item};
use async_trait::async_trait;
use serde_json::{Value, json};
use std::{
    path::Path,
    sync::{Arc, Mutex},
};
use t3_contracts::chat_attachment::ChatAttachment;
use t3_orchestration::{
    api::*,
    composition::StaticSettings,
    error::{Error, Result},
    ports::{
        runs_ingestion::{
            attachment_claims::{
                AttachmentClaimService, NoDocumentText, attachment_id, attachment_path,
            },
            thread_message_intake::ThreadMessageIntake,
        },
        thread_project::thread_management_service::{
            NoLegacyTranscript, ThreadManagementService, ThreadManagementServices,
        },
    },
    stores::receipts::{AggregateId, CommandReceipt, ReceiptStatus},
    testing::TEST_TIME,
};
struct Dispatch {
    commands: Mutex<Vec<Value>>,
    mode: &'static str,
}
#[async_trait]
impl CommandDispatcher for Dispatch {
    async fn dispatch(&self, command: OrchestrationV2ServerCommand) -> Result<CommandReceipt> {
        let value = t3_contracts::stored::to_value(&command)?;
        self.commands.lock().unwrap().push(value.clone());
        let command_id = serde_json::from_value(value["commandId"].clone())?;
        if self.mode == "reject" {
            return Err(Error::Rejected {
                command_id,
                reason: "already rejected".into(),
            });
        }
        if self.mode == "worktree" {
            return Err(Error::Service {
                operation: "ThreadManagementWorktreeRestoreError",
                detail: "Missing checkout could not be restored".into(),
            });
        }
        if self.mode == "ambiguous" {
            return Err(Error::Service {
                operation: "dispatch",
                detail: "write outcome unknown".into(),
            });
        }
        Ok(CommandReceipt {
            command_id,
            aggregate: AggregateId::Thread(serde_json::from_value(value["threadId"].clone())?),
            command_type: value["type"].as_str().unwrap().into(),
            accepted_at: TEST_TIME.into(),
            result_sequence: 1,
            status: ReceiptStatus::Accepted,
            error: None,
        })
    }
}
fn intake(
    h: &Harness,
    claims: Arc<AttachmentClaimService>,
    mode: &'static str,
) -> (ThreadMessageIntake, Arc<Dispatch>) {
    let dispatcher = Arc::new(Dispatch {
        commands: Mutex::new(vec![]),
        mode,
    });
    let threads = ThreadManagementService::new(ThreadManagementServices {
        reader: h.reader.clone(),
        dispatcher: dispatcher.clone(),
        committer: h.sink.clone(),
        database: h.database.stores.clone(),
        settings: Arc::new(StaticSettings::new(
            t3_contracts::settings::ServerSettings::default(),
        )),
        hydrator: Arc::new(NoLegacyTranscript),
        signals: h.sink.signals(),
        clock: h.clock.clone(),
    });
    (
        ThreadMessageIntake {
            threads,
            launches: None,
            attachments: claims,
            projections: h.reader.clone(),
        },
        dispatcher,
    )
}
fn upload(directory: &Path) -> ChatAttachment {
    let attachment:ChatAttachment=serde_json::from_value(json!({"type":"image","id":format!("pending-{}",uuid::Uuid::new_v4()),"name":"screen.png","mimeType":"image/png","sizeBytes":3})).unwrap();
    std::fs::write(attachment_path(directory, &attachment).unwrap(), [1, 2, 3]).unwrap();
    attachment
}
fn respond(attachment: &ChatAttachment) -> OrchestrationV2ServerCommand {
    serde_json::from_value(json!({"type":"runtime-request.respond","commandId":"command:answer","threadId":"thread:test","requestId":"request:one","answers":{"q":["Selected option"]},"attachmentsByQuestionId":{"q":[attachment]}})).unwrap()
}

#[tokio::test]
async fn queued_attachment_edits_resolve_the_message_from_the_run_id() -> Result<()> {
    let h = Harness::new().await?;
    let mut run = super::provider_event_ingestor_test::fixture("OrchestrationV2Run");
    run["id"] = "queued".into();
    run["threadId"] = "thread:test".into();
    run["status"] = "queued".into();
    run["userMessageId"] = "queued-message".into();
    h.write("run.created", run).await?;
    let mut message =
        super::provider_event_ingestor_test::fixture("OrchestrationV2ConversationMessage");
    message["id"] = "queued-message".into();
    message["threadId"] = "thread:test".into();
    message["runId"] = "queued".into();
    message["attachments"] = json!([]);
    h.write("message.updated", message).await?;
    let directory = tempfile::tempdir()?;
    let claims = Arc::new(AttachmentClaimService::new(
        directory.path().into(),
        Arc::new(NoDocumentText),
    ));
    let (intake, dispatcher) = intake(&h, claims.clone(), "accepted");
    let receipt=intake.dispatch_command(serde_json::from_value(json!({"type":"queued-run.edit","commandId":"edit","threadId":"thread:test","runId":"queued","text":"edited","attachments":[]}))?).await?;
    assert_eq!(receipt.command_id.0, "edit");
    assert_eq!(dispatcher.commands.lock().unwrap()[0]["runId"], "queued");
    claims.shutdown().await;
    Ok(())
}

#[tokio::test]
async fn question_paths_are_readable_and_ambiguous_dispatch_keeps_copies_while_rejections_release_them()
-> Result<()> {
    for mode in ["ambiguous", "reject", "worktree"] {
        let h = Harness::new().await?;
        let directory = tempfile::tempdir()?;
        let upload = upload(directory.path());
        let claims = Arc::new(AttachmentClaimService::new(
            directory.path().into(),
            Arc::new(NoDocumentText),
        ));
        let (intake, dispatcher) = intake(&h, claims.clone(), mode);
        assert!(intake.dispatch_command(respond(&upload)).await.is_err());
        let command = dispatcher.commands.lock().unwrap()[0].clone();
        let accepted: ChatAttachment =
            serde_json::from_value(command["attachmentsByQuestionId"]["q"][0].clone())?;
        assert_ne!(attachment_id(&accepted), attachment_id(&upload));
        assert_eq!(command["answers"]["q"][0], "Selected option");
        assert!(
            command["answers"]["q"][1]
                .as_str()
                .unwrap()
                .contains(attachment_id(&accepted))
        );
        assert_eq!(
            attachment_path(directory.path(), &accepted)
                .unwrap()
                .exists(),
            mode == "ambiguous"
        );
        assert!(attachment_path(directory.path(), &upload).unwrap().exists());
        claims.shutdown().await;
    }
    Ok(())
}

#[tokio::test]
async fn replayed_answers_release_new_claims_only_after_the_recorded_resolution_is_found()
-> Result<()> {
    for resolved in [true, false] {
        let h = Harness::new().await?;
        let directory = tempfile::tempdir()?;
        let upload = upload(directory.path());
        if resolved {
            let mut answer = item("user_input_request", "answer");
            answer["status"] = "completed".into();
            answer["requestId"] = "request:one".into();
            answer["questions"] = json!([]);
            answer["questionAnswer"] =
                json!({"requestId":"request:one","answers":{},"attachmentsByQuestionId":{}});
            h.write("turn-item.updated", answer).await?;
        }
        let claims = Arc::new(AttachmentClaimService::new(
            directory.path().into(),
            Arc::new(NoDocumentText),
        ));
        let (intake, dispatcher) = intake(&h, claims.clone(), "accepted");
        intake.dispatch_command(respond(&upload)).await?;
        let command = dispatcher.commands.lock().unwrap()[0].clone();
        let claimed: ChatAttachment =
            serde_json::from_value(command["attachmentsByQuestionId"]["q"][0].clone())?;
        assert_eq!(
            attachment_path(directory.path(), &claimed)
                .unwrap()
                .exists(),
            !resolved
        );
        assert!(attachment_path(directory.path(), &upload).unwrap().exists());
        claims.shutdown().await;
    }
    Ok(())
}

#[tokio::test]
async fn failed_question_preparation_rolls_back_and_retry_uses_preserved_sources() -> Result<()> {
    let h = Harness::new().await?;
    let directory = tempfile::tempdir()?;
    let first = upload(directory.path());
    let second = upload(directory.path());
    let second_path = attachment_path(directory.path(), &second).unwrap();
    std::fs::remove_file(&second_path)?;
    let claims = Arc::new(AttachmentClaimService::new(
        directory.path().into(),
        Arc::new(NoDocumentText),
    ));
    let (intake, dispatcher) = intake(&h, claims.clone(), "ambiguous");
    let command = serde_json::from_value(
        json!({"type":"runtime-request.respond","commandId":"command:retry","threadId":"thread:test","requestId":"request:one","attachmentsByQuestionId":{"q1":[first],"q2":[second]}}),
    )?;
    assert!(intake.dispatch_command(command).await.is_err());
    assert!(dispatcher.commands.lock().unwrap().is_empty());
    assert_eq!(std::fs::read_dir(directory.path())?.count(), 1);
    std::fs::write(second_path, [1, 2, 3])?;
    let command = serde_json::from_value(
        json!({"type":"runtime-request.respond","commandId":"command:retry","threadId":"thread:test","requestId":"request:one","attachmentsByQuestionId":{"q1":[first],"q2":[second]}}),
    )?;
    assert!(intake.dispatch_command(command).await.is_err());
    {
        let commands = dispatcher.commands.lock().unwrap();
        assert_eq!(commands.len(), 1);
        for attachment in [
            &commands[0]["attachmentsByQuestionId"]["q1"][0],
            &commands[0]["attachmentsByQuestionId"]["q2"][0],
        ] {
            let attachment: ChatAttachment = serde_json::from_value(attachment.clone())?;
            assert!(
                attachment_path(directory.path(), &attachment)
                    .unwrap()
                    .exists()
            );
        }
    }
    claims.shutdown().await;
    Ok(())
}

#[tokio::test]
async fn guest_dispatch_uses_only_pending_guest_attachments_and_never_client_named_files()
-> Result<()> {
    for pending in [true, false] {
        let h = Harness::new().await?;
        let directory = tempfile::tempdir()?;
        let guest = json!({"type":"image","id":"thread-guest-file","name":"screen.png","mimeType":"image/png","sizeBytes":3});
        let mut thread = t3_contracts::stored::to_value(
            &h.reader
                .thread(ThreadId("thread:test".into()))
                .await?
                .unwrap(),
        )?;
        thread["guestShare"] = json!({"id":"share","host":"host","createdAt":TEST_TIME,"expiresAt":"2027-01-01T00:00:00.000Z","guestName":null,"messages":[{"id":"guest:one","text":"hello","status":if pending{"pending"}else{"sent"},"submittedAt":TEST_TIME,"attachments":[guest]}]});
        h.write("thread.metadata-updated", thread).await?;
        let claims = Arc::new(AttachmentClaimService::new(
            directory.path().into(),
            Arc::new(NoDocumentText),
        ));
        let (intake, dispatcher) = intake(&h, claims.clone(), "ambiguous");
        let command = serde_json::from_value(
            json!({"type":"message.dispatch","commandId":"command:guest","threadId":"thread:test","messageId":"message:guest","guestMessageId":"guest:one","text":"","attachments":[{"type":"image","id":"owner-file","name":"screen.png","mimeType":"image/png","sizeBytes":3}],"dispatchMode":{"type":"queue_after_active"},"createdBy":"user","creationSource":"web"}),
        )?;
        assert!(intake.dispatch_command(command).await.is_err());
        assert_eq!(
            dispatcher.commands.lock().unwrap()[0]["attachments"],
            if pending { json!([guest]) } else { json!([]) }
        );
        claims.shutdown().await;
    }
    Ok(())
}
