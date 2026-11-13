#![allow(clippy::unwrap_used)]
use async_trait::async_trait;
use std::{collections::BTreeSet, sync::Arc};
use t3_contracts::{
    base_schemas::{EnvironmentId, ProviderSessionId, ThreadId},
    provider_instance::ProviderInstanceId,
};
use t3_mcp::{provider_sessions::*, *};
use tokio::sync::{Semaphore, mpsc};

fn request() -> CredentialRequest {
    CredentialRequest::new(
        ThreadId("thread".into()),
        ProviderInstanceId("codex".into()),
    )
}
fn runtime(name: &str) -> ProviderSessionId {
    ProviderSessionId(name.into())
}
fn raw(config: &ProviderSessionConfig) -> &str {
    config.authorization_header.strip_prefix("Bearer ").unwrap()
}
fn setup() -> (Arc<SessionRegistry>, ProviderCredentials) {
    let registry = Arc::new(SessionRegistry::new(EnvironmentId("env".into()), None));
    let manager = ProviderCredentials::new(registry.clone());
    (registry, manager)
}

#[tokio::test]
async fn credentials_reuse_across_detach_and_rotate_on_provider_access_and_lineage_changes() {
    let (registry, manager) = setup();
    let first = manager.prepare(request()).await.unwrap();
    assert!(first.issued);
    let first = first.publish(runtime("one")).unwrap();
    let id = first
        .scope
        .thread
        .as_ref()
        .unwrap()
        .provider_session_id
        .clone();
    manager
        .detach(&runtime("one"), &ThreadId("thread".into()), &id, false)
        .unwrap();
    let same = manager.prepare(request()).await.unwrap();
    assert!(!same.issued);
    assert_eq!(same.config.authorization_header, first.authorization_header);
    same.publish(runtime("two")).unwrap();
    manager.release_runtime(&runtime("one")).unwrap();
    assert!(registry.resolve(raw(&first)).is_some());
    let mut changed = request();
    changed.capabilities = Some(BTreeSet::from([Capability::Device]));
    let rotated = manager.prepare(changed).await.unwrap();
    assert!(rotated.issued);
    let rotated = rotated.publish(runtime("three")).unwrap();
    assert!(registry.resolve(raw(&first)).is_none());
    assert!(registry.resolve(raw(&rotated)).is_some());
    manager.release_runtime(&runtime("two")).unwrap();
    assert!(registry.resolve(raw(&rotated)).is_some());
    let mut changed = request();
    changed.subagent = true;
    changed.provider_instance_id = ProviderInstanceId("claude".into());
    let newest = manager.prepare(changed).await.unwrap();
    assert!(newest.issued);
    let newest = newest.publish(runtime("four")).unwrap();
    assert!(registry.resolve(raw(&rotated)).is_none());
    assert!(
        registry
            .resolve(raw(&newest))
            .unwrap()
            .thread
            .unwrap()
            .subagent
    );
    manager.disable(&ThreadId("thread".into())).await.unwrap();
    assert!(manager.read(&ThreadId("thread".into())).unwrap().is_none());
    assert!(registry.resolve(raw(&newest)).is_none());
}

struct PausedResolver {
    registry: Arc<SessionRegistry>,
    entered: mpsc::UnboundedSender<()>,
    resume: Arc<Semaphore>,
}
#[async_trait]
impl CredentialResolver for PausedResolver {
    async fn resolve(&self, token: &str) -> Option<InvocationScope> {
        self.entered.send(()).unwrap();
        self.resume.acquire().await.unwrap().forget();
        self.registry.resolve(token)
    }
}
#[tokio::test]
async fn preparation_reserves_before_validation_and_interrupted_validation_releases_it() {
    for cancelled in [false, true] {
        let registry = Arc::new(SessionRegistry::new(EnvironmentId("env".into()), None));
        let (entered, mut receiving) = mpsc::unbounded_channel();
        let resume = Arc::new(Semaphore::new(0));
        let manager = ProviderCredentials::with_resolver(
            registry.clone(),
            Arc::new(PausedResolver {
                registry: registry.clone(),
                entered,
                resume: resume.clone(),
            }),
        );
        let config = manager
            .prepare(request())
            .await
            .unwrap()
            .publish(runtime("old"))
            .unwrap();
        let preparing = manager.clone();
        let task = tokio::spawn(async move { preparing.prepare(request()).await });
        receiving.recv().await.unwrap();
        manager.release_runtime(&runtime("old")).unwrap();
        assert!(
            registry.resolve(raw(&config)).is_some(),
            "preparation must protect the token before async validation"
        );
        if cancelled {
            task.abort();
            assert!(matches!(task.await,Err(error) if error.is_cancelled()));
            assert!(
                registry.resolve(raw(&config)).is_none(),
                "cancelled preparation must not leave a reservation or an orphan credential"
            );
        } else {
            resume.add_permits(1);
            let prepared = task.await.unwrap().unwrap();
            assert!(!prepared.issued);
            let config = prepared.publish(runtime("new")).unwrap();
            manager.release_runtime(&runtime("old")).unwrap();
            assert!(registry.resolve(raw(&config)).is_some());
            manager.release_runtime(&runtime("new")).unwrap();
            assert!(registry.resolve(raw(&config)).is_none());
        }
    }
}

#[tokio::test]
async fn abandoned_and_stale_preparations_never_clear_replacement_credentials() {
    let (registry, manager) = setup();
    let abandoned = manager.prepare(request()).await.unwrap();
    let token = raw(&abandoned.config).to_owned();
    drop(abandoned);
    assert!(registry.resolve(&token).is_none());
    let stale = manager.prepare(request()).await.unwrap();
    let mut changed = request();
    changed.subagent = true;
    let replacement = manager
        .prepare(changed)
        .await
        .unwrap()
        .publish(runtime("replacement"))
        .unwrap();
    assert!(stale.publish(runtime("stale")).is_err());
    assert!(registry.resolve(raw(&replacement)).is_some());
    manager.release_runtime(&runtime("stale")).unwrap();
    assert_eq!(
        manager
            .read(&ThreadId("thread".into()))
            .unwrap()
            .unwrap()
            .authorization_header,
        replacement.authorization_header
    );
}
