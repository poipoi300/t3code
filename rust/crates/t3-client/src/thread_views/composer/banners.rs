//! Machine notices from useSystemComposerBanners; delivery failures stay in the timeline.
use super::{send::Changes, target::ComposerTarget};
use crate::{
    ClientError,
    composition::ClientRuntime,
    connection::{PresentationPhase, present_connection_state},
    platform::Platform,
};
use futures::{FutureExt, StreamExt, future::LocalBoxFuture, stream::LocalBoxStream};
use std::{cell::RefCell, collections::BTreeMap, rc::Rc};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BannerVariant {
    Default,
    Warning,
    Error,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BannerAction {
    pub id: String,
    pub label: String,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BannerIcon {
    Disconnected,
    UpdateRunning,
    UpdateFailed,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ComposerNotice {
    pub id: String,
    pub variant: BannerVariant,
    pub title: String,
    pub title_template: &'static str,
    pub title_arguments: BTreeMap<String, String>,
    pub icon: BannerIcon,
    pub description: Option<String>,
    pub description_is_key: bool,
    pub actions: Vec<BannerAction>,
    pub dismiss_label: Option<String>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BannerOutcome {
    Unchanged,
    NavigateHome,
}
pub(crate) type UpdateDismissals = Rc<
    RefCell<
        crate::view_logic::session::version::UpdateFailureDismissals<
            crate::services::updates::ServerUpdateState,
        >,
    >,
>;
#[derive(Default)]
struct State {
    reconnect_since: Option<u64>,
    unavailable_since: Option<u64>,
    notices: Vec<Rc<ComposerNotice>>,
    changes: Changes,
    client_version: Option<String>,
}
#[derive(Clone)]
pub struct ComposerBannersSession<P: Platform + Clone + 'static> {
    runtime: ClientRuntime<P>,
    platform: P,
    target: ComposerTarget,
    state: Rc<RefCell<State>>,
    dismissals: UpdateDismissals,
}
impl<P: Platform + Clone + 'static> ComposerBannersSession<P> {
    pub fn new(runtime: ClientRuntime<P>, platform: P, target: ComposerTarget) -> Self {
        let dismissals = runtime.composer_update_dismissals();
        Self {
            runtime,
            platform,
            target,
            state: Rc::default(),
            dismissals,
        }
    }
    pub async fn load_client_version(&self) {
        if let Ok(presentation) = self.platform.client_presentation().await {
            let mut state = self.state.borrow_mut();
            state.client_version = presentation.app_version.map(|version| version.0);
            state.changes.notify();
        }
    }
    pub fn snapshot(&self) -> Vec<Rc<ComposerNotice>> {
        let Some(id) = self.target.environment() else {
            return vec![];
        };
        let Some(entry) = self
            .runtime
            .environments()
            .into_iter()
            .find(|entry| &entry.id == id)
        else {
            return vec![];
        };
        let presentation = present_connection_state(&entry.state);
        let reconnecting = matches!(
            presentation.phase,
            PresentationPhase::Connecting | PresentationPhase::Reconnecting
        );
        let unavailable = presentation.phase != PresentationPhase::Connected;
        let update_resource = self.runtime.composer_server_updates().state(id.clone());
        let snapshot = self.runtime.store().snapshot();
        let config = snapshot
            .environments
            .get(id)
            .and_then(|environment| environment.config.as_deref());
        let current_update = update_resource.snapshot().value;
        let update = crate::services::updates::update_state_for_server_version(
            current_update.as_deref().cloned().unwrap_or_default(),
            config.map(|config| config.environment.server_version.0.as_str()),
        );
        let updating = matches!(
            update,
            crate::services::updates::ServerUpdateState::Running { .. }
        );
        let now = self.platform.monotonic_ms();
        let mut state = self.state.borrow_mut();
        if reconnecting {
            state.reconnect_since.get_or_insert(now);
        } else {
            state.reconnect_since = None;
        }
        if unavailable {
            state.unavailable_since.get_or_insert(now);
        } else {
            state.unavailable_since = None;
        }
        let mut next = vec![];
        let can_disconnect = unavailable
            && state
                .unavailable_since
                .is_some_and(|start| now.saturating_sub(start) >= 20_000)
            && !entry
                .routes
                .iter()
                .any(|route| route.mode == crate::connection::ConnectionMode::Local);
        if unavailable
            && (presentation.awaiting_approval
                || !reconnecting
                || !updating
                    && state.reconnect_since.is_some_and(|start| {
                        now.saturating_sub(start)
                            >= crate::view_logic::chat_view::ENVIRONMENT_RECONNECT_WARNING_GRACE_MS
                    }))
        {
            let mut actions = vec![];
            if presentation.phase != PresentationPhase::Connecting {
                actions.push(BannerAction {
                    id: "reconnect".into(),
                    label: if reconnecting {
                        "Retry now"
                    } else {
                        "Reconnect"
                    }
                    .into(),
                });
            }
            if can_disconnect {
                actions.push(BannerAction {
                    id: "disconnect".into(),
                    label: "Disconnect server".into(),
                });
            }
            next.push(ComposerNotice {
                id: format!("environment-unavailable:{}", id.0),
                variant: if presentation.phase == PresentationPhase::Error {
                    BannerVariant::Error
                } else {
                    BannerVariant::Warning
                },
                icon: BannerIcon::Disconnected,
                title_template: if presentation.awaiting_approval {
                    "{environment} is waiting to approve this device"
                } else if reconnecting {
                    "{environment} is reconnecting"
                } else {
                    "{environment} is offline"
                },
                title_arguments: BTreeMap::from([("environment".into(), entry.label.clone())]),
                title: if presentation.awaiting_approval {
                    format!("{} is waiting to approve this device", entry.label)
                } else {
                    format!(
                        "{} is {}",
                        entry.label,
                        if reconnecting {
                            "reconnecting"
                        } else {
                            "offline"
                        }
                    )
                },
                description: if presentation.awaiting_approval {
                    presentation.error
                } else {
                    self.runtime
                        .diagnosis(id, None)
                        .map(|diagnosis| diagnosis.headline)
                },
                description_is_key: false,
                actions,
                dismiss_label: None,
            });
        }
        use crate::services::updates::{ServerUpdateStage, ServerUpdateState};
        let dismissed = current_update.as_ref().is_some_and(|current| {
            self.dismissals
                .borrow()
                .contains(current, matches!(update, ServerUpdateState::Failed { .. }))
        });
        if !dismissed
            && let ServerUpdateState::Running { stage, .. }
            | ServerUpdateState::Failed { stage, .. } = &update
        {
            let failed = matches!(update, ServerUpdateState::Failed { .. });
            let label = if self.runtime.environments().len() > 1 {
                format!("{} server", entry.label)
            } else {
                "server".into()
            };
            let description = match &update {
                ServerUpdateState::Failed { message, .. } => message.clone(),
                _ => match stage {
                    ServerUpdateStage::Downloading | ServerUpdateStage::Installing => {
                        "Downloading…"
                    }
                    ServerUpdateStage::Resuming => "Restarting…",
                }
                .into(),
            };
            let can_retry = failed
                && state.client_version.as_ref().is_some_and(|version| {
                    config.is_some_and(|config| {
                        crate::view_logic::session::version::resolve_server_config_version_mismatch(Some(config),version).is_some()
                            && config.environment.capabilities.server_self_update.as_ref().is_some_and(|capability|*capability!=t3_contracts::environment::ServerSelfUpdateCapability::DesktopManaged||crate::view_logic::session::version::supports_desktop_app_update(Some(config)))
                            && self.runtime.client(id).and_then(|client|client.session()).is_some_and(|endpoint|endpoint.available::<t3_contracts::rpc::WsServerUpdateServerRpc>(&t3_contracts::server::ServerSelfUpdateInput{target_version:t3_contracts::base_schemas::TrimmedNonEmptyString(version.clone()),channel:None,target_database_migration:None}))
                    })
                });
            next.push(ComposerNotice {
                id: format!("server-version:{}", id.0),
                variant: if failed {
                    BannerVariant::Error
                } else {
                    BannerVariant::Default
                },
                icon: if failed {
                    BannerIcon::UpdateFailed
                } else {
                    BannerIcon::UpdateRunning
                },
                title_template: if failed {
                    "Could not update {server}"
                } else {
                    "Updating {server}"
                },
                title_arguments: BTreeMap::from([("server".into(), label.clone())]),
                title: format!(
                    "{} {}",
                    if failed {
                        "Could not update"
                    } else {
                        "Updating"
                    },
                    label
                ),
                description: Some(description),
                description_is_key: !failed,
                actions: if can_retry {
                    vec![BannerAction {
                        id: "update".into(),
                        label: "Retry".into(),
                    }]
                } else if !failed && can_disconnect {
                    vec![BannerAction {
                        id: "disconnect".into(),
                        label: "Disconnect server".into(),
                    }]
                } else {
                    vec![]
                },
                dismiss_label: failed.then(|| "Dismiss update notice".into()),
            });
        }
        let prior = std::mem::take(&mut state.notices);
        state.notices = next
            .into_iter()
            .map(|notice| {
                prior
                    .iter()
                    .find(|p| ***p == notice)
                    .cloned()
                    .unwrap_or_else(|| Rc::new(notice))
            })
            .collect();
        state.notices.clone()
    }
    pub fn changes(&self) -> LocalBoxStream<'static, ()> {
        let mut sources = vec![
            self.runtime.changes(),
            self.state.borrow().changes.subscribe(),
        ];
        if let Some(environment) = self.target.environment() {
            sources.push(
                self.runtime
                    .composer_server_updates()
                    .state(environment.clone())
                    .observe(self.platform.monotonic_ms())
                    .map(|_| ())
                    .boxed_local(),
            );
        }
        let changes = futures::stream::select_all(sources).boxed_local();
        futures::stream::unfold((self.clone(),changes,true),|(this,mut changes,first)|async move {
            this.snapshot();
            if first { return Some(((),(this,changes,false))); }
            let deadline = {
                let state=this.state.borrow();let now=this.platform.monotonic_ms();
                [state.reconnect_since.map(|start|start.saturating_add(crate::view_logic::chat_view::ENVIRONMENT_RECONNECT_WARNING_GRACE_MS)),state.unavailable_since.map(|start|start.saturating_add(20_000))].into_iter().flatten().filter(|deadline|*deadline>now).min()
            };
            if let Some(deadline)=deadline {
                let timer=this.platform.sleep(deadline.saturating_sub(this.platform.monotonic_ms()));
                if let futures::future::Either::Left((None,_))=futures::future::select(changes.next(),timer).await {return None;}
            } else {changes.next().await?;}
            Some(((),(this,changes,false)))
        }).boxed_local()
    }
    pub fn invoke(
        &self,
        notice_id: &str,
        action_id: &str,
    ) -> LocalBoxFuture<'static, Result<BannerOutcome, ClientError>> {
        let this = self.clone();
        let notice_id = notice_id.to_owned();
        let action_id = action_id.to_owned();
        async move {
            if !this.snapshot().iter().any(|notice| {
                notice.id == notice_id
                    && (notice.actions.iter().any(|action| action.id == action_id)
                        || action_id == "dismiss" && notice.dismiss_label.is_some())
            }) {
                return Err(ClientError::Cancelled);
            }
            let id = this
                .target
                .environment()
                .ok_or(ClientError::EnvironmentMismatch)?;
            match action_id.as_str() {
                "reconnect" => {
                    this.runtime
                        .set_composer_environment_enabled(id, true)
                        .await?;
                    this.runtime.reconnect(id).await?;
                    Ok(BannerOutcome::Unchanged)
                }
                "disconnect" => {
                    this.runtime
                        .set_composer_environment_enabled(id, false)
                        .await?;
                    Ok(BannerOutcome::NavigateHome)
                }
                "dismiss" => {
                    let update = this
                        .runtime
                        .composer_server_updates()
                        .state(id.clone())
                        .snapshot()
                        .value;
                    if let Some(update) = update {
                        this.dismissals.borrow_mut().dismiss(
                            &update,
                            matches!(
                                update.as_ref(),
                                crate::services::updates::ServerUpdateState::Failed { .. }
                            ),
                        );
                    }
                    this.state.borrow().changes.notify();
                    Ok(BannerOutcome::Unchanged)
                }
                "update" => {
                    let version = this
                        .state
                        .borrow()
                        .client_version
                        .clone()
                        .ok_or(ClientError::Unsupported("client version"))?;
                    this.runtime
                        .update_composer_server(
                            id,
                            t3_contracts::server::ServerSelfUpdateInput {
                                target_version: t3_contracts::base_schemas::TrimmedNonEmptyString(
                                    version,
                                ),
                                channel: None,
                                target_database_migration: None,
                            },
                        )
                        .await?;
                    Ok(BannerOutcome::Unchanged)
                }
                _ => Err(ClientError::Cancelled),
            }
        }
        .boxed_local()
    }
}
