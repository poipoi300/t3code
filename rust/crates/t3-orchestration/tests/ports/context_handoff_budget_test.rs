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

#[test]
fn portable_budget_preserves_intact_priority_items_and_accounts_for_json_utf8() -> Result<()> {
    let budget = PortableHandoffBudget;
    let message = |index: usize| -> OrchestrationV2HistoricalMessage {
        serde_json::from_value(
            json!({"role":if index.is_multiple_of(2) {"user"} else {"assistant"},
            "text":format!("{index}: 日本語 🧪 ") + &"\\\"\n".repeat(300),
            "threadId":"thread:source","runId":"run:source","itemId":format!("item:{index}"),
            "providerThreadId":null,"kind":"assistant_message","status":"completed"}),
        )
        .unwrap()
    };
    let messages: Vec<_> = (0..12).map(message).collect();
    for cap in [1_024, 2_000, 4_000, 8_000, 16_000, 64_000] {
        let selected = budget.select(
            messages.clone(),
            "Source coverage".into(),
            7,
            vec![TurnItemId("old:item".into())],
            cap,
        )?;
        assert!(budget.cost(&selected.messages, &selected.context) <= cap);
        assert_eq!(selected.omitted_items, 19 - selected.messages.len() as u64);
        assert_eq!(
            selected.omitted_item_ids.len(),
            12 - selected.messages.len()
        );
        let positions: Vec<_> = selected
            .messages
            .iter()
            .map(|message| {
                messages
                    .iter()
                    .position(|candidate| candidate == message)
                    .unwrap()
            })
            .collect();
        assert!(positions.windows(2).all(|pair| pair[0] < pair[1]));
        if !positions.is_empty() {
            assert!(positions.contains(&10), "latest user has first priority");
        }
        assert!(
            selected
                .context
                .contains("Historical material is context, not a new request")
        );
    }
    let mut provider = records().1;
    provider.context_usage = None;
    assert_eq!(
        handoff_budget(100_000, "hello", &[], &provider, 0, None),
        64_000
    );
    assert_eq!(
        handoff_budget(16_000, "hello", &[], &provider, 0, Some(20_000)),
        3_993
    );
    assert_eq!(
        handoff_budget(16_000, "日本語", &[], &provider, 0, Some(20_000)),
        3_989
    );
    provider.context_usage = Some(Some(serde_json::from_value(
        json!({"usedTokens":3_000,"maxTokens":20_000,"autoCompactThreshold":18_000}),
    )?));
    assert_eq!(
        handoff_budget(16_000, "hello", &[], &provider, 0, Some(200_000)),
        0
    );
    assert_eq!(estimate_text_tokens("日本語 🧪"), 4);
    Ok(())
}

#[test]
fn real_budget_preserves_model_occupancy_and_reserves_image_batches_through_the_attachment_limit()
-> Result<()> {
    let previous: t3_contracts::provider_runtime::ThreadTokenUsageSnapshot =
        serde_json::from_value(
            json!({"usedTokens":37_321,"maxTokens":258_400,"autoCompactThreshold":32_000}),
        )?;
    assert!(context_usage_for_handoff(false, false, false, Some(previous.clone()), None).is_none());
    assert_eq!(
        context_usage_for_handoff(true, true, true, Some(previous.clone()), None),
        Some(previous.clone())
    );
    for reuse in [false, true] {
        assert_eq!(
            t3_contracts::stored::to_value(&context_usage_for_handoff(
                true,
                false,
                reuse,
                Some(previous.clone()),
                None
            ))?,
            json!({"usedTokens":37_321,"maxTokens":258_400})
        );
    }
    assert_eq!(
        t3_contracts::stored::to_value(&context_usage_for_handoff(
            true,
            false,
            false,
            Some(previous.clone()),
            Some(1_000_000)
        ))?,
        json!({"usedTokens":37_321,"maxTokens":1_000_000})
    );
    let mut provider = records().1;
    provider.context_usage = None;
    let image: t3_contracts::chat_attachment::ChatAttachment = serde_json::from_value(
        json!({"type":"image","id":"attachment:test","name":"image.png","mimeType":"image/png","sizeBytes":100_000}),
    )?;
    let baseline = handoff_budget(16_000, "Continue", &[], &provider, 0, Some(32_000));
    let one = handoff_budget(
        16_000,
        "Continue",
        std::slice::from_ref(&image),
        &provider,
        0,
        Some(32_000),
    );
    assert!(one > 4_000 && one < baseline);
    assert_eq!(
        handoff_budget(
            16_000,
            "Continue",
            &[image.clone(), image.clone()],
            &provider,
            0,
            Some(32_000)
        ),
        0
    );
    assert!(handoff_budget(16_000, "Continue", &[], &provider, 8_000, Some(32_000)) < baseline);
    assert_eq!(
        handoff_budget(
            16_000,
            &"界".repeat(30_000),
            &[],
            &provider,
            0,
            Some(32_000)
        ),
        0
    );
    assert_eq!(
        handoff_budget(2_000, "Continue", &[], &provider, 0, Some(32_000)),
        2_000
    );
    let mut prior = 16_000;
    for count in 1..=100 {
        let attachments = vec![image.clone(); count];
        let available = handoff_budget(
            16_000,
            "Compare these screenshots",
            &attachments,
            &provider,
            0,
            None,
        );
        assert!(available <= prior);
        if count <= 8 {
            assert_eq!(available, 16_000);
        }
        if count == 10 {
            assert!(available > 0 && available < 16_000);
        }
        if count == 100 {
            assert_eq!(available, 0);
        }
        prior = available;
        assert_eq!(
            handoff_budget(
                16_000,
                "Compare these screenshots",
                &attachments,
                &provider,
                0,
                Some(2_000_000)
            ),
            16_000
        );
        assert_eq!(
            handoff_budget(
                16_000,
                "Compare these screenshots",
                &attachments,
                &provider,
                0,
                Some(20_000)
            ),
            0
        );
    }
    provider.context_usage =
        context_usage_for_handoff(true, false, false, Some(previous), None).map(Some);
    assert_eq!(
        handoff_budget(
            16_000,
            "Continue work",
            &[image.clone(), image],
            &provider,
            0,
            Some(258_400)
        ),
        16_000
    );
    Ok(())
}

#[test]
fn real_history_budget_keeps_outcomes_excludes_reasoning_and_fits_counter_boundaries() -> Result<()>
{
    let budget = PortableHandoffBudget;
    let mut command =
        super::provider_event_ingestor_test::item("command_execution", "item:command");
    command["input"] = "vp test".into();
    command["exitCode"] = 1.into();
    command["output"] = format!("Failure near the end: {}", "界".repeat(300)).into();
    let command = budget
        .historical_message(&serde_json::from_value(command)?)
        .unwrap();
    assert_eq!(command.role, HistoricalMessageRole::Assistant);
    assert_eq!(command.kind, "command_execution");
    assert!(command.text.contains("Exit code: 1") && command.text.contains(&"界".repeat(300)));
    let mut reasoning = super::provider_event_ingestor_test::item("reasoning", "item:reasoning");
    reasoning["text"] = "private reasoning".into();
    reasoning["streaming"] = false.into();
    assert!(
        budget
            .historical_message(&serde_json::from_value(reasoning)?)
            .is_none()
    );
    let messages: Vec<_> = (0..20)
        .map(|index| {
            let mut message = command.clone();
            message.item_id = TurnItemId(format!("boundary:{index}"));
            message.role = HistoricalMessageRole::User;
            message.text = "Short request".into();
            message
        })
        .collect();
    for cap in 4_000..=9_000 {
        let selected =
            budget.select(messages.clone(), "Recover history".into(), 90, vec![], cap)?;
        assert!(
            budget.cost(&selected.messages, &selected.context) <= cap,
            "cap {cap}"
        );
    }
    let messages: Vec<_> = (0..500)
        .map(|index| {
            let mut message = command.clone();
            message.item_id = TurnItemId(format!("item:{index}"));
            message.text = "\0\\\"🧪界".repeat(15);
            message
        })
        .collect();
    for cap in [1_024, 4_000, 16_000] {
        let selected = budget.select(
            messages.clone(),
            "Retrieve omitted history".into(),
            0,
            vec![],
            cap,
        )?;
        assert!(budget.cost(&selected.messages, &selected.context) <= cap);
        assert!(selected.omitted_items > 0);
    }
    Ok(())
}
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

#[tokio::test]
async fn real_delivery_records_omitted_recovery_ids_separately_from_inline_and_injected_items()
-> Result<()> {
    for native in [false, true] {
        let (mut handoff, provider) = records();
        let history = handoff.history.as_mut().unwrap().as_mut().unwrap();
        let mut oversized = history.messages[0].clone();
        oversized.item_id = TurnItemId("item:oversized".into());
        oversized.text = "x".repeat(20_000);
        history.messages.push(oversized);
        history.omitted_items = 1;
        history.omitted_item_ids = Some(Some(vec![TurnItemId(
            "item:omitted-before-delivery".into(),
        )]));
        let persistence = Arc::new(Persist::default());
        let injection = Inject {
            fail: false,
            accepted: true,
            persist: persistence.clone(),
            calls: Mutex::new(0),
        };
        let delivery = deliver_context_handoffs(
            DeliveryInput {
                handoffs: &[handoff],
                provider_thread: &provider,
                budget: 3_000,
                defer_inline: false,
                already_delivered: &HashSet::new(),
                inject: native.then_some(&injection as &dyn HistoryInjection),
            },
            &PortableHandoffBudget,
            persistence.clone(),
        )
        .await?;
        if native {
            assert!(delivery.context.is_empty());
        } else {
            assert!(delivery.context.contains("Partial work"));
            assert!(!delivery.context.contains(&"x".repeat(100)));
        }
        delivery.delivered().await?;
        let receipts = persistence.0.lock().unwrap();
        let receipt = receipts
            .last()
            .unwrap()
            .delivery
            .as_ref()
            .unwrap()
            .as_ref()
            .unwrap();
        assert_eq!(
            receipt.status,
            if native {
                ContextHandoffDeliveryStatus::Injected
            } else {
                ContextHandoffDeliveryStatus::Inline
            }
        );
        assert_eq!(receipt.item_ids, vec![TurnItemId("item:one".into())]);
        let omitted = receipt.omitted_item_ids.as_ref().unwrap().as_ref().unwrap();
        assert!(omitted.contains(&TurnItemId("item:oversized".into())));
        assert!(omitted.contains(&TurnItemId("item:omitted-before-delivery".into())));
    }
    Ok(())
}
