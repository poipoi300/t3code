use async_trait::async_trait;
use std::{
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};
use t3_contracts::{
    ipc::DesktopUpdateChannelSchema, resource_telemetry::DesktopUpdateStatusReport,
    server::ServerSelfUpdateProgressStage,
};
use t3_host::{Result, background::Clock, desktop_update::*};
use tokio::sync::Mutex;
struct Time(i64);
impl Clock for Time {
    fn now_ms(&self) -> i64 {
        self.0
    }
}
#[test]
fn marker_expiry_cases_are_read_only_and_capped() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("marker");
    for (raw, now, expected) in [
        (r#"{"startedAt":0,"expiresAt":60000}"#, 0, Some(60000)),
        (r#"{"startedAt":0,"expiresAt":60000}"#, 8000, Some(52000)),
        (r#"{"startedAt":0,"expiresAt":60000}"#, 60000, None),
        (r#"{"startedAt":1000,"expiresAt":61000}"#, 0, None),
        (r#"{"startedAt":0,"expiresAt":600000}"#, 0, Some(60000)),
        (r#"{"startedAt":1000,"expiresAt":0}"#, 0, None),
        ("{}", 0, None),
        ("not json", 0, None),
    ] {
        std::fs::write(&path, raw).unwrap();
        assert_eq!(remaining_wait(&path, &Time(now)), expected);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), raw);
    }
    assert_eq!(remaining_wait(Path::new("missing-marker"), &Time(0)), None);
}
fn report(id: &str, status: &str, outcome: Option<&str>) -> DesktopUpdateStatusReport {
    let mut value = serde_json::json!({"version":1,"type":"desktopUpdateStatus","requestId":id,"state":{"enabled":true,"status":status,"channel":"latest","currentVersion":"1.0.0","hostArch":"x64","appArch":"x64","runningUnderArm64Translation":false,"availableVersion":"1.1.0","downloadedVersion":"1.1.0","releaseNotes":[],"omittedReleaseCount":0,"downloadPercent":null,"checkedAt":null,"message":null,"errorContext":null,"canRetry":false}});
    if let Some(outcome) = outcome {
        value["outcome"] = outcome.into();
    }
    serde_json::from_value(value).unwrap()
}
struct Control {
    sender: Mutex<Option<tokio::sync::mpsc::UnboundedSender<DesktopUpdateStatusReport>>>,
    script: Vec<(&'static str, Option<&'static str>)>,
    keep_open: bool,
    cancelled: AtomicUsize,
    latest: Option<DesktopUpdateStatusReport>,
}
#[async_trait]
impl DesktopControl for Control {
    async fn subscribe(&self) -> Result<Reports> {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        *self.sender.lock().await = Some(tx);
        Ok(Reports {
            latest: self.latest.clone(),
            changes: rx,
        })
    }
    async fn request(&self, id: &str, _: Option<DesktopUpdateChannelSchema>) -> Result<()> {
        let mut sender = self.sender.lock().await;
        for (status, outcome) in &self.script {
            sender
                .as_ref()
                .unwrap()
                .send(report(id, status, *outcome))
                .unwrap();
        }
        if !self.keep_open {
            sender.take();
        }
        Ok(())
    }
    fn cancel(&self, _: &str) {
        self.cancelled.fetch_add(1, Ordering::SeqCst);
    }
    async fn commit(&self, _: &str) -> Result<()> {
        Ok(())
    }
}
fn control(script: Vec<(&'static str, Option<&'static str>)>, keep_open: bool) -> Arc<Control> {
    Arc::new(Control {
        sender: Mutex::new(None),
        script,
        keep_open,
        cancelled: AtomicUsize::new(0),
        latest: None,
    })
}
#[tokio::test]
async fn desktop_progress_deduplicates_and_returns_preparation_token() {
    let c = control(
        vec![
            ("checking", None),
            ("available", None),
            ("downloading", None),
            ("downloaded", Some("ready-to-install")),
        ],
        false,
    );
    let service = DesktopAppUpdate::new(true, c.clone());
    let mut stages = vec![];
    let result = service
        .run(
            |s| {
                stages.push(s);
                Ok(())
            },
            None,
        )
        .await
        .unwrap();
    assert_eq!(
        stages,
        vec![
            ServerSelfUpdateProgressStage::Downloading,
            ServerSelfUpdateProgressStage::Installing
        ]
    );
    assert_eq!(result.target_version.0, "1.1.0");
    assert!(uuid::Uuid::parse_str(&result.desktop_update_token.unwrap().0).is_ok());
    assert_eq!(c.cancelled.load(Ordering::SeqCst), 0);
}
#[tokio::test(start_paused = true)]
async fn timeout_and_closed_reports_fail_without_hanging() {
    let service = DesktopAppUpdate::new(true, control(vec![], true));
    assert!(service.run(|_| Ok(()), None).await.is_err());
    assert!(service.run(|_| Ok(()), None).await.is_err());
    let service = DesktopAppUpdate::new(true, control(vec![], false));
    assert!(service.run(|_| Ok(()), None).await.is_err());
}
#[tokio::test]
async fn track_switch_can_be_up_to_date_but_normal_update_cannot() {
    let service = DesktopAppUpdate::new(true, control(vec![("idle", Some("up-to-date"))], false));
    assert!(service.run(|_| Ok(()), None).await.is_err());
    let result = service
        .run(|_| Ok(()), Some(DesktopUpdateChannelSchema::Nightly))
        .await
        .unwrap();
    assert!(result.desktop_update_token.is_none());
}
#[tokio::test]
async fn commit_replays_retained_failure_after_acceptance() {
    let c = Arc::new(Control {
        sender: Mutex::new(None),
        script: vec![],
        keep_open: true,
        cancelled: AtomicUsize::new(0),
        latest: Some(report("prepared", "error", Some("failed"))),
    });
    let service = DesktopAppUpdate::new(true, c);
    let mut accepted = false;
    assert!(
        service
            .commit("prepared", || accepted = true)
            .await
            .is_err()
    );
    assert!(accepted);
}

#[tokio::test]
async fn checked_track_switch_forwards_target_and_refuses_an_older_control_adapter() {
    let plain = control(vec![("downloaded", Some("ready-to-install"))], false);
    let service = DesktopAppUpdate::new(true, plain.clone());
    assert!(
        service
            .run_checked(
                |_| Ok(()),
                Some(DesktopUpdateChannelSchema::Nightly),
                "1.1.0-nightly.20261101.1".into()
            )
            .await
            .is_err()
    );
    assert_eq!(plain.cancelled.load(Ordering::SeqCst), 1);
    struct Checked {
        inner: Arc<Control>,
        received: Mutex<Option<(DesktopUpdateChannelSchema, String)>>,
    }
    #[async_trait]
    impl DesktopControl for Checked {
        async fn subscribe(&self) -> Result<Reports> {
            self.inner.subscribe().await
        }
        async fn request(&self, _: &str, _: Option<DesktopUpdateChannelSchema>) -> Result<()> {
            panic!("checked target must not be dropped")
        }
        async fn request_checked(
            &self,
            id: &str,
            channel: DesktopUpdateChannelSchema,
            target: &str,
        ) -> Result<()> {
            *self.received.lock().await = Some((channel, target.into()));
            self.inner.request(id, None).await
        }
        fn cancel(&self, id: &str) {
            self.inner.cancel(id)
        }
        async fn commit(&self, id: &str) -> Result<()> {
            self.inner.commit(id).await
        }
    }
    let checked = Arc::new(Checked {
        inner: control(vec![("downloaded", Some("ready-to-install"))], false),
        received: Mutex::new(None),
    });
    DesktopAppUpdate::new(true, checked.clone())
        .run_checked(
            |_| Ok(()),
            Some(DesktopUpdateChannelSchema::Nightly),
            "1.1.0-nightly.20261101.1".into(),
        )
        .await
        .unwrap();
    assert_eq!(
        *checked.received.lock().await,
        Some((
            DesktopUpdateChannelSchema::Nightly,
            "1.1.0-nightly.20261101.1".into()
        ))
    );
}
