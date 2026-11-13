use super::provider_event_ingestor_test::fixture;
use async_trait::async_trait;
use serde_json::json;
use std::{
    collections::HashSet,
    sync::{Arc, Mutex},
};
use t3_orchestration::{
    api::*,
    error::{Error, Result},
    ports::runs_ingestion::context_handoff_delivery::*,
};

pub(crate) struct Budget;
impl HandoffBudget for Budget {
    fn cost(&self, messages: &[OrchestrationV2HistoricalMessage], context: &str) -> u64 {
        (context.len()
            + messages
                .iter()
                .map(|message| message.text.len())
                .sum::<usize>()) as u64
    }
    fn select(
        &self,
        messages: Vec<OrchestrationV2HistoricalMessage>,
        coverage: String,
        omitted_items: u64,
        _: Vec<TurnItemId>,
        _: u64,
    ) -> Result<SelectedHistory> {
        Ok(SelectedHistory {
            messages,
            context: coverage,
            omitted_items,
            omitted_item_ids: vec![],
        })
    }
    fn render(&self, messages: &[OrchestrationV2HistoricalMessage], context: &str) -> String {
        format!(
            "{context}\n{}",
            messages
                .iter()
                .map(|message| message.text.as_str())
                .collect::<Vec<_>>()
                .join("\n")
        )
    }
    fn historical_message(
        &self,
        _: &OrchestrationV2TurnItem,
    ) -> Option<OrchestrationV2HistoricalMessage> {
        None
    }
    fn coverage(
        &self,
        _: &ThreadId,
        _: &ContextHandoffCoveredRunOrdinals,
        _: &[OrchestrationV2TurnItem],
    ) -> String {
        "coverage".into()
    }
}
#[derive(Default)]
struct Persist(Mutex<Vec<OrchestrationV2ContextHandoff>>);
#[async_trait]
impl HandoffPersistence for Persist {
    async fn persist(&self, handoff: OrchestrationV2ContextHandoff) -> Result<()> {
        self.0.lock().unwrap().push(handoff);
        Ok(())
    }
}
struct Inject {
    fail: bool,
    accepted: bool,
    persist: Arc<Persist>,
    calls: Mutex<usize>,
}
#[async_trait]
impl HistoryInjection for Inject {
    async fn inject(&self, messages: &[OrchestrationV2HistoricalMessage], _: &str) -> Result<bool> {
        *self.calls.lock().unwrap() += 1;
        assert_eq!(messages.len(), 1);
        assert_eq!(
            self.persist
                .0
                .lock()
                .unwrap()
                .last()
                .unwrap()
                .delivery
                .as_ref()
                .unwrap()
                .as_ref()
                .unwrap()
                .status,
            ContextHandoffDeliveryStatus::Pending
        );
        if self.fail {
            Err(Error::Cancelled)
        } else {
            Ok(self.accepted)
        }
    }
}
fn records() -> (OrchestrationV2ContextHandoff, OrchestrationV2ProviderThread) {
    let mut handoff = fixture("OrchestrationV2ContextHandoff");
    handoff["history"] = json!({"coverage":"source coverage", "messages":[{"itemId":"item:one","threadId":"thread:source","runId":"run:source","providerThreadId":null,"role":"user","text":"Partial work 日本語 🧪","status":"completed","kind":"user_message"}],"omittedItems":0});
    handoff.as_object_mut().unwrap().remove("delivery");
    let mut provider = fixture("OrchestrationV2ProviderThread");
    provider["nativeThreadRef"] =
        json!({"driver":"codex","strength":"strong","nativeId":"native:target"});
    (
        serde_json::from_value(handoff).unwrap(),
        serde_json::from_value(provider).unwrap(),
    )
}
#[tokio::test]
async fn injection_is_marked_pending_before_send_and_ambiguous_retry_requires_replacement()
-> Result<()> {
    let (handoff, provider) = records();
    let persist = Arc::new(Persist::default());
    let inject = Inject {
        fail: true,
        accepted: false,
        persist: persist.clone(),
        calls: Mutex::new(0),
    };
    let handoffs = [handoff];
    let seen = HashSet::new();
    let input = DeliveryInput {
        handoffs: &handoffs,
        provider_thread: &provider,
        budget: 100_000,
        defer_inline: false,
        already_delivered: &seen,
        inject: Some(&inject),
    };
    assert!(matches!(
        deliver_context_handoffs(input, &Budget, persist.clone()).await,
        Err(Error::Cancelled)
    ));
    let pending = persist.0.lock().unwrap().clone();
    let result = deliver_context_handoffs(
        DeliveryInput {
            handoffs: &pending,
            provider_thread: &provider,
            budget: 100_000,
            defer_inline: false,
            already_delivered: &seen,
            inject: Some(&inject),
        },
        &Budget,
        persist.clone(),
    )
    .await;
    assert!(result.err().unwrap().to_string().contains("uncertain"));
    assert_eq!(*inject.calls.lock().unwrap(), 1);
    Ok(())
}
#[tokio::test]
async fn inline_delivery_deduplicates_history_and_unsent_restores_original_records() -> Result<()> {
    let (handoff, provider) = records();
    let mut second = handoff.clone();
    second.id = serde_json::from_value(json!("handoff:second"))?;
    let handoffs = [handoff.clone(), second];
    let seen = HashSet::new();
    let persist = Arc::new(Persist::default());
    let delivery = deliver_context_handoffs(
        DeliveryInput {
            handoffs: &handoffs,
            provider_thread: &provider,
            budget: 100_000,
            defer_inline: false,
            already_delivered: &seen,
            inject: None,
        },
        &Budget,
        persist.clone(),
    )
    .await?;
    assert_eq!(delivery.context.matches("Partial work").count(), 1);
    delivery.delivered().await?;
    assert!(persist.0.lock().unwrap().iter().skip(2).all(|handoff| {
        handoff.delivery.as_ref().unwrap().as_ref().unwrap().status
            == ContextHandoffDeliveryStatus::Inline
    }));
    delivery.unsent().await?;
    assert_eq!(persist.0.lock().unwrap()[4], handoff);
    assert_eq!(
        with_inline_context("context", "question"),
        "context\n\nUser message:\nquestion"
    );
    Ok(())
}
#[tokio::test]
async fn compaction_defers_inline_and_restores_uninjected_history() -> Result<()> {
    let (handoff, provider) = records();
    let persist = Arc::new(Persist::default());
    let seen = HashSet::new();
    let handoffs = [handoff.clone()];
    let inject = Inject {
        fail: false,
        accepted: false,
        persist: persist.clone(),
        calls: Mutex::new(0),
    };
    let delivery = deliver_context_handoffs(
        DeliveryInput {
            handoffs: &handoffs,
            provider_thread: &provider,
            budget: 100_000,
            defer_inline: true,
            already_delivered: &seen,
            inject: Some(&inject),
        },
        &Budget,
        persist.clone(),
    )
    .await?;
    assert_eq!(delivery.context, "");
    assert_eq!(persist.0.lock().unwrap().last(), Some(&handoff));
    Ok(())
}
