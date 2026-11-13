use crate::{
    api::*,
    error::{Error, Result},
    ports::runs_ingestion::notification_mailbox::is_undelivered_mailbox_steer,
    stores::receipts::CommandReceipt,
};
use async_trait::async_trait;
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
    time::Duration,
};
use t3_contracts::{orchestration_v2_id_table::IdOrIndex, provider_instance::ProviderDriverKind};
use tokio::{sync::mpsc, task::JoinSet};
use tokio_util::{sync::CancellationToken, task::TaskTracker};

#[derive(Clone)]
pub struct DelegatedContinuation {
    pub parent_run_id: RunId,
    pub generation: u64,
    pub message_id: MessageId,
}

#[derive(Clone, Copy, Default)]
pub enum ContinuationDelivery {
    #[default]
    AdapterBuffered,
    MessageText,
}

#[async_trait]
pub trait ContinuationCallbacks: Send + Sync {
    async fn dispatch_if_current(
        &self,
        dispatcher: &dyn CommandDispatcher,
        command: OrchestrationV2ServerCommand,
    ) -> Result<Option<CommandReceipt>> {
        dispatcher.dispatch(command).await.map(Some)
    }
    async fn clear_if_current(&self) -> Result<()>;
    fn watch_unstarted_run(&self) -> bool {
        false
    }
    async fn on_run_ended_without_starting(&self) -> Result<()> {
        Ok(())
    }
}

#[derive(Clone)]
pub struct ContinuationRequest {
    pub thread_id: ThreadId,
    pub provider_thread_id: ProviderThreadId,
    pub driver: ProviderDriverKind,
    pub detail: Option<String>,
    pub notification: Option<OrchestrationV2Notification>,
    pub delegated_completion: Option<DelegatedContinuation>,
    pub delivery: ContinuationDelivery,
    pub message_id: Option<MessageId>,
    pub callbacks: Option<Arc<dyn ContinuationCallbacks>>,
}

impl ContinuationRequest {
    fn retry_key(&self) -> Option<String> {
        self.delegated_completion.as_ref().map(|completion| {
            format!(
                "{}:{}:{}:{}",
                self.thread_id.0,
                completion.parent_run_id.0,
                completion.generation,
                completion.message_id.0
            )
        })
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum ContinuationOutcome {
    Settled,
    RetryAfter(Duration),
}

pub fn current_delivery(
    messages: &[OrchestrationV2ConversationMessage],
    runs: &[OrchestrationV2Run],
    turns: &[OrchestrationV2ProviderTurn],
    completion: &DelegatedContinuation,
) -> Result<Option<Value>> {
    let Some(run) = runs
        .iter()
        .find(|run| run.id == IdOrIndex::Id(completion.parent_run_id.clone()))
    else {
        return Ok(None);
    };
    let cohort = t3_contracts::stored::to_value(&run.delegated_completion)?;
    let delivery = &cohort["delivery"];
    let dispatched = messages
        .iter()
        .any(|message| message.id == IdOrIndex::Id(completion.message_id.clone()));
    if cohort["disposition"] != "open"
        || delivery["generation"] != completion.generation
        || delivery["messageId"] != json!(completion.message_id)
        || (dispatched
            && !is_undelivered_mailbox_steer(messages, runs, turns, &completion.message_id))
    {
        return Ok(None);
    }
    Ok(Some(delivery.clone()))
}

pub struct ProviderContinuationService {
    reader: Arc<dyn ProjectionReader>,
    dispatcher: Arc<dyn CommandDispatcher>,
    sink: Arc<dyn EventCommitter>,
    ids: Arc<dyn IdGenerator>,
    retry_attempts: Mutex<BTreeMap<String, u32>>,
    cancellation: CancellationToken,
    watchers: TaskTracker,
}

impl ProviderContinuationService {
    pub fn new(
        reader: Arc<dyn ProjectionReader>,
        dispatcher: Arc<dyn CommandDispatcher>,
        sink: Arc<dyn EventCommitter>,
        ids: Arc<dyn IdGenerator>,
    ) -> Self {
        Self {
            reader,
            dispatcher,
            sink,
            ids,
            retry_attempts: Mutex::new(BTreeMap::new()),
            cancellation: CancellationToken::new(),
            watchers: TaskTracker::new(),
        }
    }

    pub async fn process(&self, request: &ContinuationRequest) -> Result<ContinuationOutcome> {
        if self.cancellation.is_cancelled() {
            return Err(Error::Cancelled);
        }
        match self.dispatch(request).await {
            Ok(()) => {
                self.clear_retry(request);
                Ok(ContinuationOutcome::Settled)
            }
            Err(error) if request.delegated_completion.is_some() => {
                let key = request
                    .retry_key()
                    .ok_or(Error::Unsupported("delegated retry identity"))?;
                let mut attempts = self
                    .retry_attempts
                    .lock()
                    .unwrap_or_else(|error| error.into_inner());
                let attempt = attempts.entry(key).or_default();
                let delay = Duration::from_millis((100 * 2u64.pow((*attempt).min(6))).min(5_000));
                *attempt = attempt.saturating_add(1);
                let _ = error;
                Ok(ContinuationOutcome::RetryAfter(delay))
            }
            Err(error) => {
                if let Some(callbacks) = &request.callbacks {
                    callbacks.clear_if_current().await?;
                }
                Err(error)
            }
        }
    }

    fn clear_retry(&self, request: &ContinuationRequest) {
        if let Some(key) = request.retry_key() {
            self.retry_attempts
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .remove(&key);
        }
    }

    async fn records(&self, request: &ContinuationRequest) -> Result<ProjectionRecords> {
        self.reader
            .query_records(ProjectionRecordQuery {
                thread_id: request.thread_id.clone(),
                fields: vec![
                    RecordKind::Messages,
                    RecordKind::Runs,
                    RecordKind::ProviderTurns,
                ],
                filter: RecordFilter::default(),
                messages: MessageFilter {
                    ids: Some(
                        request
                            .delegated_completion
                            .as_ref()
                            .map(|completion| vec![completion.message_id.clone()])
                            .unwrap_or_default(),
                    ),
                    ..Default::default()
                },
                run_statuses: None,
                include_latest_run: false,
            })
            .await
    }

    async fn dispatch(&self, request: &ContinuationRequest) -> Result<()> {
        let records = self.records(request).await?;
        if records.thread.archived_at.is_some() || records.thread.deleted_at.is_some() {
            if let Some(callbacks) = &request.callbacks {
                callbacks.clear_if_current().await?;
            }
            return Ok(());
        }
        let (messages, runs, turns) = split_records(records);
        if let Some(completion) = &request.delegated_completion {
            let Some(delivery) = current_delivery(&messages, &runs, &turns, completion)? else {
                return Ok(());
            };
            let task_ids = delivery["taskIds"]
                .as_array()
                .ok_or_else(|| Error::Decode {
                    entity: "delegated delivery",
                    detail: "taskIds is not an array".into(),
                })?;
            let task_list = task_ids
                .iter()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>()
                .join(", ");
            let text = if task_ids.len() == 1 {
                format!(
                    "Delegated task {task_list} reached a terminal state. Use task_status with taskId {task_list} to read the result."
                )
            } else {
                format!(
                    "Delegated tasks {task_list} reached terminal states. Use task_status with each taskId to read the results."
                )
            };
            let command = serde_json::from_value(
                json!({"type":"message.dispatch","commandId":self.ids.next(IdKind::Command),"threadId":request.thread_id,"messageId":delivery["messageId"],"text":text,"attachments":[],"dispatchMode":{"type":"queue_after_active"},"createdBy":"agent","creationSource":"server","delegatedCompletion":{"parentRunId":completion.parent_run_id,"generation":delivery["generation"],"taskIds":task_ids}}),
            )?;
            match self.dispatcher.dispatch(command).await {
                Err(Error::Service {
                    operation: "ThreadManagementProviderInstanceDisabledError",
                    ..
                }) => {
                    for task in task_ids {
                        self.dispatcher.dispatch(serde_json::from_value(json!({"type":"delegated_task.completion-delivery.dispose","commandId":self.ids.next(IdKind::Command),"parentThreadId":request.thread_id,"taskId":task}))?).await?;
                    }
                    if let Some(callbacks) = &request.callbacks {
                        callbacks.clear_if_current().await?;
                    }
                }
                result => {
                    result?;
                }
            }
            return Ok(());
        }
        let message = request
            .message_id
            .clone()
            .unwrap_or_else(|| MessageId(self.ids.next(IdKind::Message)));
        let command_id = CommandId(format!("provider-continuation:{}", message.0));
        let command = serde_json::from_value(
            json!({"type":"message.dispatch","commandId":command_id,"threadId":request.thread_id,"messageId":message,"text":request.detail.as_deref().unwrap_or("Background task completed."),
            "notification":request.notification.as_ref().map(t3_contracts::stored::to_value).transpose()?.unwrap_or_else(|| json!({"source":{"kind":"background_task"},"outcome":"updated","summary":"Background activity updated"})),
            "attachments":[],"dispatchMode":{"type":"queue_after_active"},"createdBy":"agent","creationSource":if matches!(request.delivery, ContinuationDelivery::MessageText) { "server" } else { "provider" }}),
        )?;
        // Subscribe before dispatch so a quickly cancelled queued run cannot escape the watch.
        let subscription = self.sink.subscribe();
        let dispatched = match &request.callbacks {
            Some(callbacks) => {
                callbacks
                    .dispatch_if_current(self.dispatcher.as_ref(), command)
                    .await
            }
            None => self.dispatcher.dispatch(command).await.map(Some),
        };
        let dispatched = match dispatched {
            Err(Error::Service {
                operation: "ThreadManagementProviderInstanceDisabledError",
                ..
            }) => {
                if let Some(callbacks) = &request.callbacks {
                    callbacks.clear_if_current().await?;
                }
                return Ok(());
            }
            result => result?,
        };
        if let Some(receipt) = dispatched
            && let Some(callbacks) = request
                .callbacks
                .as_ref()
                .filter(|callbacks| callbacks.watch_unstarted_run())
        {
            let created = self
                .sink
                .events_by_command(receipt.command_id)
                .await?
                .into_iter()
                .find_map(|event| match event.event {
                    OrchestrationV2DomainEvent::RunCreated(event)
                        if event.payload.user_message_id == IdOrIndex::Id(message.clone()) =>
                    {
                        Some(event.payload.id)
                    }
                    _ => None,
                });
            if let Some(IdOrIndex::Id(run_id)) = created {
                let reader = self.reader.clone();
                let thread = request.thread_id.clone();
                let callbacks = callbacks.clone();
                let cancellation = self.cancellation.clone();
                self.watchers.spawn(async move {
                    let mut subscription = subscription;
                    loop {
                        match reader.run(thread.clone(), run_id.clone()).await {
                            Ok(Some(run)) if terminal(&run.status) => { if run.started_at.is_none() { let _ = callbacks.on_run_ended_without_starting().await; } return; }
                            Ok(_) => {}, Err(_) => return,
                        }
                        tokio::select! { _ = cancellation.cancelled() => return, event = subscription.recv() => match event { Ok(_) | Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}, Err(_) => return } }
                    }
                });
            }
        }
        Ok(())
    }

    pub async fn run(&self, mut requests: mpsc::Receiver<ContinuationRequest>) -> Result<()> {
        let mut retries = JoinSet::new();
        loop {
            let request = tokio::select! {
                _ = self.cancellation.cancelled() => break,
                request = requests.recv() => match request { Some(request) => request, None if retries.is_empty() => break, None => { if let Some(result) = retries.join_next().await { result.map_err(|error| Error::Service { operation: "ProviderContinuation.retry", detail: error.to_string() })? } else { break } } },
                request = retries.join_next(), if !retries.is_empty() => match request { Some(result) => result.map_err(|error| Error::Service { operation: "ProviderContinuation.retry", detail: error.to_string() })?, None => continue },
            };
            if let Ok(ContinuationOutcome::RetryAfter(delay)) = self.process(&request).await {
                let cancellation = self.cancellation.clone();
                retries.spawn(async move { tokio::select! { _ = cancellation.cancelled() => {}, _ = tokio::time::sleep(delay) => {} } request });
            }
        }
        retries.shutdown().await;
        Ok(())
    }

    pub async fn shutdown(&self) {
        self.cancellation.cancel();
        self.watchers.close();
        self.watchers.wait().await;
    }
}

fn split_records(
    records: ProjectionRecords,
) -> (
    Vec<OrchestrationV2ConversationMessage>,
    Vec<OrchestrationV2Run>,
    Vec<OrchestrationV2ProviderTurn>,
) {
    let (mut messages, mut runs, mut turns) = (Vec::new(), Vec::new(), Vec::new());
    for records in records.records {
        match records {
            ProjectionRecordSet::Messages(value) => messages = value,
            ProjectionRecordSet::Runs(value) => runs = value,
            ProjectionRecordSet::ProviderTurns(value) => turns = value,
            _ => {}
        }
    }
    (messages, runs, turns)
}

fn terminal(status: &OrchestrationV2RunStatus) -> bool {
    matches!(
        status,
        OrchestrationV2RunStatus::Completed
            | OrchestrationV2RunStatus::Failed
            | OrchestrationV2RunStatus::Interrupted
            | OrchestrationV2RunStatus::Cancelled
            | OrchestrationV2RunStatus::RolledBack
    )
}
