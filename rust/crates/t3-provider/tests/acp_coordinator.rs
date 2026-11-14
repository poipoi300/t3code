#![allow(clippy::unwrap_used)]
use std::sync::{
    Arc,
    atomic::{AtomicI64, Ordering},
};
use t3_contracts::{acp_registry::*, provider_instance::ProviderInstanceId};
use t3_provider::{acp_coordinator::*, sensitive::Sensitive, transport::ProviderClock};
struct Clock(AtomicI64);
impl ProviderClock for Clock {
    fn now_ms(&self) -> i64 {
        self.0.load(Ordering::SeqCst)
    }
}
fn coordinator() -> (Arc<AcpRuntimeCoordinator>, Arc<Clock>) {
    let clock = Arc::new(Clock(AtomicI64::new(0)));
    (Arc::new(AcpRuntimeCoordinator::new(clock.clone())), clock)
}
fn action(id: &str) -> Sensitive<AcpRegistryUrlAuthAction> {
    Sensitive::new(AcpRegistryUrlAuthAction {
        elicitation_id: id.into(),
        url: "https://auth.invalid/?secret=never-format".into(),
        message: "Sign in".into(),
        created_at: None,
        expires_at: None,
    })
}
fn accept(id: &str) -> AcpRegistryAcceptUrlAuthInput {
    AcpRegistryAcceptUrlAuthInput {
        instance_id: "acp".into(),
        elicitation_id: id.into(),
    }
}
#[tokio::test]
async fn foreground_startup_cancels_only_same_agent_background_work_and_counts_overlapping_starts()
{
    let (coordinator, _) = coordinator();
    let (entered, ready) = tokio::sync::oneshot::channel();
    let task = tokio::spawn({
        let coordinator = coordinator.clone();
        async move {
            coordinator
                .background_probe("same", async move {
                    let _ = entered.send(());
                    std::future::pending::<Result<(), ()>>().await
                })
                .await
        }
    });
    ready.await.unwrap();
    let first = coordinator.foreground_startup("same");
    let second = coordinator.foreground_startup("same");
    assert_eq!(task.await.unwrap(), Ok(None));
    drop(first);
    assert_eq!(
        coordinator
            .background_probe("same", async { Ok::<_, ()>(1) })
            .await,
        Ok(None)
    );
    assert_eq!(
        coordinator
            .background_probe("other", async { Ok::<_, ()>(2) })
            .await,
        Ok(Some(2))
    );
    drop(second);
    assert_eq!(
        coordinator
            .background_probe("same", async { Ok::<_, ()>(3) })
            .await,
        Ok(Some(3))
    );
}
#[tokio::test(start_paused = true)]
async fn url_auth_replacement_single_accept_expiry_and_cancellation_never_clear_a_newer_request() {
    let (coordinator, clock) = coordinator();
    let instance: ProviderInstanceId = "acp".into();
    let mut actions = coordinator.subscribe_url_auth(instance.clone());
    let old = tokio::spawn({
        let coordinator = coordinator.clone();
        let instance = instance.clone();
        async move { coordinator.request_url_auth(instance, action("old")).await }
    });
    let published = actions.recv().await.unwrap().unwrap();
    assert!(!format!("{published:?}").contains("never-format"));
    assert_eq!(
        published.expose().created_at.as_deref(),
        Some("1970-01-01T00:00:00.000Z")
    );
    let new = tokio::spawn({
        let coordinator = coordinator.clone();
        let instance = instance.clone();
        async move { coordinator.request_url_auth(instance, action("new")).await }
    });
    let published = actions.recv().await.unwrap().unwrap();
    assert_eq!(published.expose().elicitation_id.0, "new");
    assert!(!old.await.unwrap());
    assert!(!coordinator.accept_url_auth(&accept("old")));
    assert!(coordinator.accept_url_auth(&accept("new")));
    assert!(!coordinator.accept_url_auth(&accept("new")));
    assert!(new.await.unwrap());
    assert!(actions.recv().await.unwrap().is_none());
    let expired = tokio::spawn({
        let coordinator = coordinator.clone();
        let instance = instance.clone();
        async move {
            coordinator
                .request_url_auth(instance, action("expired"))
                .await
        }
    });
    actions.recv().await.unwrap();
    clock.0.store(600_000, Ordering::SeqCst);
    assert!(!coordinator.accept_url_auth(&accept("expired")));
    assert!(!expired.await.unwrap());
    assert!(actions.recv().await.unwrap().is_none());
    let cancelled = tokio::spawn({
        let coordinator = coordinator.clone();
        let instance = instance.clone();
        async move {
            coordinator
                .request_url_auth(instance, action("cancelled"))
                .await
        }
    });
    actions.recv().await.unwrap();
    cancelled.abort();
    assert!(cancelled.await.unwrap_err().is_cancelled());
    assert!(actions.recv().await.unwrap().is_none());
    assert!(coordinator.url_auth_action(&instance).is_none());
    let timeout = tokio::spawn({
        let coordinator = coordinator.clone();
        async move {
            coordinator
                .request_url_auth(instance, action("timeout"))
                .await
        }
    });
    actions.recv().await.unwrap();
    assert!(!timeout.await.unwrap());
    assert!(actions.recv().await.unwrap().is_none());
}
#[tokio::test]
async fn command_subscriptions_deliver_every_update_and_clear_does_not_republish_stale_inventory() {
    let (coordinator, _) = coordinator();
    let instance: ProviderInstanceId = "acp".into();
    let commands = AvailableCommands {
        slash_commands: vec![],
        skills: vec![],
    };
    coordinator.publish_commands(instance.clone(), commands.clone());
    let mut subscription = coordinator.subscribe_commands(instance.clone());
    assert_eq!(*subscription.recv().await.unwrap().unwrap(), commands);
    coordinator.clear_commands(&instance);
    assert!(coordinator.commands(&instance).is_none());
    let mut after_clear = coordinator.subscribe_commands(instance.clone());
    assert!(matches!(
        after_clear.try_recv(),
        Err(tokio::sync::mpsc::error::TryRecvError::Empty)
    ));
    coordinator.publish_commands(instance.clone(), commands.clone());
    coordinator.publish_commands(instance, commands);
    assert!(subscription.recv().await.unwrap().is_some());
    assert!(subscription.recv().await.unwrap().is_some());
    assert!(after_clear.recv().await.unwrap().is_some());
}
