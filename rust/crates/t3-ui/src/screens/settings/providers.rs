//! Providers settings for the environment selected by the settings route.

use std::{collections::BTreeSet, rc::Rc};

use futures::{FutureExt, StreamExt, future::LocalBoxFuture, stream::LocalBoxStream};
use nsh_core::prelude::*;
use serde_json::{Value, json};
use t3_client::{
    ClientError,
    composition::{ClientRuntime, first_run::SetupTerminalState},
    platform::Platform,
    services::{
        providers::{
            ProviderSettings, ProviderSettingsRow, ProviderStatusTone, provider_driver_allowed,
            provider_override_patch, provider_settings_rows, provider_settings_status,
        },
        settings::{scope::ScopeKind, scoped::SettingsPatch, service::SettingsScopeView},
        terminal_sessions::TerminalLaunch,
    },
    types::{EnvironmentId, ThreadId, ThreadKey},
    view_logic::{provider_readiness, shell::models},
};
use t3_contracts::{
    acp_registry::{
        AcpRegistryDeleteSessionInput, AcpRegistryImportSessionInput, AcpRegistryListSessionsInput,
        AcpRegistryListSessionsResult,
    },
    auth::AuthEnvironmentScope as Scope,
    base_schemas::ProjectId,
    model_selection::ModelSelection,
    provider_instance::{ProviderInstanceConfig, ProviderInstanceId, ProviderInstanceMutation},
    provider_setup::{
        ProviderAuthCancelInput, ProviderAuthInteraction, ProviderAuthPhase,
        ProviderAuthRespondInput, ProviderAuthResponse, ProviderAuthStartInput, ProviderAuthState,
        ProviderBrowserAuthAction, ProviderInstallCancelInput, ProviderInstallPhase,
        ProviderInstallState, ProviderSetupInput,
    },
    server::{ServerConfig, ServerProvider, ServerProviderAuthStatus},
};

use super::shell::{
    SettingsContext, SettingsRowProps, region, settings_group, settings_page_container,
    settings_row, settings_section, use_settings,
};
use crate::{
    i18n::{Text, Value as MsgValue, t, tr},
    runtime::{
        services::{links::link_opener_here, terminal::TerminalServices},
        tasks::{TaskOwner, watch},
    },
    state::url::UrlState,
    theme::{cn, tw},
    ui::{
        button::{ButtonContent, ButtonSize, ButtonVariant, button},
        confirm::{ConfirmDialogOptions, ConfirmDialogVariant, request_confirm_dialog},
        icon::{Icon, provider_icon},
        select::{SelectItem, SelectProps, Size as SelectSize, Value as SelectValue, select},
        switch::{ControlState, Size as SwitchSize, switch},
        toast::{ToastOptions, Variant, toast_manager},
    },
};

enum Command {
    Update(SettingsPatch),
    UpdateDevice(SettingsPatch),
    SetLoadWeight(t3_contracts::base_schemas::TrimmedNonEmptyString, i64),
    Instance(ProviderInstanceMutation),
    Enable(ProviderInstanceId, bool),
    Refresh,
    RefreshAfterSetup,
    DeleteSession(AcpRegistryDeleteSessionInput),
}

enum AuthCommand {
    Start(ProviderAuthStartInput),
    Respond(ProviderAuthRespondInput),
    Cancel(ProviderAuthCancelInput),
    Logout(ProviderSetupInput),
}

#[derive(Clone)]
enum InstallCommand {
    Start(ProviderSetupInput),
    Cancel(ProviderInstallCancelInput),
    Remove(ProviderSetupInput),
}

type Call<T> = LocalBoxFuture<'static, Result<T, ClientError>>;
type States<T> = LocalBoxStream<'static, Result<T, ClientError>>;
type Request<I, T = ()> = Rc<dyn Fn(EnvironmentId, I) -> Call<T>>;
type Follow<T> = Rc<dyn Fn(&EnvironmentId, ProviderSetupInput) -> States<T>>;
type Grants = Rc<dyn Fn(&EnvironmentId, Scope) -> bool>;
type OpenSetupTerminal = Rc<
    dyn Fn(
        &SetupTerminal,
    ) -> Result<(String, LocalBoxStream<'static, SetupTerminalState>), ClientError>,
>;

#[derive(Clone)]
pub(super) struct Services {
    grants: Grants,
    command: Request<Command>,
    auth: Request<AuthCommand, ProviderAuthState>,
    install: Request<InstallCommand, ProviderInstallState>,
    auth_state: Follow<ProviderAuthState>,
    install_state: Follow<ProviderInstallState>,
    setup_terminal: OpenSetupTerminal,
    terminals: TerminalServices,
    local_sign_in: Rc<dyn Fn(&EnvironmentId) -> bool>,
    managed_sign_in: Request<ProviderInstanceId, String>,
    list_sessions: Request<AcpRegistryListSessionsInput, AcpRegistryListSessionsResult>,
    import_session: Request<AcpRegistryImportSessionInput, ThreadId>,
}

impl Services {
    pub(super) fn new<P: Platform + Clone + 'static>(client: &ClientRuntime<P>) -> Self {
        let settings = ProviderSettings::new(client);
        let grants = settings.clone();
        let commands = settings.clone();
        let client_settings = client.settings();
        let first_run = client.first_run();
        let terminal_setup = first_run.clone();
        let local_auth = first_run.clone();
        let managed_auth = first_run.clone();
        let auth = settings.clone();
        let install = settings.clone();
        let auth_state = settings.clone();
        let list_sessions = settings.clone();
        let import_session = settings.clone();
        Self {
            grants: Rc::new(move |environment, scope| grants.grants(environment, scope)),
            command: Rc::new(move |environment, command| {
                let settings = commands.clone();
                let client_settings = client_settings.clone();
                let first_run = first_run.clone();
                async move {
                    match command {
                        Command::Update(patch) => settings.update(&environment, patch).await,
                        Command::UpdateDevice(patch) => client_settings.update_client(patch).await,
                        Command::SetLoadWeight(id, weight) => {
                            settings.set_load_weight(id, weight).await
                        }
                        Command::Instance(mutation) => {
                            settings.mutate_instance(&environment, mutation).await
                        }
                        Command::Enable(id, enabled) => {
                            settings.set_enabled(&environment, id, enabled).await
                        }
                        Command::Refresh => settings.refresh(&environment).await,
                        Command::RefreshAfterSetup => {
                            first_run.refresh_providers(&environment, None).await
                        }
                        Command::DeleteSession(input) => {
                            settings.delete_session(&environment, input).await
                        }
                    }
                }
                .boxed_local()
            }),
            auth: Rc::new(move |environment, command| {
                let settings = auth.clone();
                async move {
                    match command {
                        AuthCommand::Start(input) => settings.start_auth(&environment, input).await,
                        AuthCommand::Respond(input) => {
                            settings.respond_auth(&environment, input).await
                        }
                        AuthCommand::Cancel(input) => {
                            settings.cancel_auth(&environment, input).await
                        }
                        AuthCommand::Logout(input) => settings.logout(&environment, input).await,
                    }
                }
                .boxed_local()
            }),
            install: Rc::new(move |environment, command| {
                let settings = install.clone();
                async move {
                    match command {
                        InstallCommand::Start(input) => {
                            settings.start_install(&environment, input).await
                        }
                        InstallCommand::Cancel(input) => {
                            settings.cancel_install(&environment, input).await
                        }
                        InstallCommand::Remove(input) => {
                            settings.remove_install(&environment, input).await
                        }
                    }
                }
                .boxed_local()
            }),
            auth_state: Rc::new(move |environment, target| {
                auth_state.auth_state(environment, target)
            }),
            install_state: Rc::new(move |environment, target| {
                settings.install_state(environment, target)
            }),
            setup_terminal: Rc::new(move |setup| {
                let id = terminal_setup.setup_terminal_id(&setup.driver)?;
                let stream = terminal_setup.setup_terminal(
                    setup.key.clone(),
                    id.clone(),
                    setup.cwd.clone(),
                    setup.instance.clone(),
                    setup.command.clone(),
                );
                Ok((id, stream))
            }),
            terminals: TerminalServices::new(client.terminals()),
            local_sign_in: Rc::new(move |env| local_auth.local_sign_in(env)),
            managed_sign_in: Rc::new(move |env, instance| {
                let managed = managed_auth.clone();
                async move { managed.sign_in_page(&env, instance).await }.boxed_local()
            }),
            list_sessions: Rc::new(move |environment, input| {
                let service = list_sessions.clone();
                async move { service.list_sessions(&environment, input).await }.boxed_local()
            }),
            import_session: Rc::new(move |environment, input| {
                let service = import_session.clone();
                async move { service.import_session(&environment, input).await }.boxed_local()
            }),
        }
    }
}

#[derive(Clone)]
struct Page {
    services: Services,
    tasks: TaskOwner,
    selected: Signal<Option<ProviderInstanceId>>,
    refreshing: Signal<bool>,
    terminal: Signal<Option<SetupTerminal>>,
}

impl Page {
    fn request(&self, env: EnvironmentId, command: Command, error: &'static str) {
        let future = (self.services.command)(env, command);
        self.tasks.spawn(async move {
            if let Err(err) = future.await {
                show_error(error, err);
            }
        });
    }
}

fn show_error(title: &'static str, error: ClientError) {
    toast_manager().add(ToastOptions {
        variant: Variant::Error,
        title: Text::from(title),
        description: Some(Text::content(error.to_string())),
        ..Default::default()
    });
}

pub fn screen() -> AnyElement {
    let context = use_settings();
    let services = use_context::<Services>();
    let url = use_context::<UrlState>();
    let target = move || {
        url.and_then(|url| {
            let url = url.location.get();
            url.search
                .trim_start_matches('?')
                .split('&')
                .find_map(|pair| {
                    let (key, value) = pair.split_once('=')?;
                    let value = percent_encoding::percent_decode_str(value)
                        .decode_utf8()
                        .ok()?;
                    (key == "instanceId" && !value.trim().is_empty())
                        .then(|| ProviderInstanceId(value.into_owned()))
                })
        })
    };
    let page = services.map(|services| {
        Rc::new(Page {
            services,
            tasks: TaskOwner::here(),
            selected: signal(None),
            refreshing: signal(false),
            terminal: signal(None),
        })
    });
    let target = computed(target);
    let terminal = page.as_ref().map(|page| {
        let selected = page.selected;
        let setup = page.terminal;
        let page = page.clone();
        effect(move || {
            target.get();
            selected.set(None);
        });
        region(
            move || setup.get(),
            move |setup| setup.map(|setup| setup_terminal_dialog(page.clone(), setup)),
        )
    });
    settings_page_container(
        view()
            .child(region(
                move || (context.view(), target.get()),
                move |(scope, target)| {
                    let (Some(page), Some(scope)) = (page.clone(), scope) else {
                        return empty("Connect an environment to set up its providers.");
                    };
                    let Some(environment) = scope.selected.representative.as_ref() else {
                        return empty("Connect an environment to set up its providers.");
                    };
                    let Some(config) = environment.config.as_ref() else {
                        return empty("Loading provider settings");
                    };
                    environment_panel(
                        page,
                        context,
                        &scope,
                        environment.environment_id.clone(),
                        config.clone(),
                        target,
                    )
                },
            ))
            .child(terminal),
    )
    .into_element()
}

fn empty(message: &'static str) -> AnyElement {
    text(t(message))
        .app_class(tw("p-8 text-sm text-muted-foreground"))
        .into_element()
}

fn project_ids(
    scope: &SettingsScopeView,
    env: &EnvironmentId,
    config: &ServerConfig,
) -> Vec<ProjectId> {
    if !matches!(
        scope.scope.kind,
        ScopeKind::Project { .. } | ScopeKind::Checkout { .. }
    ) || config
        .environment
        .capabilities
        .project_provider_instance_overrides
        != Some(true)
    {
        return Vec::new();
    }
    scope
        .scope
        .members
        .iter()
        .filter(|member| &member.environment_id == env)
        .map(|member| member.id.clone())
        .collect()
}

fn environment_panel(
    page: Rc<Page>,
    context: SettingsContext,
    scope: &SettingsScopeView,
    env: EnvironmentId,
    config: Rc<ServerConfig>,
    target: Option<ProviderInstanceId>,
) -> AnyElement {
    let projects = project_ids(scope, &env, &config);
    let mut acp_projects = Vec::new();
    for group in &scope.groups {
        for member in group.members.iter().filter(|member| {
            member.environment_id == env
                && (!scope.scope.is_project_scope()
                    || scope
                        .scope
                        .members
                        .iter()
                        .any(|scoped| scoped.id == member.id))
        }) {
            acp_projects.push((member.id.clone(), group.display_name.clone()));
        }
    }
    let acp_projects = Rc::new(acp_projects);
    let rows = Rc::new(provider_settings_rows(&config, &projects, target.as_ref()));
    let can_manage = (page.services.grants)(&env, Scope::ProvidersManage);
    let can_write = (page.services.grants)(&env, Scope::SettingsWrite);
    let can_refresh = (page.services.grants)(&env, Scope::OrchestrationRead);
    let selected = page.selected;
    let current = computed({
        let rows = rows.clone();
        let target = target.clone();
        move || {
            let id = selected.get().or_else(|| target.clone());
            rows.iter()
                .position(|row| Some(&row.id) == id.as_ref())
                .or_else(|| (target.is_none() && !rows.is_empty()).then_some(0))
        }
    });
    let changing = page.refreshing;
    let refresh = {
        let page = page.clone();
        let env = env.clone();
        move || {
            if changing.get_untracked() {
                return;
            }
            changing.set(true);
            let future = (page.services.command)(env.clone(), Command::Refresh);
            page.tasks.spawn(async move {
                let result = future.await;
                changing.set(false);
                if let Err(error) = result {
                    show_error("Could not refresh provider status", error);
                }
            });
        }
    };
    let list = {
        let (page, rows, env, projects, config) = (
            page.clone(),
            rows.clone(),
            env.clone(),
            projects.clone(),
            config.clone(),
        );
        region(
            move || current.get(),
            move |current| {
                let mut list = view().app_class(tw("flex flex-col bg-muted/10"));
                for (index, row) in rows.iter().enumerate() {
                    list = list.child(instance_row(
                        page.clone(),
                        &env,
                        &config,
                        &projects,
                        row,
                        current == Some(index),
                        can_manage,
                        can_write,
                        index > 0,
                    ));
                }
                list
            },
        )
    };
    let details = {
        let (page, env, config) = (page.clone(), env.clone(), config.clone());
        region(
            move || current.get(),
            move |current| {
                current
                    .and_then(|index| rows.get(index))
                    .map(|row| {
                        instance_detail(
                            page.clone(),
                            context,
                            &env,
                            &config,
                            &acp_projects,
                            row,
                            can_manage,
                        )
                    })
                    .unwrap_or_else(|| {
                        empty(if target.is_some() {
                            "This provider instance is no longer available on this device."
                        } else {
                            "No providers configured."
                        })
                    })
            },
        )
    };
    let add = can_manage
        .then(|| add_provider(page.clone(), env.clone(), &config))
        .flatten();
    let settings = view()
        .app_class(tw("flex flex-col gap-2.5"))
        .child(
            view()
                .app_class(tw(
                    "flex min-h-7 flex-row items-center justify-between gap-2 px-3 sm:px-4",
                ))
                .child(
                    text(t("Providers"))
                        .role(AccessibilityRole::Header)
                        .app_class(tw("text-base font-semibold")),
                )
                .child(button(
                    ButtonContent::Icon {
                        icon: Icon::RefreshCw,
                        label: t("Refresh provider status"),
                    },
                    ButtonVariant::GhostMuted,
                    ButtonSize::IconXs,
                    move || !can_refresh || changing.get(),
                    refresh,
                )),
        )
        .child((!can_manage).then(|| {
            settings_group(settings_row(
                SettingsRowProps::titled("", "Limited permissions".into()).description(move || {
                    "This session can view providers but can't change their settings.".into()
                }),
            ))
        }))
        .child(settings_group(
            view()
                .app_class(move || {
                    tw(if context.content_width.get() >= 768.0 {
                        "flex flex-row overflow-hidden"
                    } else {
                        "flex flex-col overflow-hidden"
                    })
                })
                .child(
                    view()
                        .app_class(move || {
                            tw(if context.content_width.get() >= 768.0 {
                                "w-68 shrink-0 border-r border-border/60"
                            } else {
                                "border-b border-border/60"
                            })
                        })
                        .child(list)
                        .child(add),
                )
                .child(view().app_class(tw("min-w-0 flex-1")).child(details)),
        ));
    view()
        .app_class(tw("flex flex-col gap-8"))
        .child(settings)
        .child(default_model(context, &config, scope, can_write))
        .child(usage_sources(page.clone(), &env, &config, can_manage))
        .child({
            let (scope, env) = (scope.clone(), env.clone());
            region(
                move || context.client_settings(),
                move |_| load_balancing(page.clone(), context, &scope, &env),
            )
        })
        .into_element()
}

#[allow(clippy::too_many_arguments)]
fn instance_row(
    page: Rc<Page>,
    env: &EnvironmentId,
    config: &ServerConfig,
    projects: &[ProjectId],
    row: &ProviderSettingsRow,
    selected: bool,
    can_manage: bool,
    can_write: bool,
    divided: bool,
) -> AnyElement {
    let id = row.id.clone();
    let name = row.name.clone();
    let select_label = tr("Select {name}", &[("name", MsgValue::Text(name.clone()))]);
    let enable_label = tr("Enable {name}", &[("name", MsgValue::Text(name.clone()))]);
    let enabled = row.enabled;
    let blocked = !can_manage || (!projects.is_empty() && !can_write);
    let enable_blocked = blocked || (!enabled && !provider_driver_allowed(config, &row.driver));
    let state = provider_settings_status(row.provider.as_ref(), enabled);
    let headline = status_headline(&state);
    let tone = state.tone;
    let selection = page.selected;
    let selected_id = id.clone();
    let version = row
        .provider
        .as_ref()
        .and_then(|provider| provider.version.as_ref())
        .map(|v| {
            if v.0.starts_with(|c: char| c.is_ascii_digit()) {
                format!("v{}", v.0)
            } else {
                v.0.clone()
            }
        });
    let clear = if row.project_override.is_some() {
        let patch = provider_override_patch(&config.settings, projects, &id, None);
        let (page, env) = (page.clone(), env.clone());
        Some(action(
            "Reset to the device setting",
            ButtonVariant::GhostMuted,
            blocked,
            move || {
                page.request(
                    env.clone(),
                    Command::Update(patch.clone()),
                    "Could not update provider instance",
                );
            },
        ))
    } else {
        None
    };
    let env = env.clone();
    let options = (!projects.is_empty()).then(|| {
        (
            provider_override_patch(&config.settings, projects, &id, Some(false)),
            provider_override_patch(&config.settings, projects, &id, Some(true)),
        )
    });
    view().app_class(tw(&cn([
        Some("relative flex min-h-18 flex-row items-center gap-3 px-3 py-3 sm:px-4"),
        selected.then_some("bg-foreground/[0.06]"),
        divided.then_some("border-t border-border/50"),
    ])))
        .child(nsh_base::button()
            .accessibility_label(SharedString::from(select_label))
            .on_press(move || selection.set(Some(selected_id.clone())))
            .app_class(tw("flex min-w-0 flex-1 flex-row items-start gap-3 rounded-md text-left outline-none focus-visible:ring-2 focus-visible:ring-ring"))
            .child(provider_icon(&row.driver, &name, "size-4 shrink-0"))
            .child(view().app_class(tw("min-w-0 flex-1"))
                .child(view().app_class(tw("flex min-w-0 flex-row items-center gap-2"))
                    .child(text(name).app_class(tw("truncate text-sm font-medium text-foreground")))
                    .child(version.map(|version| text(version).app_class(tw("max-w-24 shrink-0 truncate font-mono text-xs text-muted-foreground")))))
                .child(view().app_class(tw("mt-0.5 flex flex-row items-center gap-1.5"))
                    .child(status_dot(tone))
                    .child(text(headline).app_class(tw("text-xs leading-normal text-muted-foreground/80"))))))
        .child(clear)
        .child(switch(enable_label, ControlState {
            checked: enabled.into(), disabled: enable_blocked.into(), ..Default::default()
        }, SwitchSize::Default, move |enabled| {
            let command = match &options {
                Some((off, on)) => Command::Update(if enabled { on } else { off }.clone()),
                None => Command::Enable(id.clone(), enabled),
            };
            page.request(env.clone(), command, "Could not update provider instance");
        }))
        .into_element()
}

fn status_headline(status: &t3_client::services::providers::ProviderStatus<'_>) -> String {
    let label = Text::client_key(status.headline).resolve().to_string();
    match status.auth_label {
        Some(auth) => format!("{label} · {auth}"),
        None => label,
    }
}

fn status_dot(tone: ProviderStatusTone) -> Option<AnyElement> {
    let color = match tone {
        ProviderStatusTone::Normal => return None,
        ProviderStatusTone::Warning => "size-1.5 shrink-0 rounded-full bg-warning",
        ProviderStatusTone::Error => "size-1.5 shrink-0 rounded-full bg-destructive",
    };
    Some(view().app_class(tw(color)).into_element())
}

fn action(
    label: &'static str,
    variant: ButtonVariant,
    disabled: bool,
    press: impl Fn() + 'static,
) -> AnyElement {
    button(
        ButtonContent::Label(t(label)),
        variant,
        ButtonSize::Xs,
        move || disabled,
        press,
    )
    .into_element()
}

fn default_model(
    context: SettingsContext,
    config: &ServerConfig,
    scope: &SettingsScopeView,
    can_write: bool,
) -> AnyElement {
    let settings = scope.server_settings();
    let model_settings = models::Settings {
        server: &settings,
        legacy_preferences: None,
    };
    let project = scope.target().and_then(|target| target.project_id.as_ref());
    let mut selections = vec![None];
    let mut items = vec![SelectItem::new("0", t("Automatic"))];
    for entry in models::entries_for(&config.providers.0, &settings, project) {
        if !entry.enabled || !entry.is_available {
            continue;
        }
        for model in models::options_for(&model_settings, entry.snapshot, None) {
            items.push(SelectItem::new(selections.len().to_string(), model.name));
            selections.push(Some(ModelSelection {
                instance_id: entry.snapshot.instance_id.clone(),
                model: model.slug,
                options: None,
            }));
        }
    }
    let selected = settings
        .default_model_selection
        .as_ref()
        .map_or(0, |saved| {
            selections
                .iter()
                .position(|option| {
                    option.as_ref().is_some_and(|choice| {
                        choice.instance_id == saved.instance_id && choice.model == saved.model
                    })
                })
                .unwrap_or_else(|| {
                    let index = selections.len();
                    selections.push(Some(saved.clone()));
                    items.push(SelectItem::new(index.to_string(), saved.model.clone()));
                    index
                })
        });
    settings_group(settings_row(
        SettingsRowProps::titled("provider-default-model", "Default model".into())
            .server(&["defaultModelSelection"])
            .control(move || {
                let mut props = SelectProps::new("Default model");
                props.items = items.clone().into();
                props.value = SelectValue::Controlled(Some(selected.to_string().into())).into();
                props.disabled = (!can_write).into();
                props.size = SelectSize::Sm;
                let selections = selections.clone();
                select(props, move |value| {
                    if let Some(next) = value
                        .and_then(|value| value.parse::<usize>().ok())
                        .and_then(|index| selections.get(index))
                    {
                        context.update(json!({"defaultModelSelection":next}));
                    }
                })
            }),
    ))
    .into_element()
}

fn instance_detail(
    page: Rc<Page>,
    context: SettingsContext,
    env: &EnvironmentId,
    config: &ServerConfig,
    projects: &[(ProjectId, String)],
    row: &ProviderSettingsRow,
    can_manage: bool,
) -> AnyElement {
    let state = provider_settings_status(row.provider.as_ref(), row.enabled);
    let headline = status_headline(&state);
    let detail = state
        .detail
        .map(str::to_owned)
        .or_else(|| state.fallback_detail.map(t));
    let mut section = view().app_class(tw("flex flex-col gap-6 py-4"));
    let base = config.settings.provider_instances.get(&row.id).cloned();
    let remove = can_manage
        .then(|| {
            base.as_ref().map(|instance| {
                let default = row.id
                    == t3_contracts::provider_instance::default_instance_id_for_driver(
                        &instance.driver,
                    );
                let (page, env, id, name) =
                    (page.clone(), env.clone(), row.id.clone(), row.name.clone());
                let label = if default {
                    "Reset provider settings"
                } else {
                    "Delete provider instance"
                };
                action(label, ButtonVariant::GhostMuted, false, move || {
                    let message = tr(
                        if default {
                            "Reset the {name} provider to its defaults?"
                        } else {
                            "Remove the {name} provider instance?"
                        },
                        &[("name", MsgValue::Text(name.clone()))],
                    );
                    let options = ConfirmDialogOptions {
                        variant: ConfirmDialogVariant::Destructive,
                        ..Default::default()
                    };
                    if let Some(confirmation) = request_confirm_dialog(message, Some(options)) {
                        let (page, env, id) = (page.clone(), env.clone(), id.clone());
                        page.tasks.spawn(async move {
                            if confirmation.await {
                                if let Err(error) = (page.services.command)(
                                    env,
                                    Command::Instance(ProviderInstanceMutation::Remove {
                                        instance_id: id,
                                    }),
                                )
                                .await
                                {
                                    show_error("Could not update provider instance", error);
                                } else {
                                    let _ = page.selected.try_set(None);
                                }
                            }
                        });
                    }
                })
            })
        })
        .flatten();
    let can_edit_name = can_manage
        && !config
            .managed_policy
            .as_ref()
            .and_then(|policy| policy.limits.as_ref())
            .and_then(|limits| limits.providers.as_ref())
            .and_then(|providers| providers.locked_instance_fields.as_ref())
            .and_then(|locked| locked.iter().find(|(driver, _)| driver.0 == row.driver))
            .is_some_and(|(_, paths)| paths.iter().any(|path| path == "displayName"));
    let name_row = base.map(|instance| {
        let (page, env, id) = (page.clone(), env.clone(), row.id.clone());
        let old = instance
            .display_name
            .as_ref()
            .and_then(Option::as_ref)
            .cloned()
            .unwrap_or_default();
        settings_row(
            SettingsRowProps::titled("provider-display-name", "Display name".into()).control(
                move || {
                    let name = signal(old.clone());
                    let (page, env, id, instance, old) = (
                        page.clone(),
                        env.clone(),
                        id.clone(),
                        instance.clone(),
                        old.clone(),
                    );
                    nsh_base::input()
                        .app_class(tw(
                            "min-w-0 flex-1 rounded-md border border-input px-2 text-sm",
                        ))
                        .label(t("Display name"))
                        .disabled(!can_edit_name)
                        .value(move || name.get())
                        .on_change(move |value: &str| name.set(value.to_owned()))
                        .on_submit(move |value: &str| {
                            let value = value.trim();
                            if value != old {
                                let mut instance = instance.clone();
                                instance.display_name =
                                    Some((!value.is_empty()).then(|| value.to_owned()));
                                page.request(
                                    env.clone(),
                                    Command::Instance(ProviderInstanceMutation::Upsert {
                                        instance_id: id.clone(),
                                        instance,
                                    }),
                                    "Could not update provider instance",
                                );
                            }
                        })
                },
            ),
        )
    });
    section = section.child(settings_section(
        "",
        Text::content(row.name.clone()),
        view()
            .child(settings_row(
                SettingsRowProps::titled("provider-status", "Status".into())
                    .first()
                    .status(move || Some(Text::content(headline.clone())))
                    .description(move || Text::content(detail.clone().unwrap_or_default())),
            ))
            .child(name_row)
            .child(remove),
    ));
    if let Some(provider) = &row.provider {
        section = section.child(setup_section(
            page.clone(),
            env,
            config,
            row,
            provider,
            can_manage,
        ));
        if row.driver == "acpRegistry"
            && provider
                .native_sessions
                .as_ref()
                .and_then(Option::as_ref)
                .is_some_and(|sessions| sessions.can_list)
        {
            section = section.child(acp_sessions(
                page.clone(),
                env.clone(),
                row.id.clone(),
                projects,
            ));
        }
    }
    section
        .child(models_section(page, context, env, config, row, can_manage))
        .into_element()
}

#[derive(Clone, PartialEq)]
struct SetupTerminal {
    key: ThreadKey,
    instance: ProviderInstanceId,
    driver: String,
    cwd: String,
    command: String,
}

fn setup_section(
    page: Rc<Page>,
    env: &EnvironmentId,
    config: &ServerConfig,
    row: &ProviderSettingsRow,
    provider: &ServerProvider,
    can_manage: bool,
) -> Option<AnyElement> {
    let setup = provider.setup.as_ref().and_then(Option::as_ref);
    let has_auth = can_manage
        && (setup.is_some_and(|setup| setup.can_authenticate)
            || (provider.driver.0 == "acpRegistry" && provider.installed));
    let has_install = can_manage && setup.is_some_and(|setup| setup.can_install);
    let os = config.environment.platform.os.clone();
    let key = ThreadKey {
        environment: env.clone(),
        thread: ThreadId("onboarding-agent-setup".into()),
    };
    let managed = row.driver == "codex"
        && config
            .settings
            .provider_instances
            .get(&row.id)
            .and_then(|instance| instance.config.as_ref())
            .and_then(|config| config.get("setupMode"))
            .and_then(Value::as_str)
            == Some("managed");
    let terminal = if can_manage && row.enabled && page.services.terminals.0.can_operate(&key) {
        provider_readiness::onboarding_install_command(&row.driver, os.clone())
            .and_then(|command| {
                if !provider.installed && !has_install {
                    Some(("Runtime", "Install", command.to_owned()))
                } else if provider.installed
                    && !has_auth
                    && provider.auth.status == ServerProviderAuthStatus::Unauthenticated
                {
                    Some((
                        "Account",
                        "Sign in",
                        provider_readiness::onboarding_login_command(
                            provider,
                            &config.settings,
                            os,
                        ),
                    ))
                } else {
                    None
                }
            })
            .map(|(title, label, command)| {
                (
                    title,
                    label,
                    SetupTerminal {
                        key,
                        instance: row.id.clone(),
                        driver: row.driver.clone(),
                        cwd: config.cwd.0.clone(),
                        command,
                    },
                )
            })
    } else {
        None
    };
    if !has_auth && !has_install && terminal.is_none() {
        return None;
    }
    let target = ProviderSetupInput {
        instance_id: row.id.clone(),
    };
    let mut content = view();
    if has_install {
        content = content.child(install_row(
            page.clone(),
            env.clone(),
            target.clone(),
            row.enabled,
        ));
    }
    if let Some((title, label, terminal)) = terminal {
        let active = page.terminal;
        content = content.child(settings_row(
            SettingsRowProps::titled("", title.into()).control(move || {
                let terminal = terminal.clone();
                action(label, ButtonVariant::Outline, false, move || {
                    active.set(Some(terminal.clone()))
                })
            }),
        ));
    }
    if has_auth {
        content = content.child(auth_row(
            page,
            env.clone(),
            target,
            provider.clone(),
            row.enabled,
            row.name.clone(),
            managed,
        ));
    }
    Some(settings_section("", "Setup".into(), content).into_element())
}

fn setup_terminal_dialog(page: Rc<Page>, terminal: SetupTerminal) -> AnyElement {
    use crate::ui::dialog::{MaxWidth, PopupOptions, dialog, dialog_popup, dialog_title};
    let close: Rc<dyn Fn()> = {
        let active = page.terminal;
        let env = terminal.key.environment.clone();
        let page = page.clone();
        Rc::new(move || {
            active.set(None);
            page.request(
                env.clone(),
                Command::RefreshAfterSetup,
                "Could not refresh provider status",
            );
        })
    };
    dialog(signal(true), |dialog_state| {
        let dismiss = close.clone();
        dialog_state.on_open_change(move |open| {
            if !open {
                dismiss();
            }
        });
        dialog_popup(
            dialog_state,
            PopupOptions {
                max_width: MaxWidth::TwoXl,
                ..Default::default()
            },
            move || {
                view()
                    .child(dialog_title(dialog_state, || "Setup terminal".into()))
                    .child(terminal_view(page.clone(), terminal.clone(), close.clone()))
            },
        )
    })
    .into_element()
}

fn terminal_view(page: Rc<Page>, terminal: SetupTerminal, close: Rc<dyn Fn()>) -> AnyElement {
    use SetupTerminalState::*;
    let phase = signal(Preparing);
    let (id, mut events) = match (page.services.setup_terminal)(&terminal) {
        Ok(result) => result,
        Err(error) => {
            return text(error.to_string())
                .app_class(tw("text-sm text-destructive"))
                .into_element();
        }
    };
    watch(async move {
        while let Some(state) = events.next().await {
            phase.set(state);
        }
    });
    let close_viewport = close.clone();
    let command = terminal.command.clone();
    view().app_class(tw("mt-4 overflow-hidden rounded-lg border border-border/70 bg-background text-foreground"))
        .child(view().app_class(tw("flex flex-row items-center justify-between border-b border-border/60 bg-background/60 px-3 py-1.5"))
            .child(text(move || SharedString::from(match phase.get() {
                Preparing => t("Preparing command..."),
                Ready => t("Review the command, then press Enter to run it."),
                OpenFailed => t("Could not open the setup terminal."),
                WriteFailed => tr("Run {command} in this terminal.", &[("command", MsgValue::Text(command.clone()))]),
            })).app_class(tw("text-2xs font-medium text-muted-foreground")))
            .child(action("Close", ButtonVariant::GhostMuted, false, move || close())))
        .child(region(move || phase.get(), move |phase| matches!(phase, Ready | WriteFailed).then(|| {
            view().h(px(240.0)).child(crate::screens::terminal::session::terminal_viewport(
                page.services.terminals.clone(), terminal.key.clone(), id.clone(), t("Setup terminal"),
                TerminalLaunch { cwd: terminal.cwd.clone(), worktree_path: None, env: Default::default() },
                computed(|| true), close_viewport.clone(),
            ))
        }))).into_element()
}

#[derive(Clone)]
struct Account {
    page: Rc<Page>,
    env: EnvironmentId,
    target: ProviderSetupInput,
    state: Signal<Option<ProviderAuthState>>,
    pending: Signal<bool>,
    error: Signal<Option<String>>,
    method: Signal<Option<t3_contracts::base_schemas::TrimmedNonEmptyString>>,
}

impl Account {
    fn run(&self, command: AuthCommand, confirmation: Option<crate::ui::confirm::Confirmation>) {
        if self.pending.get_untracked() {
            return;
        }
        self.pending.set(true);
        self.error.set(None);
        let account = self.clone();
        self.page.tasks.spawn(async move {
            if let Some(confirmation) = confirmation
                && !confirmation.await
            {
                let _ = account.pending.try_set(false);
                return;
            }
            match (account.page.services.auth)(account.env.clone(), command).await {
                Ok(value) => {
                    let _ = account.state.try_set(Some(value));
                }
                Err(error) => {
                    let _ = account.error.try_set(Some(error.to_string()));
                }
            }
            let _ = account.pending.try_set(false);
        });
    }

    fn respond(
        &self,
        flow: &t3_contracts::base_schemas::TrimmedNonEmptyString,
        interaction: &t3_contracts::base_schemas::TrimmedNonEmptyString,
        response: ProviderAuthResponse,
    ) {
        self.run(
            AuthCommand::Respond(ProviderAuthRespondInput {
                instance_id: self.target.instance_id.clone(),
                flow_id: flow.clone(),
                interaction_id: interaction.clone(),
                response,
            }),
            None,
        );
    }
}

fn auth_row(
    page: Rc<Page>,
    env: EnvironmentId,
    target: ProviderSetupInput,
    provider: ServerProvider,
    enabled: bool,
    name: String,
    managed: bool,
) -> AnyElement {
    let local_sign_in = (page.services.local_sign_in)(&env);
    let account = Rc::new(Account {
        state: signal(None),
        pending: signal(false),
        error: signal(None),
        method: signal(None),
        page,
        env,
        target,
    });
    let mut events = (account.page.services.auth_state)(&account.env, account.target.clone());
    let state = account.state;
    let error = account.error;
    watch(async move {
        while let Some(event) = events.next().await {
            match event {
                Ok(value) => state.set(Some(value)),
                Err(reason) => error.set(Some(reason.to_string())),
            }
        }
    });
    let pending = account.pending;
    region(
        move || (state.get(), pending.get(), error.get()),
        move |(state, pending, error)| {
            let signed_in = state.as_ref().map_or_else(
                || provider.auth.status == ServerProviderAuthStatus::Authenticated,
                |value| match value.phase {
                    ProviderAuthPhase::Succeeded => {
                        value.credential_owner.is_some()
                            || provider.auth.status == ServerProviderAuthStatus::Authenticated
                    }
                    _ => provider.auth.status == ServerProviderAuthStatus::Authenticated,
                },
            );
            let active = state.as_ref().is_some_and(|s| {
                matches!(
                    s.phase,
                    ProviderAuthPhase::Starting
                        | ProviderAuthPhase::Waiting
                        | ProviderAuthPhase::Verifying
                )
            });
            let interaction = state
                .as_ref()
                .and_then(|s| s.interaction.as_ref())
                .and_then(Option::as_ref);
            let desc = if active {
                match interaction {
                    Some(ProviderAuthInteraction::Terminal { .. }) => {
                        "Complete sign-in in the terminal below."
                    }
                    Some(ProviderAuthInteraction::Credentials { .. }) => {
                        "Enter your credentials below."
                    }
                    _ => "Finish signing in in your browser.",
                }
            } else if signed_in {
                "Signed in."
            } else {
                "Sign in"
            };
            let can_sign_out = provider.auth.can_logout.flatten().unwrap_or_else(|| {
                provider
                    .setup
                    .as_ref()
                    .and_then(Option::as_ref)
                    .is_some_and(|s| s.can_authenticate)
            });
            let flow = state.as_ref().and_then(|s| s.flow_id.clone());
            let message = error.or_else(|| {
                state
                    .as_ref()
                    .filter(|s| s.phase == ProviderAuthPhase::Failed)
                    .and_then(|s| s.message.clone())
            });
            let row = SettingsRowProps::titled("provider-account", "Account".into()).description(move || desc.into())
            .status(move || message.clone().map(Text::content))
            .control({
                let (account, flow, name, state) = (account.clone(), flow.clone(), name.clone(), state.clone());
                move || {
                    let mut buttons = view().app_class(tw("flex min-w-0 flex-row flex-wrap items-center gap-1.5"));
                    if active {
                        if let Some(flow) = &flow {
                            let (account, flow) = (account.clone(), flow.clone());
                            buttons = buttons.child(action("Cancel", ButtonVariant::Ghost, pending, move || {
                                account.run(AuthCommand::Cancel(ProviderAuthCancelInput {
                                    instance_id: account.target.instance_id.clone(), flow_id: flow.clone(),
                                }), None);
                            }));
                        }
                        return buttons;
                    }
                    if let Some(methods) = state.as_ref().and_then(|s| s.methods.as_ref()).filter(|methods| methods.len() > 1) {
                        let method = account.method;
                        let mut props = SelectProps::new("Sign-in method");
                        let mut items = vec![SelectItem::new("", t("Provider default"))];
                        items.extend(methods.iter().map(|method| SelectItem::new(method.id.0.clone(), method.name.0.clone())));
                        props.items = items.into();
                        props.value = Prop::bind(move || SelectValue::Controlled(Some(method.get().map_or_else(String::new, |id| id.0).into())));
                        props.disabled = pending.into();
                        props.size = SelectSize::Sm;
                        buttons = buttons.child(select(props, move |value| method.set(value.filter(|v| !v.is_empty()).map(|v| t3_contracts::base_schemas::TrimmedNonEmptyString(v.to_string())))));
                    }
                    let start = account.clone();
                    if managed {
                        let opener = link_opener_here();
                        buttons = buttons.child(action(if signed_in { "Change account" } else { "Continue with ChatGPT" },
                            ButtonVariant::Outline, pending || !enabled || !local_sign_in, move || {
                                if start.pending.get_untracked() { return; }
                                start.pending.set(true);
                                start.error.set(None);
                                let future = (start.page.services.managed_sign_in)(start.env.clone(), start.target.instance_id.clone());
                                let (start, opener) = (start.clone(), opener.clone());
                                start.page.tasks.spawn(async move {
                                    match future.await {
                                        Ok(url) if start.pending.try_set(false) => opener.open(url),
                                        Ok(_) => {}
                                        Err(error) => { let _ = start.error.try_set(Some(error.to_string())); }
                                    }
                                    let _ = start.pending.try_set(false);
                                });
                            }));
                    } else {
                        buttons = buttons.child(action(if signed_in { "Change account" } else { "Sign in" },
                            ButtonVariant::Outline, pending || !enabled || !provider.installed || state.is_none(), move || {
                                start.run(AuthCommand::Start(ProviderAuthStartInput {
                                    instance_id: start.target.instance_id.clone(), method_id: start.method.get_untracked(),
                                    return_url: None, callback_mode: None,
                                }), None);
                            }));
                    }
                    if signed_in && can_sign_out {
                        let (account, name) = (account.clone(), name.clone());
                        buttons = buttons.child(action("Sign out", ButtonVariant::Ghost, pending || state.is_none(), move || {
                            let message = tr(
                                "Sign out of {name}? This stops running threads that share this sign-in. Thread history is kept.",
                                &[("name", MsgValue::Text(name.clone()))],
                            );
                            if let Some(confirmation) = request_confirm_dialog(message, Some(ConfirmDialogOptions { variant: ConfirmDialogVariant::Destructive, ..Default::default() })) {
                                account.run(AuthCommand::Logout(account.target.clone()), Some(confirmation));
                            }
                        }));
                    }
                    buttons
                }
            });
            view()
                .child(settings_row(row))
                .child(flow.as_ref().zip(interaction).map(|(flow, interaction)| {
                    auth_interaction(account.clone(), flow.clone(), interaction.clone(), pending)
                }))
        },
    )
}

fn auth_interaction(
    account: Rc<Account>,
    flow: t3_contracts::base_schemas::TrimmedNonEmptyString,
    interaction: ProviderAuthInteraction,
    pending: bool,
) -> AnyElement {
    use crate::ui::input::{InputSize, masked_text_field, text_field};
    let panel = view().app_class(tw("flex flex-col gap-2 px-3 py-2 sm:px-4"));
    match interaction {
        ProviderAuthInteraction::Browser {
            id,
            url,
            requires_consent,
            ..
        } => {
            let opener = link_opener_here();
            panel
                .child(action(
                    "Open sign-in page",
                    ButtonVariant::Outline,
                    pending,
                    move || {
                        if requires_consent {
                            account.respond(
                                &flow,
                                &id,
                                ProviderAuthResponse::Browser {
                                    action: ProviderBrowserAuthAction::Accept,
                                },
                            );
                        }
                        opener.open(url.0.clone());
                    },
                ))
                .into_element()
        }
        ProviderAuthInteraction::DeviceCode { url, user_code, .. } => {
            let opener = link_opener_here();
            panel
                .child(text(tr(
                    "Enter code {code} in your browser.",
                    &[("code", MsgValue::Text(user_code.0))],
                )))
                .child(action(
                    "Open sign-in page",
                    ButtonVariant::Outline,
                    pending,
                    move || opener.open(url.0.clone()),
                ))
                .into_element()
        }
        ProviderAuthInteraction::Terminal { id, output, .. } => {
            let input = signal(String::new());
            let submit = move || {
                let line = input.get_untracked();
                input.set(String::new());
                account.respond(
                    &flow,
                    &id,
                    ProviderAuthResponse::Terminal {
                        data: format!("{line}\r"),
                        size: None,
                    },
                );
            };
            panel
                .child(text(output).app_class(tw("font-mono text-xs")))
                .child(
                    nsh_base::input()
                        .label(t("Terminal input"))
                        .disabled(pending)
                        .value(move || input.get())
                        .on_change(move |value: &str| input.set(value.to_owned()))
                        .on_submit(move |_: &str| submit()),
                )
                .into_element()
        }
        ProviderAuthInteraction::Credentials { id, fields } => {
            let mut controls = panel;
            let mut values = Vec::new();
            for field in fields {
                let value = signal(String::new());
                let name = field.name.clone();
                let label = field.label.0;
                values.push((name, value));
                controls = controls
                    .child(text(label.clone()).app_class(tw("text-xs text-muted-foreground")));
                controls = if field.secret {
                    controls.child(masked_text_field(
                        value,
                        String::new(),
                        label,
                        move || pending,
                        || {},
                    ))
                } else {
                    controls.child(text_field(
                        value,
                        String::new(),
                        label,
                        InputSize::Default,
                        || {},
                    ))
                };
            }
            controls
                .child(action(
                    "Connect",
                    ButtonVariant::Outline,
                    pending,
                    move || {
                        account.respond(
                            &flow,
                            &id,
                            ProviderAuthResponse::Credentials {
                                values: values
                                    .iter()
                                    .map(|(name, value)| (name.clone(), value.get_untracked()))
                                    .collect(),
                            },
                        );
                    },
                ))
                .into_element()
        }
    }
}

fn install_row(
    page: Rc<Page>,
    env: EnvironmentId,
    target: ProviderSetupInput,
    enabled: bool,
) -> AnyElement {
    let state = signal(None::<ProviderInstallState>);
    let pending = signal(false);
    let error = signal(None::<String>);
    let mut events = (page.services.install_state)(&env, target.clone());
    watch(async move {
        while let Some(event) = events.next().await {
            match event {
                Ok(next) => state.set(Some(next)),
                Err(reason) => error.set(Some(reason.to_string())),
            }
        }
    });
    region(
        move || (state.get(), pending.get(), error.get()),
        move |(state_value, busy, message)| {
            let command = match state_value.as_ref() {
                Some(current)
                    if matches!(
                        current.phase,
                        ProviderInstallPhase::Downloading
                            | ProviderInstallPhase::Extracting
                            | ProviderInstallPhase::Verifying
                    ) =>
                {
                    current.operation_id.clone().map(|operation_id| {
                        (
                            "Cancel",
                            InstallCommand::Cancel(ProviderInstallCancelInput {
                                instance_id: target.instance_id.clone(),
                                operation_id,
                            }),
                        )
                    })
                }
                Some(current) if current.can_remove => {
                    Some(("Remove runtime", InstallCommand::Remove(target.clone())))
                }
                Some(_) => Some((
                    "Install managed runtime",
                    InstallCommand::Start(target.clone()),
                )),
                None => None,
            };
            let message = message.or_else(|| state_value.and_then(|state| state.message));
            settings_row(
                SettingsRowProps::titled("provider-runtime", "Runtime".into())
                    .status(move || message.clone().map(Text::content))
                    .control({
                        let (page, env) = (page.clone(), env.clone());
                        let command = Rc::new(command);
                        move || {
                            command
                                .as_ref()
                                .as_ref()
                                .map(|(label, op)| {
                                    let (page, env, op) = (page.clone(), env.clone(), op.clone());
                                    action(
                                        label,
                                        ButtonVariant::Outline,
                                        busy || !enabled,
                                        move || {
                                            if pending.get_untracked() {
                                                return;
                                            }
                                            pending.set(true);
                                            error.set(None);
                                            let future =
                                                (page.services.install)(env.clone(), op.clone());
                                            page.tasks.spawn(async move {
                                                match future.await {
                                                    Ok(result) => {
                                                        let _ = state.try_set(Some(result));
                                                    }
                                                    Err(reason) => {
                                                        let _ =
                                                            error.try_set(Some(reason.to_string()));
                                                    }
                                                }
                                                let _ = pending.try_set(false);
                                            });
                                        },
                                    )
                                })
                                .unwrap_or_else(|| view().into_element())
                        }
                    }),
            )
        },
    )
}

fn icon_action(icon: Icon, label: String, disabled: bool, run: impl Fn() + 'static) -> AnyElement {
    button(
        ButtonContent::Icon { icon, label },
        ButtonVariant::GhostMuted,
        ButtonSize::IconXs,
        move || disabled,
        run,
    )
    .into_element()
}

fn models_section(
    page: Rc<Page>,
    context: SettingsContext,
    env: &EnvironmentId,
    config: &ServerConfig,
    row: &ProviderSettingsRow,
    can_manage: bool,
) -> AnyElement {
    let can_write = (page.services.grants)(env, Scope::SettingsWrite);
    let preferences = config
        .settings
        .provider_model_preferences
        .as_ref()
        .and_then(|values| values.get(&row.id));
    let favorites = config
        .settings
        .favorites
        .as_ref()
        .unwrap_or(&context.client_settings().favorites)
        .iter()
        .filter(|favorite| favorite.provider == row.id)
        .map(|favorite| favorite.model.0.clone())
        .collect::<BTreeSet<_>>();
    let hidden = preferences.map_or_else(Vec::new, |p| p.hidden_models.clone());
    let mut reported = row
        .provider
        .iter()
        .flat_map(|provider| &provider.models)
        .filter(|model| !model.is_custom)
        .map(|model| (model.slug.0.clone(), model.name.0.clone(), false))
        .collect::<Vec<_>>();
    for model in models::read_custom_models(&config.settings, &row.id, &row.driver) {
        if !reported.iter().any(|(slug, _, _)| *slug == model.slug) {
            reported.push((model.slug, model.name, true));
        }
    }
    for favorite in &favorites {
        if !reported.iter().any(|(slug, _, _)| slug == favorite) {
            reported.push((favorite.clone(), favorite.clone(), false));
        }
    }
    models::sort_models(
        &mut reported,
        |(slug, _, _)| slug,
        preferences.map_or(&[], |p| p.model_order.as_slice()),
        &favorites,
        true,
    );
    let group = |slug: &str, custom: bool| {
        if favorites.contains(slug) {
            0
        } else if !custom && hidden.iter().any(|candidate| candidate == slug) {
            2
        } else {
            1
        }
    };
    reported.sort_by_key(|(slug, _, custom)| group(slug, *custom));
    let write = {
        let (page, env) = (page.clone(), env.clone());
        move |patch: Value| {
            if let Value::Object(patch) = patch {
                page.request(
                    env.clone(),
                    Command::Update(patch),
                    "Could not update model preferences",
                );
            }
        }
    };
    let filter = signal(String::new());
    let mut content = view();
    if reported.len() > 8 {
        use crate::ui::input::{InputSize, text_field};
        content = content.child(view().app_class(tw("px-3 py-2 sm:px-4")).child(text_field(
            filter,
            t("Filter models"),
            t("Filter models"),
            InputSize::Default,
            || {},
        )));
    }
    let instance = row.id.clone();
    let custom_rows = can_manage && row.driver != "antigravity";
    let configured = config
        .settings
        .provider_instances
        .get(&row.id)
        .cloned()
        .or_else(|| {
            row.provider
                .as_ref()
                .map(|provider| ProviderInstanceConfig {
                    driver: provider.driver.clone(),
                    enabled: None,
                    display_name: None,
                    accent_color: None,
                    config: None,
                    environment: None,
                })
        });
    let custom = signal(false);
    if custom_rows && let Some(configured) = configured {
        let (page, env, id) = (page.clone(), env.clone(), instance.clone());
        let names = reported
            .iter()
            .map(|(slug, _, _)| slug.clone())
            .collect::<BTreeSet<_>>();
        content = content
            .child(view().app_class(tw("px-3 py-2 sm:px-4")).child(action(
                "Add custom model",
                ButtonVariant::Outline,
                false,
                move || custom.set(true),
            )))
            .child(region(
                move || custom.get(),
                move |open| {
                    open.then(|| {
                        let draft = signal(String::new());
                        let (page, env, id, configured, names) = (
                            page.clone(),
                            env.clone(),
                            id.clone(),
                            configured.clone(),
                            names.clone(),
                        );
                        let save = move || {
                            let draft = draft.get_untracked();
                            let slug = draft.trim();
                            if slug.is_empty()
                                || slug.encode_utf16().count() > models::MAX_CUSTOM_MODEL_LENGTH
                                || names.contains(slug)
                            {
                                return;
                            }
                            let mut instance = configured.clone();
                            let mut config = instance
                                .config
                                .take()
                                .and_then(|value| value.as_object().cloned())
                                .unwrap_or_default();
                            let mut models = config
                                .remove("customModels")
                                .and_then(|value| value.as_array().cloned())
                                .unwrap_or_default();
                            models.push(json!(slug));
                            config.insert("customModels".into(), Value::Array(models));
                            instance.config = Some(Value::Object(config));
                            custom.set(false);
                            page.request(
                                env.clone(),
                                Command::Instance(ProviderInstanceMutation::Upsert {
                                    instance_id: id.clone(),
                                    instance,
                                }),
                                "Could not update provider instance",
                            );
                        };
                        view()
                            .app_class(tw("flex flex-col gap-2 px-3 py-2 sm:px-4"))
                            .child(crate::ui::input::text_field(
                                draft,
                                String::new(),
                                t("Model ID"),
                                crate::ui::input::InputSize::Default,
                                save.clone(),
                            ))
                            .child(action("Add model", ButtonVariant::Outline, false, save))
                            .child(action(
                                "Cancel",
                                ButtonVariant::GhostMuted,
                                false,
                                move || custom.set(false),
                            ))
                    })
                },
            ));
    }
    let rows = region(
        move || filter.get(),
        move |filter| {
            let query = filter.trim().to_lowercase();
            let mut body = view();
            for (index, (slug, name, custom)) in reported.iter().enumerate() {
                if !query.is_empty()
                    && !(slug.to_lowercase().contains(&query)
                        || name.to_lowercase().contains(&query))
                {
                    continue;
                }
                let favorite = favorites.contains(slug);
                let is_hidden = !custom && hidden.iter().any(|candidate| candidate == slug);
                let favorite_label = tr(
                    if favorite {
                        "Remove {name} from favorites"
                    } else {
                        "Add {name} to favorites"
                    },
                    &[("name", MsgValue::Text(name.clone()))],
                );
                let star = {
                    let (write, instance, slug) = (write.clone(), instance.clone(), slug.clone());
                    icon_action(Icon::Star, favorite_label, !can_write, move || {
                        write(
                            json!({"setModelFavorites":[{"provider":instance,"model":slug,"favorite":!favorite}]}),
                        )
                    })
                };
                let toggle = if *custom {
                    None
                } else {
                    let (write, instance, slug) = (write.clone(), instance.clone(), slug.clone());
                    Some(switch(
                        tr("Enable {name}", &[("name", MsgValue::Text(name.clone()))]),
                        ControlState {
                            checked: (!is_hidden).into(),
                            disabled: (!can_write).into(),
                            ..Default::default()
                        },
                        SwitchSize::Sm,
                        move |shown| {
                            write(
                                json!({"setModelsHidden":[{"provider":instance,"model":slug,"hidden":!shown}]}),
                            )
                        },
                    ))
                };
                let mut order = view().app_class(tw("flex flex-row items-center gap-0.5"));
                if !is_hidden && query.is_empty() {
                    for (icon, label, other) in [
                        (Icon::ArrowUp, "Move {name} up", index.checked_sub(1)),
                        (Icon::ArrowDown, "Move {name} down", Some(index + 1)),
                    ] {
                        let other = other.filter(|other| {
                            reported.get(*other).is_some_and(|(id, _, custom_row)| {
                                let other_hidden =
                                    !custom_row && hidden.iter().any(|candidate| candidate == id);
                                let this_group = usize::from(!favorite);
                                let other_group = usize::from(!favorites.contains(id));
                                !other_hidden && this_group == other_group
                            })
                        });
                        let order_patch = other.map(|other| {
                        let mut model_order = reported.iter().map(|(slug, _, _)| slug.clone()).collect::<Vec<_>>();
                        model_order.swap(index, other);
                        json!({"setProviderModelOrder":{"provider":instance,"modelOrder":model_order}})
                    });
                        let write = write.clone();
                        order = order.child(icon_action(
                            icon,
                            tr(label, &[("name", MsgValue::Text(name.clone()))]),
                            !can_write || order_patch.is_none(),
                            move || {
                                if let Some(patch) = &order_patch {
                                    write(patch.clone());
                                }
                            },
                        ));
                    }
                }
                body = body.child(view().app_class(tw("flex min-w-0 flex-row flex-wrap items-center gap-1.5 border-t border-border/60 px-3 py-2 sm:px-4"))
                .child(star)
                .child(view().app_class(tw("flex min-w-0 flex-1 flex-row flex-wrap items-baseline gap-2"))
                    .child(text(name.clone()).app_class(tw("truncate text-sm")))
                    .child((name != slug).then(|| text(slug.clone()).app_class(tw("truncate font-mono text-xs text-muted-foreground"))))
                    .child(custom.then(|| text(t("custom")).app_class(tw("text-xs text-muted-foreground")))))
                .child(order)
                .child(toggle));
            }
            body
        },
    );
    settings_section("provider-models", "Models".into(), content.child(rows)).into_element()
}

fn usage_sources(
    page: Rc<Page>,
    env: &EnvironmentId,
    config: &ServerConfig,
    can_manage: bool,
) -> AnyElement {
    let sources = Rc::new(config.settings.usage_limit_sources.clone());
    let mut list = view();
    for (id, source) in sources.iter() {
        let label = source
            .label
            .as_ref()
            .and_then(Option::as_ref)
            .map(|label| label.0.trim())
            .filter(|name| !name.is_empty())
            .unwrap_or(&source.url.0)
            .to_owned();
        let mut meta = t(if source.enabled {
            "CLI Proxy"
        } else {
            "CLI Proxy · Disabled"
        });
        if label != source.url.0 {
            meta = format!("{meta} · {}", source.url.0);
        }
        let (remove_page, remove_env, id) = (page.clone(), env.clone(), id.clone());
        let title = label.clone();
        let mut row = SettingsRowProps::titled("", Text::content(label))
            .description(move || Text::content(meta.clone()));
        if can_manage {
            row = row.control(move || {
                let (page, env, id, title) = (
                    remove_page.clone(),
                    remove_env.clone(),
                    id.clone(),
                    title.clone(),
                );
                action("Remove", ButtonVariant::Ghost, false, move || {
                    let message = tr(
                        "Remove {label}?",
                        &[("label", MsgValue::Text(title.clone()))],
                    );
                    if let Some(confirmation) = request_confirm_dialog(
                        message,
                        Some(ConfirmDialogOptions {
                            variant: ConfirmDialogVariant::Destructive,
                            ..Default::default()
                        }),
                    ) {
                        let (page, env, id) = (page.clone(), env.clone(), id.clone());
                        page.tasks.spawn(async move {
                            if confirmation.await {
                                let patch = SettingsPatch::from_iter([(
                                    "usageLimitSources".into(),
                                    json!({id.0: null}),
                                )]);
                                if let Err(err) =
                                    (page.services.command)(env, Command::Update(patch)).await
                                {
                                    show_error("Could not update usage providers", err);
                                }
                            }
                        });
                    }
                })
            });
        }
        list = list.child(settings_row(row));
    }
    if sources.is_empty() {
        list = list.child(settings_row(SettingsRowProps::titled(
            "",
            "No hubs configured.".into(),
        )));
    }
    if can_manage {
        use crate::ui::dialog::{MaxWidth, PopupOptions, dialog, dialog_popup, dialog_title};
        let env = env.clone();
        list = list.child(dialog(signal(false), move |dialog_state| {
            view().child(view().app_class(tw("px-3 py-2 sm:px-4"))
                .child(action("Add hub", ButtonVariant::Outline, false, move || dialog_state.show())))
                .child(dialog_popup(dialog_state, PopupOptions { max_width: MaxWidth::Md, ..Default::default() }, move || {
                use crate::ui::input::{InputSize, masked_text_field, text_field};
                let url = signal(String::new());
                let key = signal(String::new());
                let label = signal(String::new());
                let (page, env, sources) = (page.clone(), env.clone(), sources.clone());
                let save = move || {
                    let (raw_url, raw_key, raw_label) = (url.get_untracked(), key.get_untracked(), label.get_untracked());
                    let (url, key, label) = (raw_url.trim(), raw_key.trim(), raw_label.trim());
                    if url.is_empty() || key.is_empty() { return; }
                    let id = t3_client::services::providers::usage_source_id(url, &sources);
                    let mut config = json!({"kind":"cliproxy","url":url,"managementKey":key,"enabled":true});
                    if !label.is_empty() {
                        config["label"] = json!(label);
                    }
                    dialog_state.close();
                    page.request(env.clone(), Command::Update(SettingsPatch::from_iter([
                        ("usageLimitSources".into(), json!({id.0:config})),
                    ])), "Could not update usage providers");
                };
                view().app_class(tw("flex flex-col gap-2 px-3 py-3 sm:px-4"))
                    .child(dialog_title(dialog_state, || "Add a CLIProxyAPI hub".into()))
                    .child(text_field(url, "https://hub.example.ts.net:8318".into(), t("Hub URL"), InputSize::Default, save.clone()))
                    .child(masked_text_field(key, String::new(), t("Management key"), || false, save.clone()))
                    .child(text_field(label, t("Defaults to the hub's host name"), t("Label (optional)"), InputSize::Default, save.clone()))
                    .child(view().app_class(tw("flex flex-row gap-2"))
                        .child(action("Cancel", ButtonVariant::Outline, false, move || dialog_state.close()))
                        .child(button(ButtonContent::Label(t("Add hub")), ButtonVariant::Default, ButtonSize::Xs, move || url.get().trim().is_empty() || key.get().trim().is_empty(), save)))
            }))
        }));
    }
    settings_section("usage-providers", "Usage providers".into(), list).into_element()
}

fn load_balancing(
    page: Rc<Page>,
    context: SettingsContext,
    scope: &SettingsScopeView,
    env: &EnvironmentId,
) -> Option<AnyElement> {
    if scope.all_environments.len() < 2 {
        return None;
    }
    let enabled = context.client_settings().load_balancing_enabled;
    let enabled_write = {
        let (page, env) = (page.clone(), env.clone());
        move |enabled| {
            page.request(
                env.clone(),
                Command::UpdateDevice(SettingsPatch::from_iter([(
                    "loadBalancingEnabled".into(),
                    json!(enabled),
                )])),
                "Could not update load balancing",
            );
        }
    };
    let mut entries = view()
        .child(settings_row(SettingsRowProps::titled("", "Automatically balance load".into())
            .description(|| "New threads in shared projects start on the machine with the most free CPU and memory, weighted by each machine's preference.".into())
            .control(move || switch("Automatically balance load", ControlState { checked: enabled.into(), ..Default::default() }, SwitchSize::Default, enabled_write.clone()))));
    for other in &scope.all_environments {
        let (page, env, id) = (page.clone(), env.clone(), other.environment_id.0.clone());
        let name = other.label.clone();
        entries = entries.child(settings_row(
            SettingsRowProps::titled("", Text::content(name.clone())).control(move || {
                let (page, env, id) = (page.clone(), env.clone(), id.clone());
                let weight = context
                    .client_settings()
                    .load_balancing_weights
                    .get(&t3_contracts::base_schemas::TrimmedNonEmptyString(
                        id.clone(),
                    ))
                    .copied();
                let mut props = SelectProps::new(Text::content(tr(
                    "{label} load preference",
                    &[("label", MsgValue::Text(name.clone()))],
                )));
                props.items = [
                    ("100", "Prefer"),
                    ("50", "Normal"),
                    ("25", "Less often"),
                    ("0", "Manual only"),
                ]
                .into_iter()
                .map(|(value, label)| SelectItem::new(value, t(label)))
                .collect::<Vec<_>>()
                .into();
                props.value = SelectValue::Controlled(Some(
                    t3_client::services::providers::load_preference(weight).into(),
                ))
                .into();
                props.disabled = (!enabled).into();
                props.size = SelectSize::Xs;
                select(props, move |value| {
                    let Some(weight) = value.and_then(|value| value.parse::<i64>().ok()) else {
                        return;
                    };
                    page.request(
                        env.clone(),
                        Command::SetLoadWeight(
                            t3_contracts::base_schemas::TrimmedNonEmptyString(id.clone()),
                            weight,
                        ),
                        "Could not update load balancing",
                    );
                })
            }),
        ));
    }
    Some(settings_section("load-balancing", "Load balancing".into(), entries).into_element())
}

#[derive(Clone)]
struct NativeSessions {
    page: Rc<Page>,
    env: EnvironmentId,
    instance: ProviderInstanceId,
    project: Signal<Option<ProjectId>>,
    result: Signal<Option<AcpRegistryListSessionsResult>>,
    pending: Signal<bool>,
}

impl NativeSessions {
    fn fetch(&self, more: bool) {
        let Some(project_id) = self.project.get_untracked() else {
            return;
        };
        if self.pending.get_untracked() {
            return;
        }
        let cursor = more
            .then(|| {
                self.result.with_untracked(|value| {
                    value.as_ref().and_then(|value| value.next_cursor.clone())
                })
            })
            .flatten();
        let future = (self.page.services.list_sessions)(
            self.env.clone(),
            AcpRegistryListSessionsInput {
                instance_id: self.instance.clone(),
                project_id,
                cursor,
            },
        );
        self.pending.set(true);
        let this = self.clone();
        self.page.tasks.spawn(async move {
            match future.await {
                Ok(mut next) => {
                    if more && let Some(Some(previous)) = this.result.try_get() {
                        let mut previous = previous.sessions;
                        next.sessions.retain(|row| {
                            !previous
                                .iter()
                                .any(|known| known.session_id == row.session_id)
                        });
                        previous.append(&mut next.sessions);
                        next.sessions = previous;
                    }
                    let _ = this.result.try_set(Some(next));
                }
                Err(error) => show_error("Could not list ACP sessions", error),
            }
            let _ = this.pending.try_set(false);
        });
    }
}

fn acp_sessions(
    page: Rc<Page>,
    env: EnvironmentId,
    instance: ProviderInstanceId,
    projects: &[(ProjectId, String)],
) -> AnyElement {
    if projects.is_empty() {
        return settings_row(
            SettingsRowProps::titled("provider-native-sessions", "Native sessions".into())
                .description(|| "Add a project before importing sessions.".into()),
        )
        .into_element();
    }
    let can_read = (page.services.grants)(&env, Scope::OrchestrationRead);
    let can_operate = (page.services.grants)(&env, Scope::OrchestrationOperate);
    let subject = Rc::new(NativeSessions {
        page,
        env,
        instance,
        project: signal(projects.first().map(|(id, _)| id.clone())),
        result: signal(None),
        pending: signal(false),
    });
    let items = projects
        .iter()
        .map(|(id, name)| SelectItem::new(id.0.clone(), name.clone()))
        .collect::<Vec<_>>();
    let body = {
        let subject = subject.clone();
        let (result, pending) = (subject.result, subject.pending);
        region(
            move || (result.get(), pending.get()),
            move |(result, pending)| {
                let Some(result) = result else {
                    return view();
                };
                let mut rows = view().app_class(tw("flex flex-col px-3 sm:px-4"));
                let can_import = result.can_load || result.can_resume;
                for session in &result.sessions {
                    let imported = session.imported_thread_id.is_some()
                        && session.native_history_pending != Some(true);
                    let import = {
                        let (subject, session) = (subject.clone(), session.clone());
                        action(
                            if imported { "Imported" } else { "Import" },
                            ButtonVariant::GhostMuted,
                            pending || imported || !can_import || !can_operate,
                            move || {
                                let Some(project_id) = subject.project.get_untracked() else {
                                    return;
                                };
                                let s = subject.clone();
                                if s.pending.get_untracked() {
                                    return;
                                }
                                s.pending.set(true);
                                let id = session.session_id.clone();
                                let future = (s.page.services.import_session)(
                                    s.env.clone(),
                                    AcpRegistryImportSessionInput {
                                        instance_id: s.instance.clone(),
                                        project_id,
                                        session_id: id.clone(),
                                        title: session.title.clone().map(|value| {
                                            Some(t3_contracts::base_schemas::TrimmedNonEmptyString(
                                                value,
                                            ))
                                        }),
                                        updated_at: session.updated_at.clone().map(|value| {
                                            Some(t3_contracts::base_schemas::TrimmedNonEmptyString(
                                                value,
                                            ))
                                        }),
                                    },
                                );
                                subject.page.tasks.spawn(async move {
                                    match future.await {
                                        Ok(thread) => {
                                            if let Some(Some(mut list)) = s.result.try_get()
                                                && let Some(row) = list
                                                    .sessions
                                                    .iter_mut()
                                                    .find(|row| row.session_id == id)
                                            {
                                                row.imported_thread_id = Some(thread);
                                                row.native_history_pending = Some(false);
                                                let _ = s.result.try_set(Some(list));
                                            }
                                        }
                                        Err(error) => {
                                            show_error("Could not import ACP session", error)
                                        }
                                    }
                                    let _ = s.pending.try_set(false);
                                });
                            },
                        )
                    };
                    let delete = result.can_delete.then(|| {
                        let (subject, session) = (subject.clone(), session.clone());
                        action(
                            "Delete",
                            ButtonVariant::GhostMuted,
                            pending || session.imported_thread_id.is_some() || !can_operate,
                            move || {
                                let Some(project_id) = subject.project.get_untracked() else {
                                    return;
                                };
                                let confirmation = request_confirm_dialog(
                                    t("Delete this native session?"),
                                    Some(ConfirmDialogOptions {
                                        variant: ConfirmDialogVariant::Destructive,
                                        ..Default::default()
                                    }),
                                );
                                let Some(confirmation) = confirmation else {
                                    return;
                                };
                                if subject.pending.get_untracked() {
                                    return;
                                }
                                subject.pending.set(true);
                                let (s, session_id) = (subject.clone(), session.session_id.clone());
                                subject.page.tasks.spawn(async move {
                                    if confirmation.await {
                                        let command =
                                            Command::DeleteSession(AcpRegistryDeleteSessionInput {
                                                instance_id: s.instance.clone(),
                                                project_id,
                                                session_id: session_id.clone(),
                                            });
                                        match (s.page.services.command)(s.env.clone(), command)
                                            .await
                                        {
                                            Ok(()) => {
                                                if let Some(Some(mut list)) = s.result.try_get() {
                                                    list.sessions
                                                        .retain(|row| row.session_id != session_id);
                                                    let _ = s.result.try_set(Some(list));
                                                }
                                            }
                                            Err(error) => {
                                                show_error("Could not delete ACP session", error)
                                            }
                                        };
                                    }
                                    let _ = s.pending.try_set(false);
                                });
                            },
                        )
                    });
                    rows = rows.child(view().app_class(tw("flex flex-row flex-wrap items-center gap-2 border-t border-border/60 py-2"))
                    .child(view().app_class(tw("min-w-0 flex-1"))
                        .child(text(session.title.clone().unwrap_or_else(|| session.session_id.0.clone())).app_class(tw("truncate text-xs")))
                        .child(text(session.session_id.0.clone()).app_class(tw("truncate font-mono text-xs text-muted-foreground"))))
                    .child(import)
                    .child(delete));
                }
                if result.next_cursor.is_some() {
                    let subject = subject.clone();
                    rows = rows.child(action(
                        "Load more",
                        ButtonVariant::Outline,
                        pending || !can_read,
                        move || subject.fetch(true),
                    ));
                }
                rows
            },
        )
    };
    let row = settings_row(
        SettingsRowProps::titled("provider-native-sessions", "Native sessions".into()).control(
            move || {
                let mut props = SelectProps::new("Project for ACP sessions");
                props.items = items.clone().into();
                let project = subject.project;
                let result = subject.result;
                let pending = subject.pending;
                props.value = Prop::bind(move || {
                    SelectValue::Controlled(project.get().map(|value| value.0.into()))
                });
                props.size = SelectSize::Sm;
                props.disabled = Prop::bind(move || pending.get() || !can_read);
                let subject = subject.clone();
                view()
                    .app_class(tw("flex flex-row flex-wrap items-center gap-2"))
                    .child(select(props, move |next| {
                        if let Some(next) = next {
                            project.set(Some(ProjectId(next.to_string())));
                            result.set(None);
                        }
                    }))
                    .child(button(
                        ButtonContent::Label(t("List sessions")),
                        ButtonVariant::Outline,
                        ButtonSize::Xs,
                        move || !can_read || pending.get(),
                        move || subject.fetch(false),
                    ))
            },
        ),
    );
    view().child(row).child(body).into_element()
}

fn add_provider(page: Rc<Page>, env: EnvironmentId, config: &ServerConfig) -> Option<AnyElement> {
    use crate::ui::input::{InputSize, text_field};
    use t3_contracts::{model::PROVIDER_DISPLAY_NAMES, provider_instance::ProviderDriverKind};
    let drivers: Vec<_> = PROVIDER_DISPLAY_NAMES
        .iter()
        .filter(|(driver, _)| *driver != "acpRegistry" && provider_driver_allowed(config, driver))
        .copied()
        .collect();
    let first_driver = drivers.first()?.0;
    let used = config
        .settings
        .provider_instances
        .keys()
        .chain(config.providers.0.iter().map(|row| &row.instance_id))
        .map(|id| id.0.clone())
        .chain(PROVIDER_DISPLAY_NAMES.iter().map(|(id, _)| id.to_string()))
        .collect::<BTreeSet<_>>();
    let open = signal(false);
    let opening = action(
        "Add provider",
        ButtonVariant::GhostMuted,
        false,
        move || open.set(true),
    );
    Some(
        view()
            .app_class(tw(
                "flex flex-col border-t border-border/50 px-3 py-3 sm:px-4",
            ))
            .child(opening)
            .child(region(
                move || open.get(),
                move |visible| {
                    visible.then(|| {
                        let id = signal(String::new());
                        let name = signal(String::new());
                        let driver = signal(first_driver.to_owned());
                        let mut props = SelectProps::new("Provider");
                        props.items = drivers
                            .iter()
                            .map(|(id, label)| SelectItem::new(*id, *label))
                            .collect::<Vec<_>>()
                            .into();
                        props.value =
                            Prop::bind(move || SelectValue::Controlled(Some(driver.get().into())));
                        props.size = SelectSize::Sm;
                        let used = Rc::new(used.clone());
                        let error = computed(move || {
                            let id = id.get();
                            t3_client::services::providers::provider_instance_id_error(
                                id.trim(),
                                used.contains(id.trim()),
                            )
                            .map(|message| (message, id))
                        });
                        let (page, env) = (page.clone(), env.clone());
                        let save = move || {
                            if error.get_untracked().is_some() {
                                return;
                            }
                            let id = ProviderInstanceId(id.get_untracked().trim().to_owned());
                            let name = name.get_untracked().trim().to_owned();
                            let instance = ProviderInstanceConfig {
                                driver: ProviderDriverKind(driver.get_untracked()),
                                display_name: (!name.is_empty()).then_some(Some(name)),
                                accent_color: None,
                                config: None,
                                environment: None,
                                enabled: Some(true),
                            };
                            open.set(false);
                            let future = (page.services.command)(
                                env.clone(),
                                Command::Instance(ProviderInstanceMutation::Create {
                                    instance_id: id.clone(),
                                    instance,
                                }),
                            );
                            let selected = page.selected;
                            page.tasks.spawn(async move {
                                match future.await {
                                    Ok(()) => {
                                        let _ = selected.try_set(Some(id));
                                    }
                                    Err(reason) => {
                                        show_error("Could not add provider instance", reason)
                                    }
                                }
                            });
                        };
                        view()
                            .app_class(tw("flex flex-col gap-2"))
                            .child(text(t("Add provider")).role(AccessibilityRole::Header))
                            .child(select(props, move |next| {
                                if let Some(next) = next {
                                    driver.set(next.to_string());
                                }
                            }))
                            .child(text_field(
                                id,
                                String::new(),
                                t("Instance ID"),
                                InputSize::Default,
                                save.clone(),
                            ))
                            .child(text_field(
                                name,
                                String::new(),
                                t("Display name"),
                                InputSize::Default,
                                save.clone(),
                            ))
                            .child(region(
                                move || error.get(),
                                move |error| {
                                    error.filter(|(_, id)| !id.is_empty()).map(|(message, id)| {
                                        text(tr(message, &[("id", MsgValue::Text(id))]))
                                            .app_class(tw("text-xs text-destructive"))
                                    })
                                },
                            ))
                            .child(button(
                                ButtonContent::Label(t("Add provider")),
                                ButtonVariant::Default,
                                ButtonSize::Xs,
                                move || error.get().is_some(),
                                save,
                            ))
                            .child(action("Cancel", ButtonVariant::Outline, false, move || {
                                open.set(false)
                            }))
                    })
                },
            ))
            .into_element(),
    )
}
