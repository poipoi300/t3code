use crate::{Error, Result, SharedClock, api::RequestContext, http::Sensitive, peer::PeerLinks};
use async_trait::async_trait;
use serde_json::json;
use std::{
    collections::HashMap,
    sync::{Arc, Mutex, Weak, atomic::Ordering},
};
use t3_contracts::{
    auth::AuthEnvironmentScope,
    base_schemas::{NodeId, RunId, ThreadId, TurnItemId},
    orchestration_v2::{
        InternalCommandLinkRequestRecord as Card, OrchestrationV2LinkRequestStatus as Status,
    },
    orchestrator_mcp::{OrchestratorMcpEnvironmentLinkInput, OrchestratorMcpEnvironmentLinkResult},
    peer_link::{PeerLink, PeerLinkAnswer, PeerLinkCreateInput, PeerLinkRequestAnswerInput},
    provider_instance::ProviderInstanceId,
};

pub struct LinkCaller {
    pub thread_id: ThreadId,
    pub provider_instance_id: ProviderInstanceId,
    pub namespace: String,
    pub linked_origin: bool,
}
pub struct ActiveParent {
    pub run_id: RunId,
    pub node_id: NodeId,
    pub provider_instance_id: ProviderInstanceId,
}
#[async_trait]
pub trait LinkRequestStore: Send + Sync {
    /// Return only an undeleted thread's card with expanded IDs and a non-null run/node.
    async fn card(&self, thread: &ThreadId, id: &TurnItemId) -> Result<Option<Card>>;
    async fn active_parent(&self, thread: &ThreadId) -> Result<Option<ActiveParent>>;
    /// Dispatch link_request.record; the decider must keep a concurrently cancelled card closed.
    async fn record(&self, command: Card) -> Result<()>;
}
#[derive(Default)]
pub(crate) struct KeyedLocks(Mutex<HashMap<String, Weak<tokio::sync::Mutex<()>>>>);
impl KeyedLocks {
    pub(crate) fn get(&self, key: &str) -> Arc<tokio::sync::Mutex<()>> {
        let mut locks = self.0.lock().unwrap_or_else(|error| error.into_inner());
        locks.retain(|_, lock| lock.strong_count() > 0);
        if let Some(lock) = locks.get(key).and_then(Weak::upgrade) {
            return lock;
        }
        let lock = Arc::new(tokio::sync::Mutex::new(()));
        locks.insert(key.into(), Arc::downgrade(&lock));
        lock
    }
}
pub struct PeerLinkRequests {
    links: Arc<PeerLinks>,
    store: Arc<dyn LinkRequestStore>,
    clock: SharedClock,
    locks: KeyedLocks,
}
impl PeerLinkRequests {
    pub fn new(
        links: Arc<PeerLinks>,
        store: Arc<dyn LinkRequestStore>,
        clock: SharedClock,
    ) -> Arc<Self> {
        Arc::new(Self {
            links,
            store,
            clock,
            locks: KeyedLocks::default(),
        })
    }
    async fn usable(&self, id: &str) -> Result<Option<PeerLink>> {
        Ok(self.links.get(id).await?.filter(|link| {
            chrono::DateTime::parse_from_rfc3339(&link.expires_at.0)
                .is_ok_and(|date| date.timestamp_millis() > self.clock.now_ms())
        }))
    }
    pub async fn request(
        &self,
        caller: &LinkCaller,
        input: OrchestratorMcpEnvironmentLinkInput,
    ) -> Result<OrchestratorMcpEnvironmentLinkResult> {
        if caller.linked_origin {
            return Err(Error::Identity);
        }
        if !self.links.enabled.load(Ordering::Relaxed) {
            return Err(Error::Disabled);
        }
        let environment = input.environment_id.clone().flatten();
        let url = input.url.clone().flatten();
        if environment.is_some() && url.is_some() {
            return Err(Error::Configuration);
        }
        let key = input
            .client_request_id
            .clone()
            .flatten()
            .map(|v| v.0.0)
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
        let id = card_id(&caller.thread_id.0, &caller.namespace, &key);
        let lock = self.locks.get(&id.0);
        let _lock = lock.lock().await;
        if let Some(environment) = &environment
            && let Some(link) = self.usable(&environment.0).await?
        {
            if let Some(mut card) = self.store.card(&caller.thread_id, &id).await?
                && card.link_status == Status::Pending
            {
                set_linked(&mut card, &link);
                card.command_id =
                    format!("link-request:{}:linked-on-retry", card.turn_item_id.0).into();
                card.wake = None;
                self.store.record(card).await?;
            }
            return linked_result(&link);
        }
        if input.client_request_id.flatten().is_some()
            && let Some(card) = self.store.card(&caller.thread_id, &id).await?
        {
            return result_of(&card);
        }
        let parent = self
            .store
            .active_parent(&caller.thread_id)
            .await?
            .filter(|parent| parent.provider_instance_id == caller.provider_instance_id)
            .ok_or(Error::PeerFailure {
                code: t3_contracts::orchestrator_mcp::OrchestratorMcpFailureCode::ParentNotActive,
            })?;
        let card: Card = crate::decode(
            json!({"commandId":format!("command:mcp:{}:link-pending:{}",component(&caller.namespace),component(&key)),"threadId":caller.thread_id,"runId":parent.run_id,"nodeId":parent.node_id,"turnItemId":id,"reason":input.reason.flatten().map(|v|v.0).unwrap_or_default(),"linkStatus":"pending"}),
        )?;
        let card = Card {
            url: input.url,
            environment_id: input.environment_id,
            hint: input.hint.map(|v| v.map(|v| v.0)),
            requested_access: input.requested_access,
            ..card
        };
        self.store.record(card).await?;
        crate::decode(json!({"status":"pending","turnItemId":id}))
    }
    pub async fn answer(
        self: &Arc<Self>,
        context: &RequestContext,
        input: Sensitive<PeerLinkRequestAnswerInput>,
    ) -> Result<()> {
        context.require(AuthEnvironmentScope::AccessWrite)?;
        let service = self.clone();
        tokio::spawn(async move { service.answer_inner(input.into_inner()).await })
            .await
            .map_err(|_| Error::Storage)?
    }
    async fn answer_inner(&self, input: PeerLinkRequestAnswerInput) -> Result<()> {
        let lock = self.locks.get(&input.turn_item_id.0);
        let _lock = lock.lock().await;
        let mut card = self
            .store
            .card(&input.thread_id, &input.turn_item_id)
            .await?
            .ok_or(Error::NotLinked)?;
        if card.link_status != Status::Pending {
            return Err(Error::Superseded);
        }
        let legacy = card.url.as_ref().and_then(Option::as_ref).is_some()
            && card
                .environment_id
                .as_ref()
                .and_then(Option::as_ref)
                .is_some();
        let outcome = match input.answer {
            PeerLinkAnswer::PeerLinkAnswerDecline(_) => {
                card.link_status = Status::Declined;
                None
            }
            PeerLinkAnswer::PeerLinkAnswerUseExisting(answer) => {
                match self.usable(&answer.environment_id.0).await {
                    Ok(Some(link)) => Some(Ok(link)),
                    Ok(None) => Some(Err(Error::NotLinked)),
                    Err(error) => Some(Err(error)),
                }
            }
            PeerLinkAnswer::PeerLinkAnswerLink(answer) => {
                if legacy && answer.environment_id.is_some() {
                    card.link_status = Status::Failed;
                    card.failure = Some(Some(
                        "This request is from before an update. Ask for the link again.".into(),
                    ));
                    None
                } else if let Some((url, alternates)) = answer.urls.split_first() {
                    Some(
                        self.links
                            .link(Sensitive::new(PeerLinkCreateInput {
                                url: url.clone(),
                                alternate_urls: (!alternates.is_empty())
                                    .then(|| alternates.to_vec()),
                                pairing_code: answer.pairing_code,
                                access: answer.access,
                                expected_environment_id: answer.environment_id,
                                expected_label: answer.label,
                            }))
                            .await,
                    )
                } else {
                    Some(Err(Error::Configuration))
                }
            }
        };
        if let Some(outcome) = outcome {
            match outcome {
                Ok(link) => set_linked(&mut card, &link),
                Err(error) => {
                    card.link_status = Status::Failed;
                    card.failure = Some(Some(error.to_string()));
                }
            }
        }
        let target = card
            .url
            .clone()
            .flatten()
            .map(|v| v.0)
            .or_else(|| card.environment_id.clone().flatten().map(|v| v.0))
            .unwrap_or_else(|| "a machine".into());
        let (text, outcome, summary) = match card.link_status {
            Status::Linked => {
                let label = card.linked_label.clone().flatten().unwrap_or(target);
                let access = crate::encode(&card.linked_access.flatten())?;
                (
                    format!(
                        "The user linked {label} with {} access. Pass environmentId {} to act there.",
                        access.as_str().unwrap_or("unknown"),
                        card.linked_environment_id
                            .clone()
                            .flatten()
                            .map(|v| v.0)
                            .unwrap_or_default()
                    ),
                    "completed",
                    format!("Linked {label}"),
                )
            }
            Status::Failed => (
                format!(
                    "Linking {target} failed: {}. Call t3_environment_link again if the user still wants it.",
                    card.failure
                        .clone()
                        .flatten()
                        .unwrap_or_else(|| "the other environment refused".into())
                ),
                "failed",
                format!("Could not link {target}"),
            ),
            _ => (
                format!("The user declined to link {target}."),
                "cancelled",
                format!("Declined to link {target}"),
            ),
        };
        let status = crate::encode(&card.link_status)?;
        card.command_id = format!(
            "link-request:{}:{}",
            card.turn_item_id.0,
            status.as_str().ok_or(Error::Protocol)?
        )
        .into();
        card.wake = Some(Some(crate::decode(
            json!({"messageId":format!("message:link-request:{}",card.turn_item_id.0),"text":text,"notification":{"source":{"kind":"background_task"},"outcome":outcome,"summary":summary}}),
        )?));
        self.store.record(card).await
    }
}
fn set_linked(card: &mut Card, link: &PeerLink) {
    card.link_status = Status::Linked;
    card.linked_environment_id = Some(Some(link.environment_id.clone()));
    card.linked_label = Some(Some(link.label.clone()));
    card.linked_access = Some(Some(link.access));
    card.failure = None;
}
fn linked_result(link: &PeerLink) -> Result<OrchestratorMcpEnvironmentLinkResult> {
    crate::decode(
        json!({"status":"linked","environmentId":link.environment_id,"label":link.label,"access":link.access}),
    )
}
fn result_of(card: &Card) -> Result<OrchestratorMcpEnvironmentLinkResult> {
    crate::decode(match card.link_status {
        Status::Pending => json!({"status":"pending","turnItemId":card.turn_item_id}),
        Status::Linked => {
            json!({"status":"linked","environmentId":card.linked_environment_id,"label":card.linked_label.clone().flatten().unwrap_or_else(||"a machine".into()),"access":card.linked_access})
        }
        Status::Failed => {
            json!({"status":"failed","message":card.failure.clone().flatten().unwrap_or_else(||"The link could not be made.".into())})
        }
        Status::Declined => json!({"status":"declined"}),
        Status::Cancelled => json!({"status":"cancelled"}),
    })
}
pub(crate) fn component(value: &str) -> String {
    const SET: &percent_encoding::AsciiSet = &percent_encoding::NON_ALPHANUMERIC
        .remove(b'-')
        .remove(b'_')
        .remove(b'.')
        .remove(b'!')
        .remove(b'~')
        .remove(b'*')
        .remove(b'\'')
        .remove(b'(')
        .remove(b')');
    percent_encoding::utf8_percent_encode(value, SET).to_string()
}
pub fn card_id(thread: &str, namespace: &str, key: &str) -> TurnItemId {
    format!(
        "turn-item:link-request:{}:{}:{}",
        component(thread),
        component(namespace),
        component(key)
    )
    .into()
}
