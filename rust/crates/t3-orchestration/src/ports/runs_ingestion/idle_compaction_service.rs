use super::{
    idle_compaction::{latest_run, window_value},
    native_child_work_query::records_value,
    provider_event_ingestor::epoch_millis,
};
use crate::{
    api::*,
    error::{Error, Result},
};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::Duration,
};
use t3_contracts::application_event::ApplicationStoredEvent;
use tokio_util::{sync::CancellationToken, task::TaskTracker};

pub struct IdleCompactionService {
    pub projections: Arc<dyn ProjectionReader>,
    pub settings: Arc<dyn SettingsService>,
    pub dispatcher: Arc<dyn CommandDispatcher>,
    pub clock: Arc<dyn Clock>,
}
pub fn eligible(settings: &Value, thread: &Value) -> bool {
    let Some(instance) = thread["providerInstanceId"].as_str() else {
        return false;
    };
    let config = &settings["providerInstances"][instance];
    let claude = if config.is_null() {
        instance == "claudeAgent"
    } else {
        config["driver"] == "claudeAgent"
    };
    let enabled = settings["projectSettingsOverrides"]
        [thread["projectId"].as_str().unwrap_or_default()]["providerInstances"][instance]["enabled"]
        .as_bool()
        .unwrap_or_else(|| {
            if config.is_null() {
                instance == "claudeAgent"
            } else {
                config["enabled"] != false && config["config"]["enabled"] != false
            }
        });
    let config = serde_json::from_value::<t3_contracts::settings::ClaudeSettings>(
        config["config"]
            .as_object()
            .map(|map| Value::Object(map.clone()))
            .unwrap_or(json!({})),
    )
    .ok()
    .and_then(|config| t3_contracts::stored::to_value(&config).ok());
    let override_mode = config
        .as_ref()
        .and_then(|config| config["idleCompaction"].as_str())
        .unwrap_or("default");
    let global = settings["idleCompaction"]
        .as_str()
        .or_else(|| settings["compactIdleThreads"].as_str())
        .unwrap_or(if settings["compactIdleThreads"] == true {
            "beforeCacheExpires"
        } else {
            "onReturn"
        });
    claude
        && enabled
        && (if override_mode == "default" {
            global
        } else {
            override_mode
        }) == "beforeCacheExpires"
}
impl IdleCompactionService {
    pub async fn compact_when_due(
        &self,
        thread_id: ThreadId,
        mut run_id: RunId,
        cancellation: CancellationToken,
    ) -> Result<()> {
        loop {
            if cancellation.is_cancelled() {
                return Err(Error::Cancelled);
            }
            let settings = t3_contracts::stored::to_value(&self.settings.get().await?)?;
            let thread = self
                .projections
                .thread(thread_id.clone())
                .await?
                .ok_or_else(|| Error::ThreadNotFound(thread_id.clone()))?;
            if !eligible(&settings, &t3_contracts::stored::to_value(&thread)?) {
                return Ok(());
            }
            let active = thread
                .active_provider_thread_id
                .as_ref()
                .map(super::provider_event_ingestor::expanded_id)
                .transpose()?
                .into_iter()
                .collect::<Vec<_>>();
            let records = records_value(
                self.projections
                    .query_records(ProjectionRecordQuery {
                        thread_id: thread_id.clone(),
                        fields: vec![
                            RecordKind::Runs,
                            RecordKind::Messages,
                            RecordKind::ProviderThreads,
                            RecordKind::ProviderTurns,
                            RecordKind::RuntimeRequests,
                            RecordKind::TurnItems,
                            RecordKind::ContextTransfers,
                        ],
                        filter: RecordFilter {
                            provider_thread_ids: Some(active.clone()),
                            provider_turn_thread_ids: Some(active),
                            latest_provider_turn_usage_only: true,
                            runtime_request_statuses: Some(vec![RuntimeRequestStatus::Pending]),
                            context_transfer_statuses: Some(vec![ContextTransferStatus::Pending]),
                            turn_item_statuses: Some(vec![
                                OrchestrationV2TurnItemStatus::Pending,
                                OrchestrationV2TurnItemStatus::Running,
                                OrchestrationV2TurnItemStatus::Waiting,
                            ]),
                            ..Default::default()
                        },
                        messages: MessageFilter {
                            run_ids: Some(vec![run_id.clone()]),
                            ..Default::default()
                        },
                        run_statuses: Some(vec![
                            OrchestrationV2RunStatus::Preparing,
                            OrchestrationV2RunStatus::Starting,
                            OrchestrationV2RunStatus::Running,
                            OrchestrationV2RunStatus::RolledBack,
                        ]),
                        include_latest_run: true,
                    })
                    .await?,
            )?;
            if !eligible(&settings, &records["thread"]) {
                return Ok(());
            }
            if let Some(latest) = latest_run(&records)
                .filter(|run| run["status"] == "completed" && run["id"] != json!(run_id))
            {
                run_id = serde_json::from_value(latest["id"].clone())?;
                continue;
            }
            let now = epoch_millis(&self.clock.now().0).ok_or_else(|| Error::Service {
                operation: "IdleCompactionService",
                detail: "Invalid clock".into(),
            })?;
            let Some(window) =
                window_value(&records, now)?.filter(|window| window.run_id == run_id)
            else {
                return Ok(());
            };
            if now < window.start_ms {
                tokio::select! {biased;_=cancellation.cancelled()=>return Err(Error::Cancelled),_=tokio::time::sleep(Duration::from_millis((window.start_ms-now) as u64))=>{}};
                continue;
            }
            let key = format!("{}:{}", thread_id.0, run_id.0);
            self.dispatcher.dispatch(serde_json::from_value(json!({"type":"message.dispatch","commandId":format!("server:idle-compact:{key}"),"messageId":format!("idle-compact:{key}"),"threadId":thread_id,"text":"/compact","attachments":[],"createdBy":"agent","creationSource":"server","idleCompaction":"beforeCacheExpires","dispatchMode":{"type":"start_immediately"}}))?).await?;
            return Ok(());
        }
    }
    pub fn start(
        self: Arc<Self>,
        events: &dyn EventCommitter,
        cancellation: CancellationToken,
    ) -> tokio::task::JoinHandle<()> {
        let mut events = events.subscribe();
        tokio::spawn(async move {
            let timers = Arc::new(Mutex::new(
                HashMap::<ThreadId, (u64, CancellationToken)>::new(),
            ));
            let tasks = TaskTracker::new();
            let mut generation = 0;
            loop {
                tokio::select! {biased;_=cancellation.cancelled()=>break,event=events.recv()=>match event {
                    Ok(ApplicationStoredEvent::Orchestration(event))=>{if let OrchestrationV2DomainEvent::RunUpdated(event)=event.event && event.payload.status==OrchestrationV2RunStatus::Completed {
                        let thread=event.thread_id;let run=match super::provider_event_ingestor::expanded_id(&event.payload.id){Ok(id)=>id,Err(_)=>continue};let token=cancellation.child_token();
                        generation+=1;let current=generation;
                        if let Some((_,previous))=timers.lock().unwrap_or_else(|error| error.into_inner()).insert(thread.clone(),(current,token.clone())){previous.cancel();}
                        let service=self.clone();let timers=timers.clone();tasks.spawn(async move {tokio::select!{biased;_=token.cancelled()=>{},_=service.compact_when_due(thread.clone(),run,token.clone())=>{}};let mut timers=timers.lock().unwrap_or_else(|error| error.into_inner());if timers.get(&thread).is_some_and(|(generation,_)|*generation==current){timers.remove(&thread);}});
                    }},Ok(_)=>{},Err(_)=>break,
                }}
            }
            for (_, timer) in timers
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .values()
            {
                timer.cancel();
            }
            tasks.close();
            tasks.wait().await;
        })
    }
}
