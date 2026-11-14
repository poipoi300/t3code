use crate::{
    desktop_update::DesktopAppUpdate, policy::ManagedPolicy,
    service_launcher::is_exact_service_version,
};
use async_trait::async_trait;
use std::{
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};
use t3_contracts::{environment::ServerSelfUpdateMethod, server::*};
use tokio::sync::mpsc;
pub type UpdateResult<T> = std::result::Result<T, ServerSelfUpdateError>;
pub fn update_error(reason: impl Into<String>) -> ServerSelfUpdateError {
    ServerSelfUpdateError {
        tag: ServerSelfUpdateErrorTag::ServerSelfUpdateError,
        reason: reason.into().into(),
        cause: None,
    }
}
pub fn release_channel(version: &str) -> &'static str {
    static RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let re = RE.get_or_init(|| {
        regex::Regex::new(r"^[^-+]+-(nightly|preview)\.[0-9]{8}\.[0-9]+$")
            .expect("constant channel regex")
    });
    match re
        .captures(version)
        .and_then(|c| c.get(1))
        .map(|s| s.as_str())
    {
        Some("nightly") => "nightly",
        Some("preview") => "preview",
        _ => "stable",
    }
}
pub fn track_switch_block_reason(
    from: &str,
    to: &str,
    target: Option<i64>,
    database: Option<i64>,
) -> Option<String> {
    let from = release_channel(from);
    let to_channel = release_channel(to);
    if from == to_channel || from == "preview" || to_channel == "preview" {
        return None;
    }
    match (target, database) {
        (Some(target), Some(database)) if target >= database => None,
        (Some(_), Some(_)) => Some(format!(
            "This database has changes t3@{to} does not support yet; it stays on this release."
        )),
        _ => Some(format!(
            "Cannot confirm this database works with t3@{to}. Switch tracks from Settings → About → Update track, which checks it first."
        )),
    }
}
/// Feed URLs can contain credentials; deliberately has no Debug implementation.
pub struct RuntimeRequest {
    pub target_version: String,
    pub database_path: PathBuf,
    pub launcher_protocol: u32,
    pub release_base_url: Option<String>,
    pub require_published_release: bool,
    pub mirror_organization: Option<String>,
}
#[async_trait]
pub trait UpdateHost: Send + Sync {
    fn launcher_protocol(&self) -> Option<u32>;
    /// Read the current persisted migration head, at least this build's head; unavailable means None.
    async fn migration_head(&self) -> Option<i64>;
    async fn install_runtime(&self, request: RuntimeRequest) -> UpdateResult<PathBuf>;
    /// Must register the reply listener before sending, match this request and bound the wait.
    async fn request_update(&self, version: &str, database: &Path) -> UpdateResult<String>;
    async fn handoff_accepted(&self);
}
pub struct ServerSelfUpdate {
    current_version: String,
    database_path: PathBuf,
    desktop_managed: bool,
    desktop: Arc<DesktopAppUpdate>,
    policy: Arc<ManagedPolicy>,
    host: Arc<dyn UpdateHost>,
    in_flight: AtomicBool,
}
impl ServerSelfUpdate {
    pub fn new(
        current_version: String,
        database_path: PathBuf,
        desktop_managed: bool,
        desktop: Arc<DesktopAppUpdate>,
        policy: Arc<ManagedPolicy>,
        host: Arc<dyn UpdateHost>,
    ) -> Self {
        Self {
            current_version,
            database_path,
            desktop_managed,
            desktop,
            policy,
            host,
            in_flight: AtomicBool::new(false),
        }
    }
    pub async fn update(
        self: &Arc<Self>,
        input: ServerSelfUpdateInput,
    ) -> UpdateResult<ServerSelfUpdateResult> {
        let service = self.clone();
        tokio::spawn(async move { service.perform(input, None).await })
            .await
            .map_err(|_| update_error("Server update preparation stopped unexpectedly."))?
    }
    pub fn update_with_progress(
        self: &Arc<Self>,
        input: ServerSelfUpdateInput,
    ) -> mpsc::UnboundedReceiver<UpdateResult<ServerSelfUpdateProgressEvent>> {
        let (tx, rx) = mpsc::unbounded_channel();
        let service = self.clone();
        tokio::spawn(async move {
            let result = service.perform(input, Some(&tx)).await;
            let event = result.map(|result| {
                ServerSelfUpdateProgressEvent::Complete(ServerSelfUpdateComplete {
                    r#type: ServerSelfUpdateCompleteType::Complete,
                    result,
                })
            });
            let _ = tx.send(event);
        });
        rx
    }
    async fn perform(
        &self,
        input: ServerSelfUpdateInput,
        progress: Option<&mpsc::UnboundedSender<UpdateResult<ServerSelfUpdateProgressEvent>>>,
    ) -> UpdateResult<ServerSelfUpdateResult> {
        let version = input.target_version.0.trim();
        self.policy
            .check_update((!self.desktop_managed).then(|| release_channel(version)))
            .map_err(|e| update_error(e.to_string()))?;
        let crosses = release_channel(&self.current_version) != release_channel(version)
            && release_channel(&self.current_version) != "preview"
            && release_channel(version) != "preview";
        if crosses
            && (!self.desktop_managed || input.channel.is_some())
            && let Some(reason) = track_switch_block_reason(
                &self.current_version,
                version,
                input.target_database_migration,
                self.host.migration_head().await,
            )
        {
            return Err(update_error(reason));
        }
        let emit = |stage| {
            if let Some(tx) = progress {
                let _ = tx.send(Ok(ServerSelfUpdateProgressEvent::Progress(
                    ServerSelfUpdateProgress {
                        r#type: ServerSelfUpdateProgressType::Progress,
                        stage,
                    },
                )));
            }
        };
        if self.desktop_managed {
            if !self.desktop.available {
                return Err(update_error(
                    "This server is managed by the T3 Code desktop app on its machine; update the desktop app to update it.",
                ));
            }
            return self
                .desktop
                .run_checked(
                    |stage| {
                        emit(stage);
                        Ok(())
                    },
                    input.channel,
                    input.target_version.0,
                )
                .await
                .map_err(|e| update_error(e.to_string()));
        }
        let Some(protocol) = self.host.launcher_protocol().filter(|p| matches!(p, 3 | 4)) else {
            return Err(update_error(
                "Remote updates require the T3 Code background service. Run `t3 service install` on the server machine.",
            ));
        };
        if !is_exact_service_version(version) {
            return Err(update_error(format!(
                "'{version}' is not an exact t3 version."
            )));
        }
        if self.in_flight.swap(true, Ordering::SeqCst) {
            return Err(update_error("A server update is already in progress."));
        }
        let outcome = async {
            emit(ServerSelfUpdateProgressStage::Downloading);
            let mirror = self.policy.update_mirror();
            let installed = self
                .host
                .install_runtime(RuntimeRequest {
                    target_version: version.into(),
                    database_path: self.database_path.clone(),
                    launcher_protocol: protocol,
                    release_base_url: mirror.map(|(_, url)| url.into()),
                    require_published_release: mirror.is_some(),
                    mirror_organization: mirror.map(|(organization, _)| organization.into()),
                })
                .await;
            installed?;
            emit(ServerSelfUpdateProgressStage::Installing);
            let id = tokio::time::timeout(
                std::time::Duration::from_secs(30),
                self.host.request_update(version, &self.database_path),
            )
            .await
            .map_err(|_| {
                update_error("The service launcher did not respond within 30 seconds.")
            })??;
            if id.trim().is_empty() {
                return Err(update_error(
                    "The service launcher supplied an invalid update id.",
                ));
            }
            self.host.handoff_accepted().await;
            Ok(ServerSelfUpdateResult {
                target_version: version.into(),
                method: ServerSelfUpdateMethod::BootService,
                update_id: Some(id.into()),
                desktop_update_token: None,
            })
        }
        .await;
        if outcome.is_err() {
            self.in_flight.store(false, Ordering::SeqCst);
        }
        outcome
    }
    pub async fn commit_desktop_update(
        self: &Arc<Self>,
        request_id: String,
    ) -> UpdateResult<std::convert::Infallible> {
        let service = self.clone();
        tokio::spawn(async move {
            let (accepted, wait) = tokio::sync::oneshot::channel();
            let host = service.host.clone();
            let notify = tokio::spawn(async move {
                if wait.await.is_ok() {
                    host.handoff_accepted().await;
                }
            });
            let result = service
                .desktop
                .commit(&request_id, || {
                    let _ = accepted.send(());
                })
                .await;
            let _ = notify.await;
            result.map_err(|e| update_error(e.to_string()))
        })
        .await
        .map_err(|_| update_error("Desktop install stopped unexpectedly."))?
    }
}
#[async_trait]
pub trait LauncherClient: Send + Sync {
    fn protocol(&self) -> Option<u32>;
    async fn request_update(&self, version: &str, database: &Path) -> UpdateResult<String>;
    async fn handoff_accepted(&self);
}
pub struct NativeUpdateHost {
    database: t3_persistence::Database,
    installer: Arc<crate::pinned_runtime::PinnedRuntimeInstaller>,
    launcher: Arc<dyn LauncherClient>,
}
impl NativeUpdateHost {
    pub fn new(
        database: t3_persistence::Database,
        installer: Arc<crate::pinned_runtime::PinnedRuntimeInstaller>,
        launcher: Arc<dyn LauncherClient>,
    ) -> Self {
        Self {
            database,
            installer,
            launcher,
        }
    }
}
#[async_trait]
impl UpdateHost for NativeUpdateHost {
    fn launcher_protocol(&self) -> Option<u32> {
        self.launcher.protocol()
    }
    async fn migration_head(&self) -> Option<i64> {
        self.database
            .reader(|c| {
                Ok(t3_persistence::migrations::history(c)?
                    .iter()
                    .map(|m| m.id)
                    .max()
                    .unwrap_or(0)
                    .max(t3_persistence::migrations::LATEST_HEAD) as i64)
            })
            .await
            .ok()
    }
    async fn install_runtime(&self, request: RuntimeRequest) -> UpdateResult<PathBuf> {
        self.installer.ensure(request).await
    }
    async fn request_update(&self, version: &str, database: &Path) -> UpdateResult<String> {
        self.launcher.request_update(version, database).await
    }
    async fn handoff_accepted(&self) {
        self.launcher.handoff_accepted().await;
    }
}
