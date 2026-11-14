#![allow(clippy::unwrap_used)]
mod support;
use async_trait::async_trait;
use futures_util::StreamExt;
use sha2::{Digest, Sha256};
use std::{
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};
use t3_cloud::managed::ManagedEndpoint;
use t3_cloud::{Secret, api::RequestContext, relay_client::*};
use t3_contracts::{auth::AuthEnvironmentScope as Scope, relay_client::*};
use tokio::sync::watch;

const BINARY: &[u8] = b"cloudflared version 2026.10.4";
struct Download(AtomicUsize);
#[async_trait]
impl BinaryDownload for Download {
    async fn fetch(&self, _: &str, destination: &Path) -> Result<(), InstallError> {
        self.0.fetch_add(1, Ordering::SeqCst);
        tokio::fs::write(destination, BINARY).await.unwrap();
        Ok(())
    }
}
struct Probe;
#[async_trait]
impl ExecutableProbe for Probe {
    async fn version(&self, path: &Path, _: Duration) -> Option<String> {
        (tokio::fs::read(path).await.ok()?.as_slice() == BINARY).then(|| PINNED_VERSION.into())
    }
    fn process_alive(&self, _: u32) -> bool {
        true
    }
}
fn context(scopes: Vec<Scope>) -> RequestContext {
    RequestContext {
        request_url: Secret::new("http://127.0.0.1"),
        subject: "test".into(),
        scopes,
        forwarded_authority: false,
    }
}
fn options(directory: &Path) -> (RelayClientOptions, watch::Sender<bool>) {
    let (allowed, receiver) = watch::channel(true);
    (
        RelayClientOptions {
            base_dir: directory.into(),
            platform: "win32".into(),
            arch: "x64".into(),
            executable_override: None,
            search_path: vec![],
            release: Some(ReleaseAsset {
                url: "https://release.test/marker-url-secret".into(),
                sha256: format!("{:x}", Sha256::digest(BINARY)),
                archive: Archive::Binary,
            }),
            install_allowed: receiver,
        },
        allowed,
    )
}

#[tokio::test]
async fn progress_install_checks_hash_activates_once_and_status_requires_read_scope() {
    let directory = tempfile::tempdir().unwrap();
    let (options, _allowed) = options(directory.path());
    let download = Arc::new(Download(AtomicUsize::new(0)));
    let client = RelayClient::new(options, download.clone(), Arc::new(Probe));
    assert!(client.status(&context(vec![])).await.is_err());
    assert!(
        client
            .install_with_progress(&context(vec![Scope::RelayRead]))
            .is_err()
    );
    assert!(matches!(
        client
            .status(&context(vec![Scope::RelayRead]))
            .await
            .unwrap(),
        RelayClientStatus::Missing { .. }
    ));
    let mut stream = client
        .install_with_progress(&context(vec![Scope::RelayWrite]))
        .unwrap();
    let mut stages = vec![];
    let mut complete = None;
    while let Some(event) = stream.next().await {
        match event.unwrap() {
            RelayClientInstallProgressEvent::Progress { stage } => stages.push(stage),
            RelayClientInstallProgressEvent::Complete { status } => complete = Some(status),
        }
    }
    stream.shutdown().await;
    assert_eq!(
        stages,
        vec![
            RelayClientInstallProgressStage::Checking,
            RelayClientInstallProgressStage::WaitingForLock,
            RelayClientInstallProgressStage::Downloading,
            RelayClientInstallProgressStage::Verifying,
            RelayClientInstallProgressStage::Installing,
            RelayClientInstallProgressStage::Validating,
            RelayClientInstallProgressStage::Activating
        ]
    );
    assert!(matches!(
        complete,
        Some(RelayClientStatus::Available {
            source: RelayClientSource::Managed,
            ..
        })
    ));
    assert_eq!(tokio::fs::read(client.pinned_path()).await.unwrap(), BINARY);
    let again = client.install(Arc::new(|_| {})).await.unwrap();
    assert_eq!(Some(again), complete);
    assert_eq!(download.0.load(Ordering::SeqCst), 1);
    assert!(
        !client
            .pinned_path()
            .with_file_name("cloudflared.exe.lock")
            .exists()
    );
}

#[tokio::test]
async fn invalid_checksum_never_activates_and_install_policy_is_live() {
    let directory = tempfile::tempdir().unwrap();
    let (mut options, allowed) = options(directory.path());
    options.release.as_mut().unwrap().sha256 = "0".repeat(64);
    assert!(!format!("{:?}", options.release).contains("marker-url-secret"));
    let download = Arc::new(Download(AtomicUsize::new(0)));
    let client = RelayClient::new(options, download.clone(), Arc::new(Probe));
    let error = client.install(Arc::new(|_| {})).await.unwrap_err();
    assert_eq!(
        error.reason,
        RelayClientInstallFailureReason::InvalidChecksum
    );
    assert!(!format!("{error:?} {error}").contains("marker-url-secret"));
    assert!(!client.pinned_path().exists());
    allowed.send_replace(false);
    assert_eq!(
        client.install(Arc::new(|_| {})).await.unwrap_err().reason,
        RelayClientInstallFailureReason::ManagedByPolicy
    );
    assert_eq!(download.0.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn a_missing_override_refuses_install_and_never_falls_back_to_path() {
    let directory = tempfile::tempdir().unwrap();
    let (mut options, _allowed) = options(directory.path());
    options.executable_override = Some(directory.path().join("missing.exe"));
    options.search_path = vec![directory.path().into()];
    tokio::fs::write(directory.path().join("cloudflared.exe"), BINARY)
        .await
        .unwrap();
    let download = Arc::new(Download(AtomicUsize::new(0)));
    let client = RelayClient::new(options, download.clone(), Arc::new(Probe));
    assert!(matches!(
        client.resolve().await,
        RelayClientStatus::Missing { .. }
    ));
    assert_eq!(
        client.install(Arc::new(|_| {})).await.unwrap_err().reason,
        RelayClientInstallFailureReason::OverrideMissing
    );
    assert_eq!(download.0.load(Ordering::SeqCst), 0);
    let endpoint = t3_cloud::managed::NativeManagedEndpoint::with_relay_client(
        client,
        Arc::new(support::Relay::default()),
        Arc::new(support::MemorySecrets::default()),
        Arc::new(support::Time::default()),
    );
    assert!(matches!(
        endpoint
            .apply(Some(t3_cloud::http::Sensitive::new(support::runtime())))
            .await
            .unwrap(),
        t3_cloud::managed::EndpointStatus::Failed { .. }
    ));
    assert!(matches!(
        endpoint.apply(None).await.unwrap(),
        t3_cloud::managed::EndpointStatus::Disabled
    ));
}
