use async_trait::async_trait;
use std::{
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};
use t3_contracts::server::*;
use t3_host::{
    Result,
    desktop_update::{DesktopAppUpdate, DesktopControl, Reports},
    policy::ManagedPolicy,
    self_update::*,
};
use tokio::sync::Notify;
struct Desktop;
#[async_trait]
impl DesktopControl for Desktop {
    async fn subscribe(&self) -> Result<Reports> {
        Err(t3_host::Error::Service)
    }
    async fn request(
        &self,
        _: &str,
        _: Option<t3_contracts::ipc::DesktopUpdateChannelSchema>,
    ) -> Result<()> {
        panic!("desktop unavailable")
    }
    fn cancel(&self, _: &str) {}
    async fn commit(&self, _: &str) -> Result<()> {
        Err(t3_host::Error::Service)
    }
}
struct Host {
    calls: AtomicUsize,
    blocked: bool,
    started: Notify,
    release: Notify,
    hold: bool,
    accepted: Notify,
}
#[async_trait]
impl UpdateHost for Host {
    fn launcher_protocol(&self) -> Option<u32> {
        Some(3)
    }
    async fn migration_head(&self) -> Option<i64> {
        Some(91)
    }
    async fn install_runtime(&self, request: RuntimeRequest) -> UpdateResult<PathBuf> {
        assert_eq!(request.launcher_protocol, 3);
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.started.notify_one();
        if self.hold {
            self.release.notified().await;
        }
        if self.blocked {
            Err(update_error("local update required"))
        } else {
            Ok("prepared/t3".into())
        }
    }
    async fn request_update(&self, version: &str, _: &Path) -> UpdateResult<String> {
        assert_eq!(version, "1.1.0");
        self.accepted.notify_one();
        Ok("launcher-id".into())
    }
    async fn handoff_accepted(&self) {}
}
fn host(blocked: bool, hold: bool) -> Arc<Host> {
    Arc::new(Host {
        calls: AtomicUsize::new(0),
        blocked,
        started: Notify::new(),
        release: Notify::new(),
        hold,
        accepted: Notify::new(),
    })
}
fn service(host: Arc<Host>, policy: ManagedPolicy) -> Arc<ServerSelfUpdate> {
    Arc::new(ServerSelfUpdate::new(
        "1.0.0".into(),
        "state.sqlite".into(),
        false,
        Arc::new(DesktopAppUpdate::new(false, Arc::new(Desktop))),
        Arc::new(policy),
        host,
    ))
}
fn input(version: &str) -> ServerSelfUpdateInput {
    ServerSelfUpdateInput {
        target_version: version.into(),
        channel: None,
        target_database_migration: None,
    }
}
#[tokio::test]
async fn update_refuses_policy_invalid_versions_and_incompatible_track_before_installing() {
    let h = host(false, false);
    let s = service(h.clone(), ManagedPolicy::from_document(None).unwrap());
    for version in [
        "../1.1.0",
        "latest",
        "1.1.0;echo bad",
        "1.1.0-nightly.20261028.1",
    ] {
        assert!(s.update(input(version)).await.is_err());
    }
    assert_eq!(h.calls.load(Ordering::SeqCst), 0);
    let policy=serde_json::from_value(serde_json::json!({"schemaVersion":1,"organization":"Example Corp","limits":{"updates":{"mode":"off"}}})).unwrap();
    let s = service(
        h.clone(),
        ManagedPolicy::from_document(Some(policy)).unwrap(),
    );
    assert_eq!(
        s.update(input("1.1.0")).await.unwrap_err().reason.0,
        "Example Corp manages T3 Code updates."
    );
    assert_eq!(h.calls.load(Ordering::SeqCst), 0);
}
#[tokio::test]
async fn progress_survives_disconnection_and_busy_gate_stays_closed_after_handoff() {
    let h = host(false, true);
    let s = service(h.clone(), ManagedPolicy::from_document(None).unwrap());
    let mut stream = s.update_with_progress(input("1.1.0"));
    assert!(matches!(
        stream.recv().await.unwrap().unwrap(),
        ServerSelfUpdateProgressEvent::Progress(_)
    ));
    h.started.notified().await;
    drop(stream);
    assert_eq!(
        s.update(input("1.1.0")).await.unwrap_err().reason.0,
        "A server update is already in progress."
    );
    h.release.notify_one();
    h.accepted.notified().await;
    assert!(s.update(input("1.1.0")).await.is_err());
}
#[tokio::test]
async fn preparation_failure_releases_gate_and_has_no_complete_event() {
    let h = host(true, false);
    let s = service(h.clone(), ManagedPolicy::from_document(None).unwrap());
    for _ in 0..2 {
        let mut stream = s.update_with_progress(input("1.1.0"));
        assert!(matches!(
            stream.recv().await.unwrap().unwrap(),
            ServerSelfUpdateProgressEvent::Progress(_)
        ));
        assert_eq!(
            stream.recv().await.unwrap().unwrap_err().reason.0,
            "local update required"
        );
        assert!(stream.recv().await.is_none());
    }
    assert_eq!(h.calls.load(Ordering::SeqCst), 2);
}
#[test]
fn track_switch_refuses_unknown_or_older_migrations_but_preview_crosses_nothing() {
    assert!(
        track_switch_block_reason("1.0.0", "1.1.0-nightly.20261101.1", Some(90), Some(91))
            .is_some()
    );
    assert!(
        track_switch_block_reason("1.0.0", "1.1.0-nightly.20261101.1", Some(91), None).is_some()
    );
    assert!(
        track_switch_block_reason("1.0.0", "1.1.0-nightly.20261101.1", Some(91), Some(91))
            .is_none()
    );
    assert!(track_switch_block_reason("1.0.0-preview.20261101.1", "1.1.0", None, None).is_none());
}
