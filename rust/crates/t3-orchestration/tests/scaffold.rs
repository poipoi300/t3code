use t3_contracts::{
    orchestration_v2::*,
    provider_instance::{ProviderDriverKind, ProviderInstanceId},
};
use t3_orchestration::{error::Result, provider::*, testing::*};
#[tokio::test]
async fn fake_provider_delivers_ordered_typed_events_and_records_close() -> Result<()> {
    let session: OrchestrationV2ProviderSession = serde_json::from_str(
        include_str!("../../../fixtures/samples/OrchestrationV2ProviderSession.jsonl")
            .lines()
            .next()
            .expect("fixture"),
    )?;
    let thread: OrchestrationV2ProviderThread = serde_json::from_str(
        include_str!("../../../fixtures/samples/OrchestrationV2ProviderThread.jsonl")
            .lines()
            .next()
            .expect("fixture"),
    )?;
    let capabilities = session.capabilities.clone();
    let snapshot = ThreadSnapshot {
        provider_thread: thread,
        provider_turns: vec![],
        messages: vec![],
        reasoning_message_ids: None,
        runtime_requests: vec![],
        provider_payload: None,
    };
    let driver: ProviderDriverKind = serde_json::from_str("\"codex\"")?;
    let (adapter, mut calls) = FakeProviderAdapter::new(
        ProviderInstanceId("codex".into()),
        driver.clone(),
        capabilities,
        session,
        snapshot,
    );
    let mut subscription = adapter.runtime.subscribe().await?;
    assert!(matches!(calls.recv().await, Some(FakeCall::Subscribe)));
    for text in ["one", "two"] {
        let event = ProviderEvent {
            driver: driver.clone(),
            kind: ProviderEventKind::CommandOutput {
                thread_id: t3_contracts::ThreadId("thread".into()),
                item_id: t3_contracts::base_schemas::TurnItemId("item".into()),
                chunk: text.into(),
            },
        };
        adapter.runtime.emit(event.clone()).await?;
        assert_eq!(subscription.events.recv().await.expect("event")?, event);
    }
    adapter.runtime.close().await?;
    assert!(matches!(calls.recv().await, Some(FakeCall::Close)));
    assert!(subscription.events.recv().await.is_none());
    assert!(adapter.runtime.subscribe().await.is_err());
    Ok(())
}
