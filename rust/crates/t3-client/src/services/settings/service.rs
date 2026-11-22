//! The settings pages' composed client service: the web's `useSettingsScope`
//! (SettingsScopeContext.tsx), `useScopedSettings`, `useUpdateScopedSettings` and
//! `useClearScopedSettings` (useScopedSettings.ts), `SettingsRow`'s write availability
//! (settingsLayout.tsx) and `persistClientSettingsPatch` (hooks/useSettings.ts).
//!
//! Client settings live in the one client-settings record (`t3code:client-settings:v1`) that
//! `ClientRuntime::composer_preferences` already owns; this service never keeps a second copy.
pub mod project_actions;
pub mod source_control;

use super::{
    panels::{LAST_ENABLED_PROJECT_GROUPING_MODE_KEY, last_enabled_project_grouping_mode},
    scope::{
        ResolvedSettingsScope, ScopeEnvironment, ScopeGroup, SettingsScopeSearch,
        resolve_settings_scope, scope_groups, select_single_environment_scope,
    },
    scoped::{
        ProjectOverrideEntry, ScopedEnvironments, ScopedSettingsPlan, ScopedSettingsTarget,
        SettingsPatch, SettingsText, dropped_setting_keys, plan_project_overrides_clear,
        plan_scoped_settings_clear, plan_scoped_settings_patch,
        required_scopes_for_server_settings_patch, resolve_scoped_settings_targets,
        select_scoped_settings_environments,
    },
    search::{is_device_only_settings_page, is_single_environment_settings_page},
};
use crate::{
    ClientError,
    composition::ClientRuntime,
    connection::ConnectionMode,
    navigation::SettingsPage,
    platform::{Platform, PlatformError, StorageArea, StorageKey, StorageWrite},
    rpc::RpcEndpoint,
    services::resources::LocalLimiter,
    types::EnvironmentId,
    workspace::{
        projects::{EnvironmentProject, GroupingMode, GroupingSettings, PhysicalProjectKey},
        sidebar::build_sidebar_snapshots,
    },
};
use futures::{FutureExt, StreamExt, future::LocalBoxFuture, stream::LocalBoxStream};
use serde_json::{Value, json};
use std::{cell::RefCell, collections::BTreeMap, rc::Rc};
use t3_contracts::{
    auth::AuthEnvironmentScope,
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
                    == self
                        .selected
                        .representative
                        .as_ref()
                        .map(|e| &e.environment_id)
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
    Denied {
        labels: String,
    },
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
    generators: Rc<super::models::GeneratorLists>,
    listeners: Rc<RefCell<Vec<futures::channel::mpsc::UnboundedSender<()>>>>,
}

/// Reached through [`ClientRuntime::settings`].
#[derive(Clone)]
pub struct SettingsService<P: Platform + Clone + 'static> {
    runtime: ClientRuntime<P>,
    platform: Rc<P>,
    spawn: crate::composition::Spawner,
    state: Rc<SettingsState>,
}

const SAVE_FAILED: &str = "The save failed. Try reconnecting and saving again.";
const MISSING_PERMISSION: &str = "This connection lacks permission to change these settings.";
const DROPPED: &str = "This environment does not support this setting. Update T3 Code on it.";

enum DevicePreferenceFormat {
    Text,
    Boolean,
    ThemeLibrary,
    ThemeHalves,
}

fn device_preference_format(key: &str) -> Option<DevicePreferenceFormat> {
    match key {
        "t3code:theme" | "t3code:theme-appearance-mode" => Some(DevicePreferenceFormat::Text),
        "t3code:themes:v1" => Some(DevicePreferenceFormat::ThemeLibrary),
        "t3code:theme-halves:v1" => Some(DevicePreferenceFormat::ThemeHalves),
        "t3code:theme-follow-system" | "t3code:typography-advanced" => {
            Some(DevicePreferenceFormat::Boolean)
        }
        _ => None,
    }
}

fn imported_theme_key(key: &str) -> Option<StorageKey> {
    let kind = match key {
        "t3code:themes:v1" => "theme-library",
        "t3code:theme"
        | "t3code:theme-appearance-mode"
        | "t3code:theme-halves:v1"
        | "t3code:theme-follow-system" => "preferences.web",
        _ => return None,
    };
    Some(StorageKey::new(kind, None, key))
}

fn invalid_preference(key: &str) -> PlatformError {
    PlatformError::InvalidData(format!("invalid value for {key}"))
}

impl DevicePreferenceFormat {
    fn accepts(&self, value: &Value) -> bool {
        match self {
            Self::Text => value.is_string(),
            Self::Boolean => value.is_boolean(),
            Self::ThemeLibrary => value.is_array(),
            Self::ThemeHalves => value.is_object(),
        }
    }
}

fn preference_write(key: &str, value: Option<Value>) -> Result<StorageWrite, PlatformError> {
    let format = device_preference_format(key).ok_or_else(|| invalid_preference(key))?;
    let bytes = match value {
        None => None,
        Some(value)
            if format.accepts(&value)
                && (!matches!(format, DevicePreferenceFormat::ThemeHalves)
                    || ["light", "dark"]
                        .iter()
                        .all(|key| value.get(*key).is_none_or(Value::is_string))) =>
        {
            Some(match value {
                Value::String(text) => text.into_bytes(),
                json => json.to_string().into_bytes(),
            })
        }
        _ => return Err(invalid_preference(key)),
    };
    Ok(StorageWrite {
        key: StorageKey::new("ui-preference", None, key),
        value: bytes,
    })
}

/// Retain the original bytes so a library edit can compare-and-swap both a device
/// record and the web-import record it shadows, without re-encoding either.
async fn read_preference<P: Platform>(
    platform: &P,
    name: &'static str,
) -> Result<(Option<Value>, Vec<crate::platform::StorageCondition>), PlatformError> {
    use crate::platform::StorageCondition;
    let format = device_preference_format(name).ok_or_else(|| invalid_preference(name))?;
    let device = StorageKey::new("ui-preference", None, name);
    let mut conditions = Vec::new();
    for key in std::iter::once(device).chain(imported_theme_key(name)) {
        let bytes = platform.get(StorageArea::Ordinary, key.clone()).await?;
        let value = match bytes.as_deref() {
            None => None,
            Some(raw) if key.namespace != "ui-preference" => {
                let payload = crate::persistence::decode_record::<Value>(raw, &key, 1)
                    .map_err(|_| invalid_preference(name))?
                    .payload;
                Some(match (name, payload) {
                    ("t3code:theme-follow-system", Value::String(s))
                        if s == "true" || s == "false" =>
                    {
                        Value::Bool(s == "true")
                    }
                    (_, value) => value,
                })
            }
            Some(raw) => Some(match format {
                DevicePreferenceFormat::Text => Value::String(
                    std::str::from_utf8(raw)
                        .map_err(|_| invalid_preference(name))?
                        .into(),
                ),
                _ => serde_json::from_slice(raw).map_err(|_| invalid_preference(name))?,
            }),
        };
        conditions.push(StorageCondition {
            area: StorageArea::Ordinary,
            key,
            expected: bytes,
        });
        if let Some(value) = value {
            return if format.accepts(&value) {
                Ok((Some(value), conditions))
            } else {
                Err(invalid_preference(name))
            };
        }
    }
    Ok((None, conditions))
}

fn grouping_mode(mode: &SidebarProjectGroupingMode) -> GroupingMode {
    match serde_json::to_value(mode)
        .ok()
        .as_ref()
        .and_then(Value::as_str)
    {
        Some("repository_path") => GroupingMode::RepositoryPath,
        Some("separate") => GroupingMode::Separate,
        _ => GroupingMode::Repository,
    }
}

impl<P: Platform + Clone + 'static> SettingsService<P> {
    pub fn new(
        runtime: ClientRuntime<P>,
        platform: Rc<P>,
        spawn: crate::composition::Spawner,
    ) -> Self {
        Self {
            runtime,
            platform,
            spawn,
            state: Rc::default(),
        }
    }

    pub(super) fn generator_lists(&self) -> Rc<super::models::GeneratorLists> {
        self.state.generators.clone()
    }

    /// Tells every `changes()` subscriber that a read may answer differently.
    pub(super) fn notifier(&self) -> impl Fn() + 'static {
        let listeners = self.state.listeners.clone();
        move || {
            listeners
                .borrow_mut()
                .retain(|listener| listener.unbounded_send(()).is_ok());
        }
    }

    pub(super) fn spawn(&self, future: LocalBoxFuture<'static, ()>) {
        (self.spawn)(future);
    }

    pub(super) fn session_for(
        &self,
        environment: &EnvironmentId,
    ) -> Option<crate::rpc::SessionEndpoint> {
        self.session(environment)
    }

    fn grouping_key() -> StorageKey {
        StorageKey::new(
            "ui-preference",
            None,
            LAST_ENABLED_PROJECT_GROUPING_MODE_KEY,
        )
    }

    /// `readLastEnabledProjectGroupingMode`: "repository" when unset or unreadable.
    pub fn last_enabled_grouping_mode(&self) -> LocalBoxFuture<'static, &'static str> {
        let platform = self.platform.clone();
        async move {
            let stored = platform
                .get(StorageArea::Ordinary, Self::grouping_key())
                .await
                .ok()
                .flatten();
            last_enabled_project_grouping_mode(stored.as_deref())
        }
        .boxed_local()
    }

    /// `rememberEnabledProjectGroupingMode`: failures are ignored, as the web's are.
    pub fn remember_enabled_grouping_mode(&self, mode: &str) -> LocalBoxFuture<'static, ()> {
        let platform = self.platform.clone();
        let mode = mode.to_owned();
        async move {
            if mode == "separate" {
                return;
            }
            let _ = platform
                .write(
                    StorageArea::Ordinary,
                    vec![StorageWrite {
                        key: Self::grouping_key(),
                        value: Some(mode.into_bytes()),
                    }],
                )
                .await;
        }
        .boxed_local()
    }

    /// Preferences stored separately by the web, such as the theme and separate fonts.
    pub fn read_device_preference(
        &self,
        key: &'static str,
    ) -> LocalBoxFuture<'static, Option<Value>> {
        let platform = self.platform.clone();
        async move { read_preference(platform.as_ref(), key).await.ok()?.0 }.boxed_local()
    }

    /// Captures the bytes read in the device and imported namespaces. A later edit can
    /// atomically compare these records before updating the selection and library.
    pub async fn read_theme_preference(
        &self,
        key: &'static str,
    ) -> Result<(Option<Value>, Vec<crate::platform::StorageCondition>), PlatformError> {
        if imported_theme_key(key).is_none() {
            return Err(invalid_preference(key));
        }
        read_preference(self.platform.as_ref(), key).await
    }

    pub fn write_device_preference(
        &self,
        key: &'static str,
        value: Value,
    ) -> LocalBoxFuture<'static, Result<(), PlatformError>> {
        let platform = self.platform.clone();
        async move {
            let write = preference_write(key, Some(value))?;
            platform.write(StorageArea::Ordinary, vec![write]).await
        }
        .boxed_local()
    }

    /// A whole-theme pick clears the mix; importing also changes the library. Only publish
    /// these together after every record validates and the storage transaction succeeds.
    pub fn write_theme_preferences(
        &self,
        changes: Vec<(&'static str, Option<Value>)>,
        conditions: Vec<crate::platform::StorageCondition>,
    ) -> LocalBoxFuture<'static, Result<bool, PlatformError>> {
        let platform = self.platform.clone();
        async move {
            let mut writes = changes
                .into_iter()
                .map(|(key, value)| {
                    if !matches!(
                        key,
                        "t3code:theme"
                            | "t3code:theme-appearance-mode"
                            | "t3code:theme-halves:v1"
                            | "t3code:themes:v1"
                    ) {
                        return Err(invalid_preference(key));
                    }
                    preference_write(key, value)
                })
                .collect::<Result<Vec<_>, _>>()?;
            // A cleared mix must not resurrect the record imported from web storage.
            let retired = writes
                .iter()
                .filter(|write| write.value.is_none())
                .filter_map(|write| imported_theme_key(&write.key.record))
                .map(|key| StorageWrite { key, value: None })
                .collect::<Vec<_>>();
            writes.extend(retired);
            platform
                .transaction(crate::platform::StorageTransaction {
                    conditions,
                    ordinary: writes,
                    secure: Vec::new(),
                })
                .await
        }
        .boxed_local()
    }

    /// Changes to device themes, including records imported from web storage.
    pub fn theme_preference_changes(&self) -> LocalBoxStream<'static, ()> {
        self.platform
            .events()
            .filter_map(|event| async move {
                let crate::platform::PlatformEvent::StorageChanged(changed) = event else {
                    return None;
                };
                let imported = imported_theme_key(&changed.record)?;
                (changed == imported
                    || (changed.namespace == "ui-preference" && changed.environment.is_none()))
                .then_some(())
            })
            .boxed_local()
    }

    /// Fires whenever any read below may answer differently. Coalesce and re-read.
    /// Thread streams are not read here, so their changes are skipped.
    pub fn changes(&self) -> LocalBoxStream<'static, ()> {
        use crate::store::Scope;
        let (sender, receiver) = futures::channel::mpsc::unbounded();
        self.state.listeners.borrow_mut().push(sender);
        futures::stream::select_all(vec![
            receiver.boxed_local(),
            self.runtime.changes(),
            self.runtime
                .store()
                .subscribe()
                .filter_map(|change| async move {
                    change
                        .scopes
                        .iter()
                        .any(|scope| !matches!(scope, Scope::Thread(_)))
                        .then_some(())
                })
                .boxed_local(),
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
            .find(|entry| {
                entry
                    .routes
                    .iter()
                    .any(|route| route.mode == ConnectionMode::Local)
            })
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

    /// `useEnvironmentsWithScope(…, AuthSettingsWriteScope)`: a confirmed settings grant.
    fn can_write_settings(&self, environment: &EnvironmentId) -> bool {
        self.session(environment)
            .and_then(|session| session.grants_scope(AuthEnvironmentScope::SettingsWrite))
            == Some(true)
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
        self.run(plan_scoped_settings_clear(
            &view.scope,
            &view.selected.environments,
            keys,
        ))
    }

    /// `useClearProjectOverrides()(entries, keys)`.
    pub fn clear_project_overrides(
        &self,
        view: &SettingsScopeView,
        entries: &[ProjectOverrideEntry],
        keys: &[&str],
    ) -> LocalBoxFuture<'static, SettingsUpdateOutcome> {
        self.run(plan_project_overrides_clear(
            &view.selected.environments,
            entries,
            keys,
        ))
    }

    /// `updateClientSettings(patch)`: a device setting outside any scope.
    pub fn update_client(
        &self,
        patch: SettingsPatch,
    ) -> LocalBoxFuture<'static, Result<(), ClientError>> {
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
                description.push(SettingsText::with(
                    "Saved on {labels}.",
                    "labels",
                    saved.join(", "),
                ));
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
            let input = serde_json::from_value(json!({ "patch": &patch }))
                .map_err(|error| ClientError::from(error).to_string())?;
            let _permit = lane.await;
            let session = this
                .session(&environment)
                .ok_or_else(|| ClientError::Disconnected.to_string())?;
            // The contract leaves `server.updateSettings` unguarded; the web checks the scopes
            // the patch needs (`requiredScopesForServerSettingsPatch`) before sending.
            let granted = required_scopes_for_server_settings_patch(&patch)
                .into_iter()
                .all(|scope| session.grants_scope(scope) == Some(true));
            if !granted || !session.available::<WsServerUpdateSettingsRpc>(&input) {
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
