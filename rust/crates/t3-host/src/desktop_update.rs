use crate::{Error, Result, background::Clock};
use async_trait::async_trait;
use std::{
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use t3_contracts::{
    base_schemas::TrimmedNonEmptyString,
    environment::ServerSelfUpdateMethod,
    ipc::{DesktopUpdateChannelSchema, DesktopUpdateStatusSchema},
    resource_telemetry::{DesktopUpdateRemoteOutcome, DesktopUpdateStatusReport},
    server::{ServerSelfUpdateProgressStage, ServerSelfUpdateResult},
};
use tokio::sync::mpsc;

pub fn remaining_wait(marker: &Path, clock: &dyn Clock) -> Option<u64> {
    let bytes = std::fs::read(marker).ok()?;
    let value: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    let now = clock.now_ms() as f64;
    let start = value["startedAt"].as_f64()?;
    let expires = value["expiresAt"].as_f64()?;
    (start.is_finite() && expires.is_finite() && start <= now && expires > now && expires > start)
        .then(|| ((expires - now).min(60_000.0)) as u64)
}
pub struct Reports {
    pub latest: Option<DesktopUpdateStatusReport>,
    pub changes: mpsc::UnboundedReceiver<DesktopUpdateStatusReport>,
}
#[async_trait]
pub trait DesktopControl: Send + Sync {
    async fn subscribe(&self) -> Result<Reports>;
    async fn request(&self, id: &str, track: Option<DesktopUpdateChannelSchema>) -> Result<()>;
    async fn request_checked(
        &self,
        _id: &str,
        _track: DesktopUpdateChannelSchema,
        _target_version: &str,
    ) -> Result<()> {
        Err(Error::Refused(
            "desktop control does not support a checked track switch",
        ))
    }
    fn cancel(&self, id: &str);
    async fn commit(&self, id: &str) -> Result<()>;
}
pub struct DesktopAppUpdate {
    pub available: bool,
    control: Arc<dyn DesktopControl>,
    in_flight: AtomicBool,
}
struct InFlight<'a>(&'a AtomicBool);
impl Drop for InFlight<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
    }
}
struct Cancel {
    control: Arc<dyn DesktopControl>,
    id: String,
    armed: bool,
}
impl Drop for Cancel {
    fn drop(&mut self) {
        if self.armed {
            self.control.cancel(&self.id);
        }
    }
}
impl DesktopAppUpdate {
    pub fn new(available: bool, control: Arc<dyn DesktopControl>) -> Self {
        Self {
            available,
            control,
            in_flight: AtomicBool::new(false),
        }
    }
    pub async fn run(
        &self,
        progress: impl FnMut(ServerSelfUpdateProgressStage) -> Result<()>,
        track: Option<DesktopUpdateChannelSchema>,
    ) -> Result<ServerSelfUpdateResult> {
        self.run_internal(progress, track, None).await
    }
    pub async fn run_checked(
        &self,
        progress: impl FnMut(ServerSelfUpdateProgressStage) -> Result<()>,
        track: Option<DesktopUpdateChannelSchema>,
        target_version: String,
    ) -> Result<ServerSelfUpdateResult> {
        self.run_internal(progress, track, Some(target_version))
            .await
    }
    async fn run_internal(
        &self,
        mut progress: impl FnMut(ServerSelfUpdateProgressStage) -> Result<()>,
        track: Option<DesktopUpdateChannelSchema>,
        target_version: Option<String>,
    ) -> Result<ServerSelfUpdateResult> {
        if !self.available {
            return Err(Error::Refused("desktop updates are unavailable"));
        }
        if self.in_flight.swap(true, Ordering::SeqCst) {
            return Err(Error::Refused("a desktop update is already in progress"));
        }
        let _guard = InFlight(&self.in_flight);
        let id = uuid::Uuid::new_v4().to_string();
        let mut cancellation = Cancel {
            control: self.control.clone(),
            id: id.clone(),
            armed: true,
        };
        let result = tokio::time::timeout(Duration::from_secs(20 * 60), async {
            let mut reports = self.control.subscribe().await?;
            if let (Some(track), Some(target)) = (track.clone(), target_version.as_deref()) {
                self.control.request_checked(&id, track, target).await?;
            } else {
                self.control.request(&id, track.clone()).await?;
            }
            let mut last = None;
            while let Some(report) = reports.changes.recv().await {
                if report.request_id.as_ref().map(|s| s.0.as_str()) != Some(id.as_str()) {
                    continue;
                }
                let stage = match report.state.status {
                    DesktopUpdateStatusSchema::Checking
                    | DesktopUpdateStatusSchema::Available
                    | DesktopUpdateStatusSchema::Downloading => {
                        Some(ServerSelfUpdateProgressStage::Downloading)
                    }
                    DesktopUpdateStatusSchema::Downloaded => {
                        Some(ServerSelfUpdateProgressStage::Installing)
                    }
                    _ => None,
                };
                if let Some(outcome) = report.outcome {
                    match outcome {
                        DesktopUpdateRemoteOutcome::ReadyToInstall => {
                            if last != Some(ServerSelfUpdateProgressStage::Installing) {
                                progress(ServerSelfUpdateProgressStage::Installing)?;
                            }
                            return Ok(ServerSelfUpdateResult {
                                target_version: TrimmedNonEmptyString(
                                    report
                                        .state
                                        .downloaded_version
                                        .or(report.state.available_version)
                                        .unwrap_or(report.state.current_version),
                                ),
                                method: ServerSelfUpdateMethod::DesktopApp,
                                desktop_update_token: Some(TrimmedNonEmptyString(id.clone())),
                                update_id: None,
                            });
                        }
                        DesktopUpdateRemoteOutcome::UpToDate if track.is_some() => {
                            return Ok(ServerSelfUpdateResult {
                                target_version: TrimmedNonEmptyString(report.state.current_version),
                                method: ServerSelfUpdateMethod::DesktopApp,
                                desktop_update_token: None,
                                update_id: None,
                            });
                        }
                        _ => {
                            return Err(Error::Refused(
                                "desktop update did not prepare an install",
                            ));
                        }
                    }
                }
                if let Some(stage) = stage
                    && last != Some(stage)
                {
                    progress(stage)?;
                    last = Some(stage);
                }
            }
            Err(Error::Refused("desktop app stopped reporting its update"))
        })
        .await
        .map_err(|_| Error::Refused("desktop update timed out"))?;
        cancellation.armed = false;
        result
    }
    pub async fn commit(
        &self,
        id: &str,
        on_accepted: impl FnOnce(),
    ) -> Result<std::convert::Infallible> {
        if !self.available {
            return Err(Error::Refused("desktop updates are unavailable"));
        }
        let mut reports = self.control.subscribe().await?;
        self.control.commit(id).await?;
        on_accepted();
        tokio::time::timeout(Duration::from_secs(120), async {
            let mut report = reports.latest.take();
            loop {
                if let Some(report) = report.take()
                    && report.request_id.as_ref().map(|s| s.0.as_str()) == Some(id)
                    && report.outcome == Some(DesktopUpdateRemoteOutcome::Failed)
                {
                    return Err(Error::Refused("desktop install failed"));
                }
                report = reports.changes.recv().await;
                if report.is_none() {
                    return Err(Error::Refused("desktop app stopped reporting the install"));
                }
            }
        })
        .await
        .map_err(|_| Error::Refused("desktop install timed out"))?
    }
}
