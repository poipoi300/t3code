use crate::{
    api::*,
    error::{Error, Result},
};
use async_trait::async_trait;
use std::{collections::HashSet, sync::Arc};
use t3_contracts::orchestration_v2_id_table::IdOrIndex;

pub struct SelectedHistory {
    pub messages: Vec<OrchestrationV2HistoricalMessage>,
    pub context: String,
    pub omitted_items: u64,
    pub omitted_item_ids: Vec<TurnItemId>,
}
// seam-shim: provider-core handoffBudget has no Rust host implementation yet.
pub trait HandoffBudget: Send + Sync {
    fn cost(&self, messages: &[OrchestrationV2HistoricalMessage], context: &str) -> u64;
    fn select(
        &self,
        messages: Vec<OrchestrationV2HistoricalMessage>,
        coverage: String,
        omitted_items: u64,
        omitted_item_ids: Vec<TurnItemId>,
        budget: u64,
    ) -> Result<SelectedHistory>;
    fn render(&self, messages: &[OrchestrationV2HistoricalMessage], context: &str) -> String;
    fn historical_message(
        &self,
        item: &OrchestrationV2TurnItem,
    ) -> Option<OrchestrationV2HistoricalMessage>;
    fn coverage(
        &self,
        thread_id: &ThreadId,
        range: &ContextHandoffCoveredRunOrdinals,
        items: &[OrchestrationV2TurnItem],
    ) -> String;
}
#[async_trait]
pub trait HandoffPersistence: Send + Sync {
    async fn persist(&self, handoff: OrchestrationV2ContextHandoff) -> Result<()>;
}
#[async_trait]
pub trait HistoryInjection: Send + Sync {
    async fn inject(
        &self,
        messages: &[OrchestrationV2HistoricalMessage],
        context: &str,
    ) -> Result<bool>;
}
pub struct DeliveryInput<'a> {
    pub handoffs: &'a [OrchestrationV2ContextHandoff],
    pub provider_thread: &'a OrchestrationV2ProviderThread,
    pub budget: u64,
    pub defer_inline: bool,
    pub already_delivered: &'a HashSet<TurnItemId>,
    pub inject: Option<&'a dyn HistoryInjection>,
}
pub struct PreparedDelivery {
    pub context: String,
    original: Vec<OrchestrationV2ContextHandoff>,
    inline: Vec<OrchestrationV2ContextHandoff>,
    persistence: Arc<dyn HandoffPersistence>,
}
impl PreparedDelivery {
    pub async fn delivered(&self) -> Result<()> {
        for handoff in &self.inline {
            self.persistence.persist(handoff.clone()).await?;
        }
        Ok(())
    }
    pub async fn unsent(&self) -> Result<()> {
        for handoff in &self.original {
            self.persistence.persist(handoff.clone()).await?;
        }
        Ok(())
    }
}
pub fn with_inline_context(context: &str, user_text: &str) -> String {
    if context.is_empty() {
        user_text.into()
    } else {
        format!("{context}\n\nUser message:\n{user_text}")
    }
}
fn failure(detail: &str) -> Error {
    Error::Service {
        operation: "ContextHandoffDelivery",
        detail: detail.into(),
    }
}
fn id_string(id: &IdOrIndex<ThreadId>) -> String {
    match id {
        IdOrIndex::Id(id) => id.0.clone(),
        IdOrIndex::Index(index) => index.to_string(),
    }
}
pub async fn deliver_context_handoffs(
    input: DeliveryInput<'_>,
    budget: &dyn HandoffBudget,
    persistence: Arc<dyn HandoffPersistence>,
) -> Result<PreparedDelivery> {
    let empty = || PreparedDelivery {
        context: String::new(),
        original: vec![],
        inline: vec![],
        persistence: persistence.clone(),
    };
    let native = input
        .provider_thread
        .native_thread_ref
        .as_ref()
        .and_then(|reference| reference.native_id.as_ref())
        .map(|id| id.as_str());
    let pending: Vec<_> = input
        .handoffs
        .iter()
        .filter(|handoff| {
            let delivery = handoff.delivery.as_ref().and_then(Option::as_ref);
            native.is_none()
                || delivery.is_none_or(|delivery| {
                    Some(delivery.native_thread_id.as_str()) != native
                        || delivery.status == ContextHandoffDeliveryStatus::Pending
                })
        })
        .cloned()
        .collect();
    if pending.is_empty() || (input.defer_inline && input.inject.is_none()) {
        return Ok(empty());
    }
    let mut coverage = pending.iter().map(|handoff| {
        let strategy = t3_contracts::stored::to_value(&handoff.strategy).unwrap_or_default();
        let strategy = if handoff.strategy == ContextHandoffStrategy::ForkDeltaSummary { "merge_back / fork_delta_summary" } else { strategy.as_str().unwrap_or_default() };
        let text = handoff.history.as_ref().and_then(Option::as_ref).map(|history| history.coverage.clone()).unwrap_or_else(|| format!("From thread {}, runs {}-{}. Recover history with t3_thread_read, view=activity, afterPosition=-1; paginate with afterPosition=nextPosition, and use itemId/textOffset for long items.", id_string(&handoff.thread_id), handoff.covered_run_ordinals.r#from, handoff.covered_run_ordinals.to));
        format!("Context handoff ({strategy}):\n{text}")
    }).collect::<Vec<_>>().join("\n");
    if budget.cost(&[], &coverage) > 4_000.min(input.budget / 2) {
        let mut strategies = vec![];
        for handoff in &pending {
            let strategy = t3_contracts::stored::to_value(&handoff.strategy)?;
            let strategy = strategy.as_str().unwrap_or_default().to_owned();
            if !strategies.contains(&strategy) {
                strategies.push(strategy);
            }
        }
        let thread = input
            .provider_thread
            .app_thread_id
            .as_ref()
            .unwrap_or(&pending[0].thread_id);
        coverage = format!(
            "Context handoff ({}). {} handoff records; detailed coverage references omitted. Recover an omitted item only when the request needs it, with t3_thread_read({{threadId:\"{}\",view:\"activity\",itemId:\"<omitted item ID>\",maxCharsPerItem:4000}}); continue long text with textOffset=nextTextOffset until null. Follow fork/handoff source references in activity. Items included below need no read.",
            strategies.join(", "),
            pending.len(),
            id_string(thread)
        );
    }
    let mut seen = input.already_delivered.clone();
    let messages = pending
        .iter()
        .filter_map(|handoff| handoff.history.as_ref().and_then(Option::as_ref))
        .flat_map(|history| &history.messages)
        .filter(|message| seen.insert(message.item_id.clone()))
        .cloned()
        .collect();
    let old = pending
        .iter()
        .filter(|handoff| handoff.history.as_ref().and_then(Option::as_ref).is_none())
        .map(|handoff| handoff.summary_text.as_str())
        .collect::<Vec<_>>()
        .join("\n\n");
    if !old.is_empty()
        && budget
            .cost(&[], &format!("{coverage}\n{old}"))
            .saturating_add(512)
            <= input.budget
    {
        coverage.push('\n');
        coverage.push_str(&old);
    }
    let omitted = pending
        .iter()
        .filter_map(|handoff| handoff.history.as_ref().and_then(Option::as_ref))
        .map(|history| history.omitted_items)
        .sum();
    let selected = budget.select(
        messages,
        coverage,
        omitted,
        pending
            .iter()
            .filter_map(|handoff| handoff.history.as_ref().and_then(Option::as_ref))
            .flat_map(|history| {
                history
                    .omitted_item_ids
                    .clone()
                    .flatten()
                    .unwrap_or_default()
            })
            .collect(),
        input.budget,
    )?;
    if budget.cost(&selected.messages, &selected.context) > input.budget {
        if input.defer_inline {
            return Ok(empty());
        }
        return Err(failure("ContextHandoffBudgetError"));
    }
    let decorate = |status: ContextHandoffDeliveryStatus| {
        pending
            .iter()
            .map(|handoff| {
                let mut handoff = handoff.clone();
                if let Some(native) = native {
                    let history = handoff.history.as_ref().and_then(Option::as_ref);
                    let mut omitted = history
                        .and_then(|history| history.omitted_item_ids.clone().flatten())
                        .unwrap_or_default();
                    omitted.extend(
                        history
                            .into_iter()
                            .flat_map(|history| &history.messages)
                            .filter(|message| selected.omitted_item_ids.contains(&message.item_id))
                            .map(|message| message.item_id.clone()),
                    );
                    handoff.delivery = Some(Some(ContextHandoffDelivery {
                        native_thread_id: native.into(),
                        status: status.clone(),
                        omitted_item_ids: Some(Some(omitted)),
                        item_ids: selected
                            .messages
                            .iter()
                            .filter(|message| {
                                history.is_some_and(|history| {
                                    history
                                        .messages
                                        .iter()
                                        .any(|candidate| candidate.item_id == message.item_id)
                                })
                            })
                            .map(|message| message.item_id.clone())
                            .collect(),
                    }));
                }
                handoff
            })
            .collect::<Vec<_>>()
    };
    if input.inject.is_some()
        && native.is_some()
        && pending.iter().any(|handoff| {
            handoff
                .delivery
                .as_ref()
                .and_then(Option::as_ref)
                .is_some_and(|delivery| {
                    Some(delivery.native_thread_id.as_str()) == native
                        && delivery.status == ContextHandoffDeliveryStatus::Pending
                })
        })
    {
        return Err(failure(
            "Historical context delivery is uncertain; replace the native thread before retrying.",
        ));
    }
    for handoff in decorate(ContextHandoffDeliveryStatus::Pending) {
        persistence.persist(handoff).await?;
    }
    if let Some(inject) = input.inject.filter(|_| native.is_some())
        && inject.inject(&selected.messages, &selected.context).await?
    {
        for handoff in decorate(ContextHandoffDeliveryStatus::Injected) {
            persistence.persist(handoff).await?;
        }
        return Ok(empty());
    }
    if input.defer_inline {
        for handoff in pending {
            persistence.persist(handoff).await?;
        }
        return Ok(empty());
    }
    Ok(PreparedDelivery {
        context: budget.render(&selected.messages, &selected.context),
        inline: decorate(ContextHandoffDeliveryStatus::Inline),
        original: pending,
        persistence,
    })
}
