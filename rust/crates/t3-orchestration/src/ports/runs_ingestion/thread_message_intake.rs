use super::attachment_claims::{attachment_id, attachment_path, validate_attachment_limits};
use crate::{
    api::*,
    error::{Error, Result},
    ports::{
        checkpoints_forks::thread_launch_service::{
            LaunchInput, LaunchResult, ThreadLaunchService,
        },
        thread_project::thread_management_service::{
            SendInput, SendResult, ThreadManagementService,
        },
    },
    stores::receipts::CommandReceipt,
};
use async_trait::async_trait;
use serde_json::{Value, json};
use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
    sync::Arc,
};
use t3_contracts::chat_attachment::ChatAttachment;

#[derive(Clone)]
pub struct ThreadMessageIntake {
    pub threads: ThreadManagementService,
    pub launches: Option<ThreadLaunchService>,
    pub attachments: Arc<dyn AttachmentBackend>,
    pub projections: Arc<dyn ProjectionReader>,
}

pub struct AttachmentReference {
    pub path: PathBuf,
    pub extracted_path: Option<PathBuf>,
}
// seam-shim: move onto AttachmentBackend::attachment_reference when the host path seam lands.
#[async_trait]
pub trait AttachmentReferenceResolver: Send + Sync {
    async fn reference(&self, attachment: &ChatAttachment) -> Result<AttachmentReference>;
}
fn not_accepted(error: &Error) -> bool {
    // seam-shim: owner08 preserves this pre-acceptance tag until Error has a typed restore variant.
    matches!(
        error,
        Error::Rejected { .. }
            | Error::ThreadNotFound(_)
            | Error::Service {
                operation: "ThreadManagementWorktreeRestoreError",
                ..
            }
    )
}
fn intake_error(detail: impl Into<String>) -> Error {
    Error::Service {
        operation: "ThreadMessageIntake",
        detail: detail.into(),
    }
}

struct PreparedClaims {
    backend: Arc<dyn AttachmentBackend>,
    paths: Vec<PathBuf>,
}

impl PreparedClaims {
    fn take(&mut self) -> Vec<PathBuf> {
        std::mem::take(&mut self.paths)
    }
}
impl Drop for PreparedClaims {
    fn drop(&mut self) {
        if !self.paths.is_empty() {
            let paths = self.take();
            let backend = self.backend.clone();
            tokio::spawn(async move {
                let _ = backend.release_claimed_paths(paths).await;
            });
        }
    }
}

// Once dispatch may accept the command, only its outcome can decide whether to roll back.
// Drive dispatch and reconciliation to completion even if the RPC receiver goes away.
async fn finish_dispatch<T: Send + 'static>(
    has_copies: bool,
    work: impl std::future::Future<Output = Result<T>> + Send + 'static,
) -> Result<T> {
    if !has_copies {
        return work.await;
    }
    tokio::spawn(work)
        .await
        .map_err(|e| intake_error(e.to_string()))?
}

async fn claim(
    backend: &dyn AttachmentBackend,
    thread: ThreadId,
    attachments: Vec<ChatAttachment>,
) -> Result<ClaimedAttachments> {
    validate_attachment_limits(&attachments)?;
    let mut ids = HashSet::new();
    if attachments
        .iter()
        .any(|attachment| !ids.insert(attachment_id(attachment)))
    {
        return Err(intake_error("Duplicate attachment ids are not allowed."));
    }
    if attachments.is_empty() {
        return Ok(ClaimedAttachments {
            attachments,
            claimed_paths: vec![],
        });
    }
    backend.claim_pending(thread, attachments).await
}
fn remap_context(value: &mut Value, before: &[ChatAttachment], after: &[ChatAttachment]) {
    let ids: HashMap<_, _> = before
        .iter()
        .zip(after)
        .map(|(before, after)| (attachment_id(before), attachment_id(after)))
        .collect();
    if let Some(records) = value
        .get_mut("context")
        .and_then(|context| context.get_mut("records"))
        .and_then(Value::as_array_mut)
    {
        for record in records {
            if matches!(record["kind"].as_str(), Some("image" | "file"))
                && let Some(id) = record["attachmentId"].as_str().and_then(|id| ids.get(id))
            {
                record["attachmentId"] = json!(id);
            }
        }
    }
}
fn attachment_name(attachment: &ChatAttachment) -> &str {
    match attachment {
        ChatAttachment::Image(value) => &value.name,
        ChatAttachment::File(value) => &value.name,
        ChatAttachment::Unknown(value) => &value.name,
    }
}

impl ThreadMessageIntake {
    pub async fn dispatch_command(
        &self,
        command: OrchestrationV2ServerCommand,
    ) -> Result<CommandReceipt> {
        self.dispatch_command_with_resolver(command, None).await
    }
    pub async fn dispatch_command_with_resolver(
        &self,
        command: OrchestrationV2ServerCommand,
        resolver: Option<&dyn AttachmentReferenceResolver>,
    ) -> Result<CommandReceipt> {
        let mut value = t3_contracts::stored::to_value(&command)?;
        let kind = value["type"].as_str().unwrap_or_default().to_owned();
        if kind == "message.dispatch" {
            self.with_guest_attachments(&mut value).await?;
        }
        if kind == "runtime-request.respond" && value["attachmentsByQuestionId"].is_object() {
            return self.respond_with_attachments(value, resolver).await;
        }
        if kind != "message.dispatch"
            && (kind != "queued-run.edit" || value.get("attachments").is_none())
        {
            return self.threads.dispatch(command).await;
        }
        let before: Vec<ChatAttachment> =
            serde_json::from_value(value.get("attachments").cloned().unwrap_or(json!([])))?;
        let thread: ThreadId = serde_json::from_value(value["threadId"].clone())?;
        let claimed = claim(self.attachments.as_ref(), thread.clone(), before.clone()).await?;
        let mut guard = PreparedClaims {
            backend: self.attachments.clone(),
            paths: claimed.claimed_paths,
        };
        value["attachments"] = t3_contracts::stored::to_value(&claimed.attachments)?;
        remap_context(&mut value, &before, &claimed.attachments);
        let prepared = match serde_json::from_value::<OrchestrationV2ServerCommand>(value.clone()) {
            Ok(command) => command,
            Err(error) => {
                self.release(guard.take()).await;
                return Err(error.into());
            }
        };
        let paths = guard.take();
        let intake = self.clone();
        finish_dispatch(!paths.is_empty(), async move {
            let result = intake.threads.dispatch(prepared).await;
            if result.is_ok() {
                let message_id = if kind == "queued-run.edit" {
                    intake
                        .projections
                        .run(
                            thread.clone(),
                            serde_json::from_value(value["runId"].clone())?,
                        )
                        .await?
                        .map(|run| {
                            super::provider_event_ingestor::expanded_id(&run.user_message_id)
                        })
                        .transpose()?
                } else {
                    Some(serde_json::from_value::<MessageId>(
                        value["messageId"].clone(),
                    )?)
                };
                let records = intake
                    .projections
                    .query_records(ProjectionRecordQuery {
                        thread_id: thread,
                        fields: vec![RecordKind::Messages],
                        filter: RecordFilter::default(),
                        messages: MessageFilter {
                            ids: Some(message_id.into_iter().collect()),
                            ..Default::default()
                        },
                        run_statuses: None,
                        include_latest_run: false,
                    })
                    .await?;
                let accepted = records
                    .records
                    .into_iter()
                    .flat_map(|records| match records {
                        ProjectionRecordSet::Messages(messages) => messages
                            .into_iter()
                            .flat_map(|message| message.attachments)
                            .collect(),
                        _ => vec![],
                    })
                    .collect::<Vec<_>>();
                intake.release_unused(paths, &accepted).await;
            } else if result.as_ref().err().is_some_and(not_accepted) {
                intake.release(paths).await;
            }
            result
        })
        .await
    }
    async fn respond_with_attachments(
        &self,
        mut value: Value,
        resolver: Option<&dyn AttachmentReferenceResolver>,
    ) -> Result<CommandReceipt> {
        let thread: ThreadId = serde_json::from_value(value["threadId"].clone())?;
        let incoming: serde_json::Map<String, Value> =
            serde_json::from_value(value["attachmentsByQuestionId"].clone())?;
        let all = incoming
            .values()
            .map(|attachments| serde_json::from_value::<Vec<ChatAttachment>>(attachments.clone()))
            .collect::<std::result::Result<Vec<_>, _>>()?
            .into_iter()
            .flatten()
            .collect::<Vec<_>>();
        validate_attachment_limits(&all)?;
        let mut guard = PreparedClaims {
            backend: self.attachments.clone(),
            paths: vec![],
        };
        let prepared = async {
            let mut questions = serde_json::Map::new();
            for (question, attachments) in incoming {
                let claimed = claim(
                    self.attachments.as_ref(),
                    thread.clone(),
                    serde_json::from_value(attachments)?,
                )
                .await?;
                guard.paths.extend(claimed.claimed_paths);
                questions.insert(
                    question,
                    t3_contracts::stored::to_value(&claimed.attachments)?,
                );
            }
            let mut answers = value["answers"].as_object().cloned().unwrap_or_default();
            for (question, attachments) in &questions {
                let attachments: Vec<ChatAttachment> = serde_json::from_value(attachments.clone())?;
                let mut references = Vec::new();
                for attachment in attachments {
                    let reference = resolver
                        .ok_or(Error::Unsupported("AttachmentBackend.attachment_reference"))?
                        .reference(&attachment)
                        .await?;
                    let suffix = match reference.extracted_path {
                        Some(path) => format!(
                            " (text extracted to: {})",
                            serde_json::to_string(&path.to_string_lossy())?
                        ),
                        None => String::new(),
                    };
                    references.push(format!(
                        "Attached {} {}: {}{suffix}",
                        match &attachment {
                            ChatAttachment::Image(_) => "image",
                            ChatAttachment::File(_) => "file",
                            ChatAttachment::Unknown(value) => &value.r#type.0,
                        },
                        serde_json::to_string(attachment_name(&attachment))?,
                        serde_json::to_string(&reference.path.to_string_lossy())?
                    ));
                }
                if references.is_empty() {
                    continue;
                }
                let references = references.join("\n");
                match answers.get_mut(question) {
                    Some(Value::Array(values)) => values.push(references.into()),
                    Some(Value::String(text)) if !text.is_empty() => {
                        text.push_str("\n\n");
                        text.push_str(&references);
                    }
                    _ => {
                        answers.insert(question.clone(), references.into());
                    }
                }
            }
            value["answers"] = Value::Object(answers);
            value["attachmentsByQuestionId"] = Value::Object(questions);
            Ok::<_, Error>(serde_json::from_value::<OrchestrationV2ServerCommand>(
                value.clone(),
            )?)
        }
        .await;
        let command = match prepared {
            Ok(command) => command,
            Err(error) => {
                self.release(guard.take()).await;
                return Err(error);
            }
        };
        let paths = guard.take();
        let intake = self.clone();
        finish_dispatch(!paths.is_empty(), async move {
            let result = intake.threads.dispatch(command).await;
            if result.is_ok() {
                let request: RuntimeRequestId = serde_json::from_value(value["requestId"].clone())?;
                let records = intake
                    .projections
                    .records(
                        thread,
                        vec![RecordKind::TurnItems],
                        RecordFilter {
                            turn_item_request_ids: Some(vec![request]),
                            ..Default::default()
                        },
                    )
                    .await?;
                let mut resolved = false;
                let mut accepted = Vec::new();
                for set in records.records {
                    if let ProjectionRecordSet::TurnItems(items) = set {
                        for item in items {
                            if let OrchestrationV2TurnItem::UserInputRequest(item) = item {
                                if item.status == OrchestrationV2TurnItemStatus::Pending {
                                    continue;
                                }
                                resolved = true;
                                let item = t3_contracts::stored::to_value(&item)?;
                                if let Some(questions) =
                                    item["questionAnswer"]["attachmentsByQuestionId"].as_object()
                                {
                                    for attachments in questions.values() {
                                        accepted.extend(serde_json::from_value::<
                                            Vec<ChatAttachment>,
                                        >(
                                            attachments.clone()
                                        )?);
                                    }
                                }
                            }
                        }
                    }
                }
                if resolved {
                    intake.release_unused(paths, &accepted).await;
                }
            } else if result.as_ref().err().is_some_and(not_accepted) {
                intake.release(paths).await;
            }
            result
        })
        .await
    }
    pub async fn send_to_thread(&self, mut input: SendInput) -> Result<SendResult> {
        let claimed = claim(
            self.attachments.as_ref(),
            input.thread_id.clone(),
            input.attachments,
        )
        .await?;
        input.attachments = claimed.attachments;
        if claimed.claimed_paths.is_empty() {
            return self.threads.send_to_thread(input).await;
        }
        let intake = self.clone();
        finish_dispatch(true, async move {
            let result = intake.threads.send_to_thread(input).await;
            if let Ok(result) = &result {
                intake
                    .release_unused(claimed.claimed_paths, &result.message.attachments)
                    .await;
            } else if result.as_ref().err().is_some_and(not_accepted) {
                intake.release(claimed.claimed_paths).await;
            }
            result
        })
        .await
    }
    pub async fn launch_thread(&self, mut input: LaunchInput) -> Result<LaunchResult> {
        let launches = self
            .launches
            .as_ref()
            .ok_or(Error::Unsupported("ThreadMessageIntake.launch_thread"))?;
        let Some(message) = input.initial_message.as_mut() else {
            return launches.launch(input).await;
        };
        validate_attachment_limits(&message.attachments)?;
        if message.attachments.is_empty() {
            return launches.launch(input).await;
        }
        let thread = input
            .thread_id
            .clone()
            .ok_or_else(|| intake_error("Attachments need a thread id at launch."))?;
        let before = message.attachments.clone();
        let claimed = claim(self.attachments.as_ref(), thread, before.clone()).await?;
        let mut guard = PreparedClaims {
            backend: self.attachments.clone(),
            paths: claimed.claimed_paths,
        };
        message.attachments = claimed.attachments;
        if let Some(context) = &message.context {
            let mut value = json!({"context":t3_contracts::stored::to_value(context)?});
            remap_context(&mut value, &before, &message.attachments);
            message.context = Some(serde_json::from_value(value["context"].clone())?);
        }
        let paths = guard.take();
        let intake = self.clone();
        let launches = launches.clone();
        finish_dispatch(!paths.is_empty(), async move {
            let result = launches.launch(input).await;
            if let Ok(result) = &result {
                intake
                    .release_unused(
                        paths,
                        &result
                            .projection
                            .messages
                            .iter()
                            .flat_map(|message| message.attachments.clone())
                            .collect::<Vec<_>>(),
                    )
                    .await;
            } else if result.as_ref().err().is_some_and(not_accepted) {
                intake.release(paths).await;
            }
            result
        })
        .await
    }
    async fn release(&self, paths: Vec<PathBuf>) {
        if !paths.is_empty() {
            let backend = self.attachments.clone();
            let _ = tokio::spawn(async move { backend.release_claimed_paths(paths).await }).await;
        }
    }
    async fn release_unused(&self, paths: Vec<PathBuf>, accepted: &[ChatAttachment]) {
        let Some(directory) = paths.first().and_then(|path| path.parent()) else {
            return;
        };
        let keep = accepted
            .iter()
            .filter_map(|attachment| attachment_path(directory, attachment))
            .flat_map(|path| [PathBuf::from(format!("{}.txt", path.display())), path])
            .collect::<HashSet<_>>();
        self.release(
            paths
                .into_iter()
                .filter(|path| !keep.contains(path))
                .collect(),
        )
        .await;
    }
    async fn with_guest_attachments(&self, value: &mut Value) -> Result<()> {
        let Some(guest) = value["guestMessageId"].as_str() else {
            return Ok(());
        };
        let thread: ThreadId = serde_json::from_value(value["threadId"].clone())?;
        let thread = t3_contracts::stored::to_value(
            &self
                .projections
                .thread(thread.clone())
                .await?
                .ok_or(Error::ThreadNotFound(thread))?,
        )?;
        value["attachments"] = thread["guestShare"]["messages"]
            .as_array()
            .and_then(|messages| {
                messages
                    .iter()
                    .find(|message| message["id"] == guest && message["status"] == "pending")
            })
            .and_then(|message| {
                message
                    .get("attachments")
                    .filter(|value| !value.is_null())
                    .cloned()
            })
            .unwrap_or(json!([]));
        Ok(())
    }
}
