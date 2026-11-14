//! The settings pages' composed client service: the web's `useSettingsScope`
//! (SettingsScopeContext.tsx), `useScopedSettings`, `useUpdateScopedSettings` and
//! `useClearScopedSettings` (useScopedSettings.ts), `SettingsRow`'s write availability
//! (settingsLayout.tsx) and `persistClientSettingsPatch` (hooks/useSettings.ts).
//!
//! Client settings live in the one client-settings record (`t3code:client-settings:v1`) that
//! `ClientRuntime::composer_preferences` already owns; this service never keeps a second copy.
use super::{
    scope::{
        ResolvedSettingsScope, ScopeEnvironment, ScopeGroup, SettingsScopeSearch,
        resolve_settings_scope, scope_groups, select_single_environment_scope,
    },
    scoped::{
        ProjectOverrideEntry, ScopedEnvironments, ScopedSettingsPlan, ScopedSettingsTarget,
        SettingsPatch, SettingsText, dropped_setting_keys, plan_project_overrides_clear,
        plan_scoped_settings_clear, plan_scoped_settings_patch, resolve_scoped_settings_targets,
        select_scoped_settings_environments,
    },
    search::{is_device_only_settings_page, is_single_environment_settings_page},
};
use crate::{
    ClientError,
    composition::ClientRuntime,
    connection::ConnectionMode,
    platform::Platform,
    rpc::RpcEndpoint,
    services::resources::LocalLimiter,
    types::EnvironmentId,
    workspace::{
        projects::{EnvironmentProject, GroupingMode, GroupingSettings, PhysicalProjectKey},
        sidebar::build_sidebar_snapshots,
    },
    navigation::SettingsPage,
};
use futures::{FutureExt, StreamExt, future::LocalBoxFuture, stream::LocalBoxStream};
use serde_json::{Value, json};
use std::{cell::RefCell, collections::BTreeMap, rc::Rc};
use t3_contracts::{
    rpc::WsServerUpdateSettingsRpc,
    settings::{ClientSettingsSchema, ServerSettings, SidebarProjectGroupingMode},
};

/// What `useResolvedSettingsScope` returns for one page and address.
#[derive(Debug, Clone, PartialEq)]
pub struct SettingsScopeView {
    pub page: SettingsPage,
    /// The effective axes: a single-environment page fills in `machine`.
    pub search: SettingsScopeSearch,
    pub scope: ResolvedSettingsScope,
    pub groups: Vec<ScopeGroup>,
    /// Every known environment, for labels and pickers.
    pub all_environments: Vec<ScopeEnvironment>,
    pub selected: ScopedEnvironments,
    pub targets: Vec<ScopedSettingsTarget>,
    pub single_environment: bool,
    pub device_only: bool,
    pub primary: Option<EnvironmentId>,
}

impl SettingsScopeView {
    /// `target`: the representative's target, else the first.
    pub fn target(&self) -> Option<&ScopedSettingsTarget> {
        self.targets
            .iter()
            .find(|candidate| {
                Some(&candidate.environment_id)
                    == self.selected.representative.as_ref().map(|e| &e.environment_id)
            })
            .or_else(|| self.targets.first())
    }
    /// The server half of `useScopedSettings`: the target's settings, else the defaults.
    pub fn server_settings(&self) -> Rc<ServerSettings> {
        self.target()
            .map(|target| target.settings.clone())
            .unwrap_or_else(|| Rc::new(ServerSettings::default()))
    }
}

/// `SettingsRow`'s reasons a server-scoped control is inert, in the web's precedence after an
/// administrator's lock.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SettingsWriteAccess {
    Writable,
    /// `This connection lacks permission to change settings on {labels}.`
    Denied { labels: String },
    /// `Reconnect the selected environment to change this setting.`
    Disconnected,
}

impl SettingsWriteAccess {
    pub fn text(&self) -> Option<SettingsText> {
        match self {
            Self::Writable => None,
            Self::Denied { labels } => Some(SettingsText::with(
                "This connection lacks permission to change settings on {labels}.",
                "labels",
                labels.clone(),
            )),
            Self::Disconnected => Some(SettingsText::key(
                "Reconnect the selected environment to change this setting.",
            )),
        }
    }
}

/// The toast a settings save produces (`toastManager.add` in useScopedSettings.ts).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SettingsToast {
    pub error: bool,
    pub title: SettingsText,
    pub description: Vec<SettingsText>,
}

/// The result of one scoped save. `failed` means `onFailed` would run.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SettingsUpdateOutcome {
    pub failed: bool,
    pub toast: Option<SettingsToast>,
    pub saved_environments: Vec<String>,
}

#[derive(Default)]
struct SettingsState {
    lanes: RefCell<BTreeMap<EnvironmentId, LocalLimiter>>,
}

/// Reached through [`ClientRuntime::settings`].
#[derive(Clone)]
pub struct SettingsService<P: Platform + Clone + 'static> {
    runtime: ClientRuntime<P>,
    state: Rc<SettingsState>,
}

const SAVE_FAILED: &str = "The save failed. Try reconnecting and saving again.";
const MISSING_PERMISSION: &str = "This connection lacks permission to change these settings.";
const DROPPED: &str = "This environment does not support this setting. Update T3 Code on it.";

fn grouping_mode(mode: &SidebarProjectGroupingMode) -> GroupingMode {
    match serde_json::to_value(mode).ok().as_ref().and_then(Value::as_str) {
        Some("repository_path") => GroupingMode::RepositoryPath,
        Some("separate") => GroupingMode::Separate,
        _ => GroupingMode::Repository,
    }
}

impl<P: Platform + Clone + 'static> SettingsService<P> {
    pub fn new(runtime: ClientRuntime<P>) -> Self {
        Self {
            runtime,
            state: Rc::default(),
        }
    }

    /// Fires whenever any read below may answer differently. Coalesce and re-read.
    pub fn changes(&self) -> LocalBoxStream<'static, ()> {
        futures::stream::select_all(vec![
            self.runtime.changes(),
            self.runtime.store().subscribe().map(|_| ()).boxed_local(),
            self.runtime.composer_preferences().changes(),
        ])
        .boxed_local()
    }

    /// `useClientSettings()`: defaults until the saved record is read.
    pub fn client_settings(&self) -> Rc<ClientSettingsSchema> {
        self.runtime.composer_preferences().snapshot()
    }

    /// The saved environments as settings pages present them.
    pub fn environments(&self) -> Vec<ScopeEnvironment> {
        let snapshot = self.runtime.store().snapshot();
        self.runtime
            .environments()
            .into_iter()
            .map(|entry| ScopeEnvironment {
                display_url: entry
                    .routes
                    .iter()
                    .find(|route| route.mode != ConnectionMode::Relay)
                    .map(|route| route.http_base.clone()),
                config: snapshot
                    .environments
                    .get(&entry.id)
                    .and_then(|state| state.config.clone()),
                phase: entry.state.phase,
                label: entry.label,
                environment_id: entry.id,
            })
            .collect()
    }

    /// `usePrimaryEnvironmentId`: the environment reached through the primary (local) target.
    pub fn primary_environment(&self) -> Option<EnvironmentId> {
        self.runtime
            .environments()
            .into_iter()
            .find(|entry| entry.routes.iter().any(|route| route.mode == ConnectionMode::Local))
            .map(|entry| entry.id)
    }

    /// `useSettingsProjectGroups`.
    pub fn groups(&self) -> Vec<ScopeGroup> {
        let snapshot = self.runtime.store().snapshot();
        let client = self.client_settings();
        let settings = GroupingSettings {
            mode: grouping_mode(&client.sidebar_project_grouping_mode),
            overrides: client
                .sidebar_project_grouping_overrides
                .iter()
                .filter_map(|(key, mode)| {
                    let (environment, path) = key.0.split_once(':')?;
                    Some((
                        PhysicalProjectKey::new(EnvironmentId(environment.into()), path),
                        grouping_mode(mode),
                    ))
                })
                .collect(),
        };
        let labels: BTreeMap<EnvironmentId, String> = self
            .runtime
            .environments()
            .into_iter()
            .map(|entry| (entry.id, entry.label))
            .collect();
        let projects: Vec<EnvironmentProject> = snapshot
            .environments
            .iter()
            .flat_map(|(environment, state)| {
                let scratch = state
                    .config
                    .as_ref()
                    .and_then(|c| c.scratch_workspace_root.as_ref())
                    .map(|root| crate::workspace::projects::normalize_project_path_for_comparison(&root.0));
                state.shell.snapshot.iter().flat_map(move |shell| {
                    let scratch = scratch.clone();
                    shell.projects.iter().map(move |project| EnvironmentProject {
                        environment: environment.clone(),
                        is_scratch: scratch.as_ref().is_some_and(|root| {
                            *root
                                == crate::workspace::projects::normalize_project_path_for_comparison(
                                    &project.workspace_root.0,
                                )
                        }),
                        project: project.clone(),
                    })
                })
            })
            .collect();
        let primary = self.primary_environment();
        scope_groups(&build_sidebar_snapshots(
            &projects,
            &settings,
            false,
            primary.as_ref(),
            |id| labels.get(id).cloned(),
            |_| false,
            |_| false,
        ))
    }

    /// `useResolvedSettingsScope(search, singleEnvironment)` for `page`.
    pub fn scope_view(&self, page: SettingsPage, raw: &SettingsScopeSearch) -> SettingsScopeView {
        let groups = self.groups();
        let environments = self.environments();
        let primary = self.primary_environment();
        let single_environment = is_single_environment_settings_page(page);
        let search = if single_environment {
            let scope = resolve_settings_scope(raw, &groups, &environments);
            select_single_environment_scope(raw, &scope, &environments, primary.as_ref())
        } else {
            raw.clone()
        };
        let scope = resolve_settings_scope(&search, &groups, &environments);
        let selected = select_scoped_settings_environments(&scope, &environments, primary.as_ref());
        let targets = resolve_scoped_settings_targets(&scope, &selected.connected);
        SettingsScopeView {
            page,
            search,
            scope,
            groups,
            all_environments: environments,
            selected,
            targets,
            single_environment,
            device_only: is_device_only_settings_page(page),
            primary,
        }
    }

    fn session(&self, environment: &EnvironmentId) -> Option<crate::rpc::SessionEndpoint> {
        self.runtime.client(environment).and_then(|c| c.session())
    }

    fn can_write_settings(&self, environment: &EnvironmentId) -> bool {
        let probe = serde_json::from_value(json!({"patch": {}}));
        match (self.session(environment), probe) {
            (Some(session), Ok(probe)) => session.available::<WsServerUpdateSettingsRpc>(&probe),
            _ => false,
        }
    }

    /// `SettingsRow`'s `canWriteSettings` and connection checks for a server-scoped row.
    pub fn write_access(&self, view: &SettingsScopeView) -> SettingsWriteAccess {
        let connected = &view.selected.connected;
        let denied: Vec<&str> = connected
            .iter()
            .filter(|target| !self.can_write_settings(&target.environment_id))
            .map(|target| target.label.as_str())
            .collect();
        if connected.is_empty() {
            // canWriteSettings is false with no connected target; the web names the selection.
            return SettingsWriteAccess::Denied {
                labels: "the selected environment".into(),
            };
        }
        if !denied.is_empty() {
            return SettingsWriteAccess::Denied {
                labels: denied.join(", "),
            };
        }
        SettingsWriteAccess::Writable
    }

    /// `useUpdateScopedSettings()(patch)`.
    pub fn update(
        &self,
        view: &SettingsScopeView,
        patch: SettingsPatch,
    ) -> LocalBoxFuture<'static, SettingsUpdateOutcome> {
        let plan = plan_scoped_settings_patch(
            &view.scope,
            &view.selected.environments,
            &patch,
            view.device_only,
        );
        self.run(plan)
    }

    /// `useClearScopedSettings()(keys)`.
    pub fn clear(
        &self,
        view: &SettingsScopeView,
        keys: &[&str],
    ) -> LocalBoxFuture<'static, SettingsUpdateOutcome> {
        self.run(plan_scoped_settings_clear(&view.scope, &view.selected.environments, keys))
    }

    /// `useClearProjectOverrides()(entries, keys)`.
    pub fn clear_project_overrides(
        &self,
        view: &SettingsScopeView,
        entries: &[ProjectOverrideEntry],
        keys: &[&str],
    ) -> LocalBoxFuture<'static, SettingsUpdateOutcome> {
        self.run(plan_project_overrides_clear(&view.selected.environments, entries, keys))
    }

    /// `updateClientSettings(patch)`: a device setting outside any scope.
    pub fn update_client(&self, patch: SettingsPatch) -> LocalBoxFuture<'static, Result<(), ClientError>> {
        let preferences = self.runtime.composer_preferences();
        async move { persist_client_patch(&preferences, patch).await }.boxed_local()
    }

    /// `useRunScopedPlan`.
    fn run(&self, plan: ScopedSettingsPlan) -> LocalBoxFuture<'static, SettingsUpdateOutcome> {
        let this = self.clone();
        async move {
            if let Some(reason) = plan.unavailable_reason {
                return SettingsUpdateOutcome {
                    failed: true,
                    toast: Some(SettingsToast {
                        error: false,
                        title: SettingsText::key("Setting not saved"),
                        description: vec![reason],
                    }),
                    saved_environments: Vec::new(),
                };
            }
            let preferences = this.runtime.composer_preferences();
            let client = plan.client_patch.clone();
            let client_write = async move {
                // The web logs a failed device save and shows no toast; the record rolls back.
                if !client.is_empty() {
                    let _ = persist_client_patch(&preferences, client).await;
                }
            };
            let server_writes = futures::future::join_all(plan.server_writes.iter().map(|write| {
                let this = this.clone();
                let write = write.clone();
                async move {
                    let result = this.save(&write.environment_id, write.patch.clone()).await;
                    (write, result)
                }
            }));
            let ((), results) = futures::join!(client_write, server_writes);
            let mut failed = Vec::new();
            let mut saved = Vec::new();
            for (write, result) in results {
                match result {
                    Ok(value) if dropped_setting_keys(&write, &value).is_empty() => {
                        saved.push(write.label)
                    }
                    Ok(_) => failed.push((write.label, DROPPED.to_owned())),
                    Err(message) => failed.push((write.label, message)),
                }
            }
            if failed.is_empty() {
                return SettingsUpdateOutcome {
                    failed: false,
                    toast: None,
                    saved_environments: saved,
                };
            }
            let mut description: Vec<SettingsText> = failed
                .into_iter()
                .map(|(label, message)| SettingsText {
                    key: "Could not save on {label}: {message}",
                    args: vec![("label", label), ("message", message)],
                })
                .collect();
            if !saved.is_empty() {
                description.push(SettingsText::with("Saved on {labels}.", "labels", saved.join(", ")));
            }
            SettingsUpdateOutcome {
                failed: true,
                toast: Some(SettingsToast {
                    error: true,
                    title: SettingsText::key(if saved.is_empty() {
                        "Setting not saved"
                    } else {
                        "Setting saved on some environments"
                    }),
                    description,
                }),
                saved_environments: saved,
            }
        }
        .boxed_local()
    }

    /// One `server.updateSettings` on `environment`, serialised with that environment's other
    /// config writes. The error string is the message the toast shows.
    fn save(
        &self,
        environment: &EnvironmentId,
        patch: SettingsPatch,
    ) -> LocalBoxFuture<'static, Result<Value, String>> {
        let lane = self
            .state
            .lanes
            .borrow_mut()
            .entry(environment.clone())
            .or_insert_with(|| LocalLimiter::new(1))
            .acquire();
        let this = self.clone();
        let environment = environment.clone();
        async move {
            let input = serde_json::from_value(json!({ "patch": patch }))
                .map_err(|error| ClientError::from(error).to_string())?;
            let _permit = lane.await;
            let session = this
                .session(&environment)
                .ok_or_else(|| ClientError::Disconnected.to_string())?;
            if !session.available::<WsServerUpdateSettingsRpc>(&input) {
                return Err(MISSING_PERMISSION.to_owned());
            }
            match session.command::<WsServerUpdateSettingsRpc>(input).await {
                Ok(saved) => serde_json::to_value(saved).map_err(|_| SAVE_FAILED.to_owned()),
                Err(error) => Err(match error.into_client_error() {
                    ClientError::Rejected { message, detail } => {
                        crate::error::rejection_text(&message, detail.as_deref())
                    }
                    ClientError::Unsupported(_) | ClientError::PermissionDenied { .. } => {
                        MISSING_PERMISSION.to_owned()
                    }
                    _ => SAVE_FAILED.to_owned(),
                }),
            }
        }
        .boxed_local()
    }
}

/// `persistClientSettingsPatch`: the patch over the saved record, read first so defaults never
/// overwrite preferences that were not loaded yet.
async fn persist_client_patch<P: Platform + 'static>(
    preferences: &crate::thread_views::composer::preferences::ComposerPreferences<P>,
    patch: SettingsPatch,
) -> Result<(), ClientError> {
    preferences.load().await?;
    let mut decoded = Ok(());
    preferences
        .update(|settings| {
            let mut value = serde_json::to_value(&*settings).unwrap_or_default();
            if let Some(fields) = value.as_object_mut() {
                fields.extend(patch);
            }
            match serde_json::from_value::<ClientSettingsSchema>(value) {
                Ok(next) => *settings = next,
                Err(error) => decoded = Err(ClientError::from(error)),
            }
        })
        .await?;
    decoded
}

impl<P: Platform + Clone + 'static> ClientRuntime<P> {
    /// The settings pages' service.
    pub fn settings(&self) -> SettingsService<P> {
        SettingsService::new(self.clone())
    }
}
