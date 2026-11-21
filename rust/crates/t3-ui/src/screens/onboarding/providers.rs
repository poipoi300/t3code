//! Port of apps/web/src/components/onboarding/WelcomePage.tsx AgentsSection and AgentRow.
use crate::{
    i18n::{Text, Value, t, tr},
    runtime::{
        Shared,
        services::terminal::TerminalServices,
        tasks::{TaskOwner, watch},
    },
    state::{
        nav::{Route, SettingsPage, navigator},
        url::{UrlRequest, UrlState},
    },
    theme::{cn, tw},
    ui::{
        button::{ButtonContent, ButtonSize, ButtonVariant, button},
        icon::{Icon, icon_with_class, provider_icon},
        toast::{ToastOptions, Variant, toast_manager},
    },
};
use futures::StreamExt;
use nsh_core::prelude::*;
use std::rc::Rc;
use t3_client::{
    composition::{ClientRuntime, first_run::CodexSetupMode},
    platform::Platform,
    services::{providers::provider_instance_display_name, terminal_sessions::TerminalLaunch},
    types::{EnvironmentId, ThreadId, ThreadKey},
    view_logic::provider_readiness::{
        self as logic, OnboardingProviderState as State, WelcomeAgents,
    },
};
use t3_contracts::{
    provider_instance::ProviderInstanceId,
    server::{ServerConfig, ServerProvider},
};

fn allowed(config: Option<&ServerConfig>, driver: &str) -> bool {
    config
        .and_then(|cfg| {
            cfg.managed_policy
                .as_ref()?
                .limits
                .as_ref()?
                .providers
                .as_ref()?
                .allowed_drivers
                .as_ref()
        })
        .is_none_or(|allowed| allowed.iter().any(|kind| kind.0 == driver))
}
pub(super) fn groups(config: Option<&ServerConfig>) -> WelcomeAgents<'_> {
    logic::group_welcome_agents(
        config.map(|cfg| cfg.providers.0.as_slice()),
        config.map(|cfg| &cfg.settings),
        provider_instance_display_name,
        |provider| allowed(config, &provider.driver.0),
        |a, b| {
            a.to_lowercase()
                .cmp(&b.to_lowercase())
                .then_with(|| a.cmp(b))
        },
    )
}

#[derive(Clone, PartialEq)]
struct SetupTerminal {
    key: ThreadKey,
    terminal: String,
    cwd: String,
    command: String,
    instance: ProviderInstanceId,
}
struct Agents<P: Platform + Clone + 'static> {
    client: ClientRuntime<P>,
    id: EnvironmentId,
    tasks: TaskOwner,
    terminal: Signal<Option<SetupTerminal>>,
    card: Signal<Option<ProviderInstanceId>>,
    signing_in: Signal<bool>,
    cancel_sign_in: std::cell::RefCell<Option<futures::future::AbortHandle>>,
    pending_cli: Signal<Option<ProviderInstanceId>>,
    more: Signal<Option<bool>>,
}

pub(super) fn section<P: Platform + Clone + 'static>(
    client: ClientRuntime<P>,
    id: EnvironmentId,
    config: Memo<Option<Shared<ServerConfig>>>,
    label: Memo<String>,
) -> AnyElement {
    let state = stored(Rc::new(Agents {
        client: client.clone(),
        id: id.clone(),
        tasks: TaskOwner::here(),
        terminal: signal(None),
        card: signal(None),
        signing_in: signal(false),
        cancel_sign_in: Default::default(),
        pending_cli: signal(None),
        more: signal(None),
    }));
    let refresh = client.first_run();
    watch(async move {
        let _ = refresh.refresh_providers(&id, None).await;
    });
    let more = state.get().more;
    effect(move || {
        let cfg = config.get();
        let grouped = groups(cfg.as_deref());
        if !grouped.checking && grouped.found.is_empty() && more.get_untracked().is_none() {
            more.set(Some(true));
        }
    });
    effect(move || {
        let s = state.get();
        let _ = s.card.get();
        if let Some(handle) = s.cancel_sign_in.take() {
            handle.abort();
        }
        s.signing_in.set(false);
    });
    let pending = state.get().pending_cli;
    effect(move || {
        let Some(id) = pending.get() else {
            return;
        };
        let cfg = config.get();
        let Some(cfg) = cfg else { return };
        let Some(p) = cfg.providers.0.iter().find(|p| p.instance_id == id) else {
            return;
        };
        match logic::cli_terminal_step(p, Some(&cfg.settings)) {
            logic::CliTerminalStep::Wait => {}
            step => {
                pending.set(None);
                if step == logic::CliTerminalStep::Open {
                    open_terminal(state, &cfg, p);
                }
            }
        }
    });
    let terminal = state.get().terminal;
    view()
        .app_class(tw(
            "grid gap-x-10 border-b border-border/60 py-4 sm:grid-cols-[6rem_minmax(0,1fr)]",
        ))
        .child(
            text(t("Agents"))
                .role(AccessibilityRole::Header)
                .app_class(tw("text-sm leading-10 text-muted-foreground")),
        )
        .child(
            view()
                .app_class(tw("min-w-0"))
                .child(dynamic(move || {
                    let cfg = config.get();
                    let label = label.get();
                    untracked(|| agents_list(state, cfg.as_deref(), label))
                }))
                .child(dynamic(move || {
                    let cfg = config.get();
                    let card = state.get().card.get();
                    untracked(|| codex_card(state, cfg.as_deref(), card))
                }))
                .child(dynamic(move || {
                    terminal
                        .get()
                        .map(|session| untracked(|| agent_install_terminal(state, session)))
                })),
        )
        .into_element()
}

fn agents_list<P: Platform + Clone + 'static>(
    state: Stored<Rc<Agents<P>>>,
    cfg: Option<&ServerConfig>,
    label: String,
) -> AnyElement {
    let group = groups(cfg);
    let can_manage = {
        let s = state.get();
        s.client.first_run().can_manage(&s.id)
    };
    let can_operate = {
        let s = state.get();
        s.client.terminals().can_operate(&s.id)
    };
    let mut body = view();
    if let Some(cfg) = cfg {
        for p in &group.found {
            body = body.child(agent_row(state, cfg, p, false, can_manage, can_operate));
        }
    }
    if group.checking && group.found.is_empty() {
        body = body.child(
            text(tr("Checking {label}…", &[("label", Value::Text(label))]))
                .app_class(tw("text-sm leading-10 text-muted-foreground")),
        );
    }
    let acp = allowed(cfg, "acpRegistry");
    let listed = || group.found.iter().chain(group.available.iter()).copied();
    if !can_operate
        && listed().any(|p| {
            cfg.is_some_and(|cfg| command(cfg, p).is_some())
                && matches!(
                    logic::welcome_agent_state(p, cfg.map(|c| &c.settings)),
                    State::Install | State::SignIn
                )
                || logic::codex_needs_setup(p, cfg.map(|c| &c.settings))
        })
    {
        body = body.child(
            text(t("This connection cannot control terminals."))
                .app_class(tw("pl-8 text-xs text-muted-foreground")),
        );
    }
    if !can_manage
        && (acp
            || listed().any(|p| {
                logic::welcome_agent_state(p, cfg.map(|c| &c.settings)) == State::Disabled
                    || logic::codex_needs_setup(p, cfg.map(|c| &c.settings))
            }))
    {
        body = body.child(
            text(t("This connection cannot manage agents."))
                .app_class(tw("pl-8 text-xs text-muted-foreground")),
        );
    }
    let toggle = if group.found.is_empty() {
        "Set up an agent"
    } else {
        "Add more agents"
    };
    let more = state.get().more;
    let mut trigger = nsh_base::button()
        .accessibility_label(SharedString::from(t(toggle)))
        .on_press(move || more.update(|expanded| *expanded = Some(!expanded.unwrap_or(false))))
        .app_class(tw("group flex min-h-10 w-full flex-row items-center gap-3 rounded-md text-left outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-inset"))
        .child(icon_with_class(Icon::Plus, "size-4"))
        .child(text(t(toggle)).app_class(tw("text-sm group-hover:text-foreground")));
    for p in group.available.iter().take(4) {
        let driver = p.driver.0.clone();
        let name = provider_instance_display_name(p);
        trigger = trigger.child(when(
            move || !more.get().unwrap_or(false),
            move || provider_icon(&driver, &name, "size-4"),
        ));
    }
    body = body.child(trigger.child(icon_with_class(
        Icon::ChevronDown,
        "ml-auto size-4 text-muted-foreground",
    )));
    let mut grid = view().app_class(tw("grid grid-cols-1 gap-x-8 sm:grid-cols-2"));
    if let Some(cfg) = cfg {
        for p in &group.available {
            grid = grid.child(agent_row(state, cfg, p, true, can_manage, can_operate));
        }
    }
    let mut extra = view()
        .app_class(move || {
            tw(if more.get().unwrap_or(false) {
                "flex flex-col"
            } else {
                "hidden"
            })
        })
        .child(grid);
    if acp {
        extra = extra.child(
            view()
                .app_class(tw("flex min-h-9 flex-row items-center gap-3"))
                .child(provider_icon("acpRegistry", "ACP Registry", "size-4"))
                .child(
                    text(t("More agents in the ACP Registry"))
                        .app_class(tw("min-w-0 flex-1 text-sm text-muted-foreground")),
                )
                .child(registry_browse(state, can_manage)),
        );
    }
    body.child(extra).into_element()
}

fn codex_card<P: Platform + Clone + 'static>(
    state: Stored<Rc<Agents<P>>>,
    cfg: Option<&ServerConfig>,
    card: Option<ProviderInstanceId>,
) -> AnyElement {
    let s = state.get();
    let can_manage = s.client.first_run().can_manage(&s.id);
    let can_operate = s.client.terminals().can_operate(&s.id);
    let mut body = view();
    if let Some(cfg) = cfg
        && let Some(p) = cfg.providers.0.iter().find(|p| {
            Some(&p.instance_id) == card.as_ref()
                && logic::codex_needs_setup(p, Some(&cfg.settings))
        })
    {
        let id = p.instance_id.clone();
        let sign_in = id.clone();
        let busy = state.get().signing_in;
        let service = state.get().client.first_run();
        let env = state.get().id.clone();
        let local = service.local_sign_in(&env);
        let capable = p
            .setup
            .as_ref()
            .and_then(Option::as_ref)
            .is_some_and(|setup| setup.can_authenticate);
        let managed = cfg
            .settings
            .provider_instances
            .get(&p.instance_id)
            .and_then(|instance| instance.config.as_ref())
            .and_then(|config| config.get("setupMode"))
            .and_then(|mode| mode.as_str())
            == Some("managed");
        let mode_locked = cfg
            .managed_policy
            .as_ref()
            .and_then(|p| p.limits.as_ref())
            .and_then(|limits| limits.providers.as_ref())
            .and_then(|p| p.locked_instance_fields.as_ref())
            .and_then(|locks| locks.get(&p.driver))
            .is_some_and(|paths| paths.iter().any(|p| p == "config.setupMode"));
        let opener = crate::runtime::services::links::link_opener_here();
        let description = if local {
            "Code with your ChatGPT subscription."
        } else {
            "ChatGPT sign-in is unavailable on this remote connection."
        };
        body = body.child(
            view()
                .app_class(tw("mt-2"))
                .child(text(provider_instance_display_name(p)))
                .child(text(t(description)))
                .child(button(
                    ButtonContent::Label(t(if managed {
                        "Open sign-in page"
                    } else {
                        "Continue with ChatGPT"
                    })),
                    ButtonVariant::Outline,
                    ButtonSize::Sm,
                    move || {
                        !local
                            || (managed && !capable)
                            || (!managed && mode_locked)
                            || !can_manage
                            || busy.get()
                    },
                    move || {
                        if busy.get_untracked() {
                            return;
                        }
                        busy.set(true);
                        let (service, env, id, opener) = (
                            service.clone(),
                            env.clone(),
                            sign_in.clone(),
                            opener.clone(),
                        );
                        let (abort, registration) = futures::future::AbortHandle::new_pair();
                        state.get().cancel_sign_in.replace(Some(abort));
                        state.get().tasks.spawn(async move {
                            let setup = async {
                                if !managed {
                                    service
                                        .enable_provider(&env, &id, Some(CodexSetupMode::Managed))
                                        .await?;
                                }
                                service.sign_in_page(&env, id).await
                            };
                            if let Ok(page) =
                                futures::future::Abortable::new(setup, registration).await
                            {
                                busy.set(false);
                                match page {
                                    Ok(page) => opener.open(page),
                                    Err(error) => {
                                        show_error("Could not start sign-in".into(), error)
                                    }
                                }
                            }
                        });
                    },
                ))
                .child(button(
                    ButtonContent::Label(t("Use existing CLI")),
                    ButtonVariant::GhostMuted,
                    ButtonSize::Sm,
                    move || !can_operate || !can_manage || (managed && mode_locked),
                    move || enable(state, id.clone(), true),
                )),
        );
    }
    body.into_element()
}

fn show_error(title: Text, error: impl std::fmt::Display) {
    toast_manager().add(ToastOptions {
        variant: Variant::Error,
        title,
        description: Some(Text::client_key(error.to_string())),
        ..Default::default()
    });
}
fn enable<P: Platform + Clone + 'static>(
    state: Stored<Rc<Agents<P>>>,
    id: ProviderInstanceId,
    existing_cli: bool,
) {
    {
        let s = state.get();
        let service = s.client.first_run();
        let env = s.id.clone();
        let pending = s.pending_cli;
        s.tasks.spawn(async move {
            match service
                .enable_provider(&env, &id, existing_cli.then_some(CodexSetupMode::Existing))
                .await
            {
                Err(error) => show_error(
                    if existing_cli {
                        "Could not save Codex setup".into()
                    } else {
                        tr(
                            "Could not turn on {agent}",
                            &[("agent", Value::Text(id.0.clone()))],
                        )
                        .into()
                    },
                    error,
                ),
                Ok(()) if existing_cli => pending.set(Some(id)),
                Ok(()) => {}
            }
        });
    };
}

fn command(cfg: &ServerConfig, p: &ServerProvider) -> Option<String> {
    let os = cfg.environment.platform.os.clone();
    logic::onboarding_install_command(&p.driver.0, os.clone()).map(|install| {
        if p.installed {
            logic::onboarding_login_command(p, &cfg.settings, os)
        } else {
            install.to_owned()
        }
    })
}
fn open_terminal<P: Platform + Clone + 'static>(
    state: Stored<Rc<Agents<P>>>,
    cfg: &ServerConfig,
    p: &ServerProvider,
) {
    let Some(command) = command(cfg, p) else {
        return;
    };
    {
        let s = state.get();
        if !s.client.terminals().can_operate(&s.id)
            || s.terminal.with(|term| {
                term.as_ref()
                    .is_some_and(|term| term.instance == p.instance_id)
            })
        {
            return;
        }
        match s.client.first_run().setup_terminal_id(&p.driver.0) {
            Ok(terminal) => s.terminal.set(Some(SetupTerminal {
                key: ThreadKey {
                    environment: s.id.clone(),
                    thread: ThreadId("onboarding-agent-setup".into()),
                },
                terminal,
                command,
                cwd: cfg.cwd.0.clone(),
                instance: p.instance_id.clone(),
            })),
            Err(error) => show_error("Could not open the setup terminal.".into(), error),
        }
    };
}

fn agent_row<P: Platform + Clone + 'static>(
    state: Stored<Rc<Agents<P>>>,
    cfg: &ServerConfig,
    p: &ServerProvider,
    compact: bool,
    can_manage: bool,
    can_operate: bool,
) -> AnyElement {
    let status = logic::welcome_agent_state(p, Some(&cfg.settings));
    let name = provider_instance_display_name(p);
    let mut details = view()
        .app_class(tw("min-w-0 flex-1"))
        .child(text(name.clone()).app_class(tw(&cn([
            Some("block truncate text-sm text-foreground"),
            (!compact).then_some("font-medium"),
        ]))));
    let detail =
        if status == State::Install && !matches!(p.driver.0.as_str(), "claudeAgent" | "codex") {
            Some(t("Not installed"))
        } else if !compact && matches!(status, State::Attention | State::SignIn) {
            p.unavailable_reason
                .as_ref()
                .and_then(Option::as_ref)
                .or_else(|| {
                    p.compatibility_advisory
                        .as_ref()
                        .and_then(|a| a.message.as_ref())
                })
                .or_else(|| p.message.as_ref().and_then(Option::as_ref))
                .map(|v| v.0.clone())
        } else {
            None
        };
    if let Some(detail) = detail {
        details = details.child(text(detail).app_class(tw(
            "line-clamp-2 text-xs leading-relaxed text-muted-foreground",
        )));
    }
    let action = match status {
        State::Ready => view()
            .app_class(tw("inline-flex flex-row items-center gap-1.5 text-xs font-medium text-success-foreground"))
            .child(icon_with_class(Icon::Check, "size-3.5"))
            .child(text(t("Ready")))
            .into_element(),
        State::Checking => text(t("Checking…")).app_class(tw("text-xs text-muted-foreground")).into_element(),
        _ => {
            let id = p.instance_id.clone();
            let env = state.get().id.clone();
            let nav = navigator();
            let urls = use_context::<UrlState>();
            let settings = move || {
                use t3_client::view_logic::shell::navigation::encode_component;
                if let Some(urls) = urls {
                    let path = format!("/settings/providers?environmentId={}&instanceId={}",
                        encode_component(&env.0), encode_component(&id.0));
                    urls.request(UrlRequest::Push(path));
                } else {
                    nav.open(Route::Settings(SettingsPage::Providers));
                }
            };
            let id = p.instance_id.clone();
            let installable = matches!(status, State::Install | State::SignIn) && command(cfg,p).is_some();
            let setup = logic::codex_needs_setup(p, Some(&cfg.settings));
            let terminal_action = installable && !setup && !logic::is_broken_version(p);
            let (label, disabled, press): (&str, bool, Rc<dyn Fn()>) =
                if status == State::Disabled {
                    ("Turn on", !can_manage, Rc::new(move || enable(state,id.clone(),false)))
                } else if logic::is_broken_version(p) {
                    ("Update", false, Rc::new(settings))
                } else if setup {
                    ("Set up", false, Rc::new(move || {
                        state.get().card.update(|card| {
                            *card = (card.as_ref() != Some(&id)).then(|| id.clone());
                        });
                    }))
                } else if terminal_action {
                    let label = if status == State::SignIn { "Sign in" } else { "Install" };
                    (label, !can_operate, Rc::new(move || { let s = state.get();
                        let snapshot = s.client.store().snapshot();
                        let Some(cfg) = snapshot.environments.get(&s.id).and_then(|e| e.config.as_ref()) else {return};
                        if let Some(provider) = cfg.providers.0.iter().find(|p| p.instance_id == id) {
                            open_terminal(state, cfg, provider);
                        }
                     }))
                } else {
                    let label = match status { State::SignIn => "Sign in", State::Attention => "Details", _ => "Set up" };
                    (label, false, Rc::new(settings))
                };
            let instance = p.instance_id.clone();
            let disabled = move || disabled || terminal_action && state.get().terminal.with(|term|
                term.as_ref().is_some_and(|term|term.instance == instance));
            button(ButtonContent::Custom{ label: format!("{}: {name}",t(label)), child: text(t(label)).into_element() },
                ButtonVariant::Outline, ButtonSize::Xs, disabled, move || press()).into_element()
        }
    };
    view()
        .app_class(tw(if compact {
            "flex min-h-9 flex-row items-center gap-3"
        } else {
            "flex min-h-10 flex-row items-center gap-3 py-1"
        }))
        .child(provider_icon(
            &p.driver.0,
            &name,
            if compact { "size-4" } else { "size-[18px]" },
        ))
        .child(details)
        .child(view().app_class(tw("shrink-0")).child(action))
        .into_element()
}

fn agent_install_terminal<P: Platform + Clone + 'static>(
    state: Stored<Rc<Agents<P>>>,
    session: SetupTerminal,
) -> AnyElement {
    use t3_client::composition::first_run::SetupTerminalState::*;
    let client = state.get().client.clone();
    let first_run = client.first_run();
    let terminals = TerminalServices::new(client.terminals());
    let phase = signal(Preparing);
    let mut events = first_run.setup_terminal(
        session.key.clone(),
        session.terminal.clone(),
        session.cwd.clone(),
        session.instance.clone(),
        session.command.clone(),
    );
    watch(async move {
        while let Some(event) = events.next().await {
            phase.set(event);
        }
    });
    let close = move || {
        {
            let s = state.get();
            let service = s.client.first_run();
            let id = s.id.clone();
            let tasks = s.tasks;
            s.terminal.set(None);
            tasks.spawn(async move {
                let _ = service.refresh_providers(&id, None).await;
            });
        };
    };
    let retry = session.clone();
    let command = session.command.clone();
    view().app_class(tw("mt-4 overflow-hidden rounded-lg border border-border/70 bg-background text-foreground"))
        .child(view().app_class(tw("flex flex-row items-center justify-between border-b border-border/60 bg-background/60 px-3 py-1.5"))
            .child(text(move || SharedString::from(match phase.get(){
                Preparing=>t("Preparing command..."),Ready=>t("Review the command, then press Enter to run it."),
                OpenFailed=>t("Could not open the setup terminal."),WriteFailed=>tr("Run {command} in this terminal.",&[("command",Value::Text(command.clone()))])
            })).app_class(tw("text-2xs font-medium text-muted-foreground")))
            .child(dynamic(move || (phase.get()==OpenFailed).then(||{
                let retry=retry.clone();
                button(ButtonContent::Label(t("Retry")),ButtonVariant::Outline,ButtonSize::Xs,||false,move ||{
                    { let s = state.get();
                        if let Ok(id)=s.client.first_run().setup_terminal_id("retry"){
                            s.terminal.set(Some(SetupTerminal{terminal:id,..retry.clone()}));
                        }
                     };
                })
            })))
            .child(button(ButtonContent::Label(t("Close")),ButtonVariant::GhostMuted,ButtonSize::Xs,||false,close)))
        .child(dynamic(move|| matches!(phase.get(),Ready|WriteFailed).then(||untracked(||view().h(px(240.0)).child(crate::screens::terminal::session::terminal_viewport(terminals.clone(),session.key.clone(),session.terminal.clone(),t("Setup terminal"),TerminalLaunch {cwd:session.cwd.clone(),worktree_path:None,env:Default::default()},computed(||true),Rc::new(close))).into_element()))))
        .into_element()
}

fn registry_browse<P: Platform + Clone + 'static>(
    state: Stored<Rc<Agents<P>>>,
    can_manage: bool,
) -> AnyElement {
    use crate::ui::dialog::{dialog, dialog_popup, dialog_title};
    let open = signal(false);
    dialog(open, move |modal| {
        view()
            .child(button(
                ButtonContent::Label(t("Browse")),
                ButtonVariant::Outline,
                ButtonSize::Xs,
                move || !can_manage,
                move || modal.show(),
            ))
            .child(dialog_popup(modal, Default::default(), move || {
                view()
                    .child(dialog_title(modal, || "Add provider instance".into()))
                    .child(registry_search(state, modal))
            }))
    })
    .into_element()
}

fn registry_search<P: Platform + Clone + 'static>(
    state: Stored<Rc<Agents<P>>>,
    modal: crate::ui::dialog::DialogState,
) -> AnyElement {
    use crate::ui::input::{InputSize, text_field};
    use t3_contracts::acp_registry::AcpRegistrySearchAgent;
    let query = signal(String::new());
    let agents = signal(None::<Vec<AcpRegistrySearchAgent>>);
    let selected = signal(None::<AcpRegistrySearchAgent>);
    let label = signal(String::new());
    let busy = signal(false);
    let error = signal(String::new());
    let tasks = TaskOwner::here();
    let request = signal(0_u64);
    let search = move || {
        let query = query.get_untracked();
        request.update(|n| *n += 1);
        let version = request.get_untracked();
        let (api, id) = {
            let s = state.get();
            (s.client.first_run(), s.id.clone())
        };
        tasks.spawn(async move {
            let result = api.search_registry(&id, &query).await;
            if request.get_untracked() != version {
                return;
            }
            match result {
                Ok(result) => {
                    agents.set(Some(result));
                    error.set(String::new());
                }
                Err(err) => error.set(err.to_string()),
            }
        });
    };
    search();
    view()
        .child(dynamic(move || {
            let selected_agent = selected.get();
            untracked(|| {
                if let Some(agent) = selected_agent {
                    view()
                        .child(text_field(
                            label,
                            t("Display name"),
                            t("Display name"),
                            InputSize::Default,
                            || {},
                        ))
                        .child(button(
                            ButtonContent::Label(t("Add provider")),
                            ButtonVariant::Default,
                            ButtonSize::Sm,
                            move || busy.get() || label.get().trim().is_empty(),
                            move || {
                                if busy.get_untracked() {
                                    return;
                                }
                                busy.set(true);
                                let agent = agent.clone();
                                let name = label.get_untracked();
                                let (api, id) = {
                                    let s = state.get();
                                    (s.client.first_run(), s.id.clone())
                                };
                                tasks.spawn(async move {
                                    let result = api.add_registry_agent(&id, &agent, &name).await;
                                    busy.set(false);
                                    match result {
                                        Ok(()) => modal.close(),
                                        Err(err) => error.set(err.to_string()),
                                    }
                                });
                            },
                        ))
                        .into_element()
                } else {
                    view()
                        .child(text_field(
                            query,
                            t("Search agents…"),
                            t("Search ACP Registry"),
                            InputSize::Default,
                            search,
                        ))
                        .child(button(
                            ButtonContent::Label(t("Search")),
                            ButtonVariant::Outline,
                            ButtonSize::Sm,
                            move || busy.get(),
                            search,
                        ))
                        .child(dynamic(move || {
                            let list = agents.get();
                            untracked(|| {
                                let mut rows = view().app_class(tw("grid gap-3"));
                                for agent in list.unwrap_or_default() {
                                    let name = agent.name.0.clone();
                                    rows = rows.child(
                                        view()
                                            .child(text(name.clone()))
                                            .child(text(agent.description.clone()))
                                            .child(button(
                                                ButtonContent::Custom {
                                                    label: tr(
                                                        "Add {name}",
                                                        &[("name", Value::Text(name))],
                                                    ),
                                                    child: text(t("Add")).into_element(),
                                                },
                                                ButtonVariant::Outline,
                                                ButtonSize::Xs,
                                                move || busy.get(),
                                                move || {
                                                    if busy.get_untracked() {
                                                        return;
                                                    }
                                                    busy.set(true);
                                                    let agent = agent.clone();
                                                    let (api, id) = {
                                                        let s = state.get();
                                                        (s.client.first_run(), s.id.clone())
                                                    };
                                                    tasks.spawn(async move {
                                                        let result = api
                                                            .prepare_registry_agent(&id, &agent)
                                                            .await;
                                                        busy.set(false);
                                                        match result {
                                                            Ok(()) => {
                                                                label.set(agent.name.0.clone());
                                                                selected.set(Some(agent));
                                                            }
                                                            Err(err) => error.set(err.to_string()),
                                                        }
                                                    });
                                                },
                                            )),
                                    );
                                }
                                rows
                            })
                        }))
                        .into_element()
                }
            })
        }))
        .child(text(move || SharedString::from(error.get())))
        .into_element()
}
