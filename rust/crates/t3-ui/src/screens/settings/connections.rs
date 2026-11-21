//! Port of ConnectionsSettings.tsx, SavedEnvironmentRows.tsx and EnvironmentRoutesList.tsx.
use super::shell::{region, settings_page_container, settings_section};
use crate::{
    i18n::{Text, Value, t, tr},
    runtime::{rt, tasks::TaskOwner},
    theme::tw,
    ui::{
        button::{ButtonContent, ButtonSize, ButtonVariant, button},
        confirm::{ConfirmDialogOptions, ConfirmDialogVariant, request_confirm_dialog},
        switch::{ControlState, Size, switch},
    },
};
use futures::future::LocalBoxFuture;
use nsh_core::prelude::*;
use std::rc::Rc;
use t3_client::{
    ClientError,
    composition::{ClientRuntime, EnvironmentEntry},
    connection::{ConnectionDiagnosis, ConnectionMode, FailureKind, HopResult, Phase},
    platform::Platform,
    services::settings::{SavedRouteEdit, saved_route_address, saved_route_label},
    types::EnvironmentId,
};
use t3_client::{
    authorization::Sensitive,
    services::settings::{PeerDestination, PeerLinkFailure},
    thread_views::link_request::LinkableMachine,
};
use t3_contracts::{
    auth::{AuthEnvironmentScope, AuthMcpClientAccess},
    peer_link::PeerLinkSummary,
};
type LinkCall = LocalBoxFuture<'static, Result<(), PeerLinkFailure>>;

type Call = LocalBoxFuture<'static, Result<(), ClientError>>;
pub trait ConnectionSettings {
    fn machines(&self, source: &EnvironmentId, links: &[PeerLinkSummary]) -> Vec<LinkableMachine>;
    fn link(
        &self,
        source: EnvironmentId,
        dest: PeerDestination,
        code: Option<Sensitive<String>>,
        access: AuthMcpClientAccess,
    ) -> LinkCall;
    fn diagnose(&self, id: &EnvironmentId) -> Option<ConnectionDiagnosis>;
    fn enabled(&self, id: &EnvironmentId, enabled: bool) -> Call;
    fn retry(&self, id: &EnvironmentId) -> Call;
    fn remove_saved(&self, id: &EnvironmentId) -> Call;
    fn relay_state(&self) -> Option<t3_client::relay::RelayDiscoveryState>;
    fn relay_changes(&self) -> futures::stream::LocalBoxStream<'static, ()>;
    fn add_relay(&self, id: &EnvironmentId, label: &str) -> Call;
    fn edit_routes(&self, id: EnvironmentId, observed: Vec<String>, edit: SavedRouteEdit) -> Call;
    fn grants(&self, id: &EnvironmentId, scope: AuthEnvironmentScope) -> Option<bool>;
    fn active_route(&self, id: &EnvironmentId) -> Option<String>;
    fn peer_session(&self, id: &EnvironmentId) -> Option<t3_client::rpc::SessionEndpoint>;
}

impl<P: Platform + Clone + 'static> ConnectionSettings for ClientRuntime<P> {
    fn peer_session(&self, id: &EnvironmentId) -> Option<t3_client::rpc::SessionEndpoint> {
        self.settings_peer_session(id)
    }
    fn machines(&self, source: &EnvironmentId, links: &[PeerLinkSummary]) -> Vec<LinkableMachine> {
        self.settings_linkable_machines(source, links)
    }
    fn link(
        &self,
        source: EnvironmentId,
        dest: PeerDestination,
        code: Option<Sensitive<String>>,
        access: AuthMcpClientAccess,
    ) -> LinkCall {
        self.link_settings_peer(source, dest, code, access)
    }
    fn diagnose(&self, id: &EnvironmentId) -> Option<ConnectionDiagnosis> {
        let mut diagnosis = self.diagnosis(id, option_env!("T3_APP_VERSION"))?;
        // Server errors and trace IDs are unconstrained strings and may contain credentials.
        diagnosis
            .facts
            .retain(|(label, _)| !matches!(label.as_str(), "Error" | "Trace ID"));
        if let Some(untrusted) = self
            .client(id)
            .and_then(|c| c.state().last_failure)
            .and_then(|f| f.verification_code)
            .filter(|code| !valid_device_display_code(code))
        {
            for step in &mut diagnosis.steps {
                if !untrusted.is_empty() {
                    *step = step.replace(&untrusted, "<code>");
                }
            }
        }
        Some(diagnosis)
    }
    fn enabled(&self, id: &EnvironmentId, enabled: bool) -> Call {
        self.set_saved_environment_enabled(id, enabled)
    }
    fn remove_saved(&self, id: &EnvironmentId) -> Call {
        self.remove(id)
    }
    fn relay_state(&self) -> Option<t3_client::relay::RelayDiscoveryState> {
        self.relay_environments()
    }
    fn relay_changes(&self) -> futures::stream::LocalBoxStream<'static, ()> {
        ClientRuntime::relay_changes(self)
    }
    fn add_relay(&self, id: &EnvironmentId, label: &str) -> Call {
        self.add_relay_environment(id, label)
    }
    fn retry(&self, id: &EnvironmentId) -> Call {
        self.reconnect(id)
    }
    fn edit_routes(&self, id: EnvironmentId, observed: Vec<String>, edit: SavedRouteEdit) -> Call {
        self.edit_saved_routes(id, observed, edit)
    }
    fn grants(&self, id: &EnvironmentId, scope: AuthEnvironmentScope) -> Option<bool> {
        self.client(id)?.session()?.grants_scope(scope)
    }
    fn active_route(&self, id: &EnvironmentId) -> Option<String> {
        self.client(id)?.active_route_id()
    }
}

// The public comparison code derived by t3-auth is two groups of four characters.
fn valid_device_display_code(code: &str) -> bool {
    let Some((first, last)) = code.split_once('-') else {
        return false;
    };
    first.len() == 4
        && last.len() == 4
        && first
            .bytes()
            .chain(last.bytes())
            .all(|b| b"23456789ABCDEFGHJKLMNPQRSTUVWXYZ".contains(&b))
}

pub(super) type Services = Stored<Rc<dyn ConnectionSettings>>;

pub(super) fn action(label: String, on_press: impl Fn() + 'static) -> AnyElement {
    button(
        ButtonContent::Label(label),
        ButtonVariant::GhostMuted,
        ButtonSize::Xs,
        || false,
        on_press,
    )
    .into_element()
}

pub(super) fn saved(entry: &EnvironmentEntry) -> bool {
    !entry.routes.first().is_some_and(|r| {
        r.mode == ConnectionMode::Local
            || (r.mode == ConnectionMode::Remote && r.id.starts_with("local:"))
    })
}

pub fn screen() -> AnyElement {
    let rt = rt();
    let services = rt.backend().connection_settings().map(stored);
    let form = signal(None);
    let relay_tick = signal(0u64);
    if let Some(s) = services {
        use futures::StreamExt;
        let mut changes = s.get().relay_changes();
        crate::runtime::tasks::watch(async move {
            while changes.next().await.is_some() {
                relay_tick.update(|v| *v += 1);
            }
        });
    }
    let rows = each(
        move || {
            rt.environments()
                .into_iter()
                .filter(saved)
                .collect::<Vec<_>>()
        },
        |e| e.id.clone(),
        move |row, _| saved_environment(row, services, form, relay_tick),
    );
    let empty = region(
        move || {
            relay_tick.get();
            !rt.environments().iter().any(saved)
                && services
                    .and_then(|s| s.get().relay_state())
                    .is_none_or(|state| !state.refreshing && state.environments.is_empty())
        },
        move |empty| {
            empty.then(|| {
                view()
                    .app_class(tw("min-h-52 items-center justify-center"))
                    .child(text(t("No saved remote environments")))
                    .child(text(t(if services.and_then(|s|s.get().relay_state()).is_some() {
                        "Click “Add environment” to pair another environment, or connect one from T3 Connect."
                    }else{"Click “Add environment” to pair another environment."})))
            })
        },
    );
    let section = settings_section(
        "remote-environments",
        "Environments".into(),
        view()
            .child(action(t("Add environment"), move || form.set(Some(None))))
            .child(rows)
            .child(services.map(|s| relay_rows(s, relay_tick)))
            .child(empty),
    );
    settings_page_container(
        view()
            .child(section)
            .child(services.map(linked_environments))
            .child(region(
                move || form.get(),
                move |request| {
                    request.map(|target| add_environment(form, target, services, relay_tick))
                },
            )),
    )
    .into_element()
}

#[derive(Clone, Copy)]
struct Commands {
    tasks: TaskOwner,
    busy: Signal<bool>,
    error: Signal<Option<&'static str>>,
}
impl Commands {
    fn run(self, error: &'static str, work: impl FnOnce() -> Call + 'static) {
        if self.busy.get_untracked() {
            return;
        }
        self.error.set(None);
        self.busy.set(true);
        let result = work();
        self.tasks.spawn(async move {
            if result.await.is_err() {
                self.error.set(Some(error));
            }
            self.busy.set(false);
        });
    }
}

type PairForm = Signal<Option<Option<(EnvironmentId, String)>>>;

fn saved_environment(
    row: ReadSignal<EnvironmentEntry>,
    services: Option<Services>,
    form: PairForm,
    relay_tick: Signal<u64>,
) -> AnyElement {
    let routes_open = signal(false);
    let permissions_open = signal(false);
    let details_open = signal(false);
    let commands = Commands {
        tasks: TaskOwner::here(),
        busy: signal(false),
        error: signal(None),
    };
    region(
        move || (row.get(), relay_tick.get()),
        move |(entry, _)| {
            let id = stored(entry.id.clone());
            let connected = entry.state.phase == Phase::Connected;
            let unsupported =
                entry.state.last_failure.as_ref().is_some_and(|f| {
                    f.server_update_required || f.kind == FailureKind::Unsupported
                });
            let enabled = entry.state.desired && !unsupported;
            let blocked = unsupported
                && !entry
                    .routes
                    .iter()
                    .any(|r| r.mode == ConnectionMode::Remote && !r.id.starts_with("learned:"));
            let diagnosis = services.and_then(|s| {
                (!unsupported)
                    .then(|| s.get().diagnose(&entry.id))
                    .flatten()
            });
            let status = if unsupported {
                t("Client not supported")
            } else if !enabled {
                t("Off")
            } else {
                match entry.state.phase {
                    Phase::Connected => t("Connected"),
                    Phase::Connecting => t("Connecting"),
                    Phase::Backoff => t("Reconnecting"),
                    Phase::Available => t("Not connected"),
                    Phase::Offline => t("Offline"),
                    Phase::Blocked => t("Connection failed"),
                }
            };
            let active = connected
                .then(|| services.and_then(|s| s.get().active_route(&entry.id)))
                .flatten();
            let count = entry.routes.len();
            let transport = entry
                .routes
                .first()
                .map(|route| {
                    if count > 1 {
                        if let Some(active) =
                            entry.routes.iter().find(|r| active.as_ref() == Some(&r.id))
                        {
                            return tr(
                                "via {route}",
                                &[("route", Value::Text(saved_route_label(active)))],
                            );
                        }
                        saved_route_label(route)
                    } else {
                        saved_route_address(route).unwrap_or_else(|| saved_route_label(route))
                    }
                })
                .unwrap_or_default();
            let summary = if diagnosis.is_some() {
                transport
            } else {
                [transport, status]
                    .into_iter()
                    .filter(|s| !s.is_empty())
                    .collect::<Vec<_>>()
                    .join(" · ")
            };
            let route_toggle = if count == 1 {
                t("Routes")
            } else {
                tr("{count} routes", &[("count", Value::Int(count as i64))])
            };
            let label = tr(
                if enabled {
                    "Switch off {name}"
                } else {
                    "Switch on {name}"
                },
                &[("name", Value::Text(entry.label.clone()))],
            );
            let switch_control = services.map(|s| {
                switch(
                    label,
                    ControlState {
                        checked: enabled.into(),
                        disabled: Prop::bind(move || blocked || commands.busy.get()),
                        ..Default::default()
                    },
                    Size::Sm,
                    move |next| {
                        commands.run(
                            if next {
                                "Could not switch backend on"
                            } else {
                                "Could not switch backend off"
                            },
                            move || s.get().enabled(&id.get(), next),
                        )
                    },
                )
            });
            let relay_managed = entry.routes.iter().any(|r| r.mode == ConnectionMode::Relay);
            let discovery = services
                .and_then(|s| s.get().relay_state())
                .and_then(|state| {
                    state
                        .environments
                        .get(&entry.id)
                        .map(|row| row.environment.endpoint.http_base_url.0.clone())
                });
            let mcp = t3_client::connection::environment_mcp_url(
                &entry.routes,
                active.as_deref(),
                discovery.as_deref(),
            )
            .and_then(|url| t3_client::connection::agent_consent::page_origin(&url))
            .map(|origin| format!("{origin}/mcp"));
            let routes = stored(entry.routes);
            let active = stored(active);
            let host = stored(entry.label.clone());
            view()
                .app_class(tw("min-w-0 px-3 py-2.5 sm:px-4"))
                .child(
                    view()
                        .flex_row()
                        .items_center()
                        .gap(px(8.0))
                        .child(
                            view()
                                .flex_1()
                                .min_w(px(0.0))
                                .child(
                                    text(entry.label).app_class(tw(
                                        "truncate text-sm font-medium text-foreground",
                                    )),
                                )
                                .child(
                                    text(summary)
                                        .app_class(tw("truncate text-xs text-muted-foreground")),
                                ),
                        )
                        .child(switch_control)
                        .child(services.map(|s| {
                            environment_menu(s, id, host, commands, relay_managed, routes_open, mcp)
                        })),
                )
                .child(
                    view()
                        .flex_row()
                        .child(action(route_toggle, move || {
                            routes_open.update(|v| *v = !*v)
                        }))
                        .child(action(t("Permissions"), move || {
                            permissions_open.update(|v| *v = !*v)
                        })),
                )
                .child(
                    diagnosis
                        .map(|d| diagnosis_details(d, host, id, services, commands, details_open)),
                )
                .child(region(
                    move || routes_open.get(),
                    move |open| {
                        open.then(|| {
                            routes_list(id, routes, active, services, commands, form, host)
                        })
                    },
                ))
                .child(region(
                    move || permissions_open.get(),
                    move |open| {
                        open.then(|| {
                            super::access::session_permissions(id.get(), connected, true, services)
                        })
                    },
                ))
                .child(region(
                    move || commands.error.get(),
                    |message| {
                        message.map(|m| {
                            text(Text::client_key(m).resolve())
                                .app_class(tw("text-xs text-destructive"))
                        })
                    },
                ))
        },
    )
}

fn routes_list(
    id: Stored<EnvironmentId>,
    routes: Stored<Vec<t3_client::connection::Route>>,
    active: Stored<Option<String>>,
    services: Option<Services>,
    commands: Commands,
    form: PairForm,
    host: Stored<String>,
) -> AnyElement {
    let saved = routes.get();
    let order = stored(saved.iter().map(|r| r.id.clone()).collect::<Vec<_>>());
    let mut list = view()
        .role(AccessibilityRole::List)
        .app_class(tw("mt-2 border-t border-border/50 py-2"));
    for (index, route) in saved.iter().enumerate() {
        let label = saved_route_label(route);
        let address = saved_route_address(route);
        let learned = route.id.starts_with("learned:");
        let mut line = view()
            .app_class(tw("flex flex-row items-center gap-2 py-2"))
            .child(
                view()
                    .flex_1()
                    .child(text(label.clone()).app_class(tw("text-xs font-medium text-foreground")))
                    .child(address.as_ref().map(|a| {
                        text(if learned {
                            format!("{a} · {}", t("found automatically"))
                        } else {
                            a.clone()
                        })
                        .app_class(tw("truncate text-2xs text-muted-foreground"))
                    }))
                    .child((active.get().as_ref() == Some(&route.id)).then(|| {
                        text(t("In use")).app_class(tw("text-2xs text-success-foreground"))
                    })),
            );
        if let Some(s) = services {
            for (next, glyph) in [
                (index.checked_sub(1), "↑"),
                ((index + 1 < saved.len()).then_some(index + 1), "↓"),
            ] {
                if let Some(next) = next {
                    let name = tr(
                        "Move {route} to position {position}",
                        &[
                            ("route", Value::Text(label.clone())),
                            ("position", Value::Int((next + 1) as i64)),
                        ],
                    );
                    line = line.child(
                        nsh_base::button()
                            .accessibility_label(SharedString::from(name))
                            .disabled(move || commands.busy.get())
                            .child(text(glyph))
                            .on_press(move || {
                                let observed = order.get();
                                let mut desired = observed.clone();
                                desired.swap(index, next);
                                commands.run("Reorder routes", move || {
                                    s.get().edit_routes(
                                        id.get(),
                                        observed,
                                        SavedRouteEdit::Order(desired),
                                    )
                                });
                            }),
                    );
                }
            }
            if !learned && saved.len() > 1 {
                let name = tr(
                    "Remove {route} route",
                    &[("route", Value::Text(label.clone()))],
                );
                let message = tr("Remove {route} route?", &[("route", Value::Text(label))]);
                let message =
                    stored(address.map_or(message.clone(), |addr| format!("{message}\n{addr}")));
                let target = stored(route.id.clone());
                line = line.child(button(
                    ButtonContent::Icon {
                        icon: crate::ui::icon::Icon::X,
                        label: name,
                    },
                    ButtonVariant::GhostMuted,
                    ButtonSize::IconXs,
                    move || commands.busy.get(),
                    move || {
                        let observed = order.get();
                        let target = target.get();
                        let Some(answer) = request_confirm_dialog(
                            message.get(),
                            Some(ConfirmDialogOptions {
                                variant: ConfirmDialogVariant::Destructive,
                                ..Default::default()
                            }),
                        ) else {
                            return;
                        };
                        commands.tasks.spawn(async move {
                            if answer.await {
                                commands.run("Remove route", move || {
                                    s.get().edit_routes(
                                        id.get(),
                                        observed,
                                        SavedRouteEdit::Remove(target),
                                    )
                                });
                            }
                        });
                    },
                ));
            }
        }
        list = list.child(line);
    }
    list.child(action(t("Add route"), move || {
        form.set(Some(Some((id.get(), host.get()))))
    }))
    .into_element()
}

fn diagnosis_details(
    d: ConnectionDiagnosis,
    host: Stored<String>,
    id: Stored<EnvironmentId>,
    services: Option<Services>,
    commands: Commands,
    open: Signal<bool>,
) -> AnyElement {
    let headline = d.headline.clone();
    let d = stored(d);
    view()
        .child(text(headline).app_class(tw("text-xs text-destructive")))
        .child(action(t("Details"), move || open.update(|v| *v = !*v)))
        .child(region(
            move || open.get(),
            move |shown| {
                shown.then(|| {
                    let d = d.get();
                    let mut panel = view().app_class(tw(
                        "mt-2 flex flex-col gap-3 border-t border-border/50 pt-2 text-xs",
                    ));
                    for hop in &d.hops {
                        let note = match hop.result {
                            HopResult::Ok => "✓".into(),
                            HopResult::Failed => t("Failed"),
                            HopResult::Unchecked => t("Not checked"),
                        };
                        panel = panel.child(text(format!("{} · {note}", hop.label)));
                    }
                    panel = panel.child(text(t("What to try")));
                    for step in &d.steps {
                        panel = panel.child(text(step.clone()));
                    }
                    for (label, value) in &d.facts {
                        panel = panel.child(text(format!("{label} · {value}")));
                    }
                    if let Some(url) = d.more_info_url.clone() {
                        let opener = crate::runtime::services::links::link_opener_here();
                        panel =
                            panel.child(action(t("More info"), move || opener.open(url.clone())));
                    }
                    let report = t3_client::connection::connection_diagnosis_text(&host.get(), &d);
                    panel
                        .child(services.map(|s| {
                            action(t("Retry now"), move || {
                                commands.run("Connection failed", move || s.get().retry(&id.get()))
                            })
                        }))
                        .child(action(t("Copy diagnostics"), move || {
                            set_clipboard(report.clone())
                        }))
                })
            },
        ))
        .into_element()
}

/// Port of AddEnvironmentDialog.tsx's remote fields. A pasted pairing URL is
/// split before the host is drawn. Only the masked input holds the code.
fn add_environment(
    form: PairForm,
    target: Option<(EnvironmentId, String)>,
    services: Option<Services>,
    relay_tick: Signal<u64>,
) -> AnyElement {
    use crate::ui::{
        dialog::{
            PopupOptions, dialog, dialog_description, dialog_header, dialog_panel, dialog_popup,
            dialog_title,
        },
        input::masked_text_field,
    };
    use t3_client::connection::pairing_link::{
        parse_pairing_url_fields, parse_remote_pairing_fields,
    };
    let host = signal(String::new());
    let code = signal(String::new());
    let busy = signal(false);
    let error = signal(None::<String>);
    let owner = TaskOwner::here();
    let rt = rt();
    let backend = stored(rt.backend());
    let title = target
        .as_ref()
        .map(|(_, name)| {
            tr(
                "Add a route to {name}",
                &[("name", Value::Text(name.clone()))],
            )
        })
        .unwrap_or_else(|| t("Add Environment"));
    let title = stored(title);
    let is_route = target.is_some();
    let target = stored(target);
    let submit = move || {
        if busy.get_untracked() {
            return;
        }
        let fields =
            match parse_remote_pairing_fields(&host.get_untracked(), &code.get_untracked(), None) {
                Ok(fields) => fields,
                Err(failure) => {
                    error.set(Some(Text::client_key(failure.detail).resolve().to_string()));
                    return;
                }
            };
        let mut input = fields.pairing_input();
        input.expected_environment = target.get().map(|(id, _)| id);
        busy.set(true);
        error.set(None);
        let work = backend.get().pair(input);
        owner.spawn(async move {
            match work.await {
                Ok(_) => form.set(None),
                Err(failure) => {
                    busy.set(false);
                    // Only locally defined configuration failures have stable copy. Other
                    // failure details can include credentials echoed by the remote host.
                    let message = match failure.kind {
                        FailureKind::Authentication => t("Pairing code was rejected."),
                        _ => t("Could not add backend"),
                    };
                    error.set(Some(message));
                }
            }
        });
    };
    dialog(signal(true), move |state|{
        state.on_open_change(move |next|{if !next {form.set(None);}});
        dialog_popup(state,PopupOptions::default(),move ||{
            view().child(dialog_header(state,view().child(dialog_title(state, move ||title.get().into())).child(dialog_description(state,move ||if is_route {
                "Pair this machine again over another address, such as its Tailscale name. It joins the existing routes instead of adding a second machine.".into()
            } else {"Pair another environment to this client.".into()}))))
                .child(dialog_panel(state,view().app_class(tw("space-y-4"))
                    .child(region(move || {relay_tick.get();let (id,_) = target.get()?;let s=services?;
                        let account=s.get().relay_state()?;
                        let record=account.environments.get(&id)?;
                        let exists=rt.environments().iter().any(|e|e.id==id&&e.routes.iter().any(|r|r.mode==ConnectionMode::Relay));
                        (!exists).then(||(id,record.environment.label.0.clone()))
                    },move |offer|offer.map(|(id,label)|{
                        view().child(text(t("This machine is on your T3 Connect account. Use it as a fallback route."))).child(button(ButtonContent::Label(t("Add T3 Connect")),ButtonVariant::Outline,ButtonSize::Xs,move ||busy.get(),move ||{
                            let Some(s)=services else{return};
                            if busy.get_untracked(){return;} busy.set(true);
                            let work=s.get().add_relay(&id,&label);
                            owner.spawn(async move{ if work.await.is_ok(){form.set(None);}else{busy.set(false);error.set(Some(t("Could not add the route.")));} });
                        }))
                    })))
                    .child(text(t("Host")))
                    .child(nsh_base::input().label(t("Host")).placeholder(t("backend.example.com")).disabled(move ||busy.get())
                        .value(move ||host.get()).on_change(move |raw:&str|{ if let Some(fields)=parse_pairing_url_fields(raw,None) {
                            host.set(fields.host);code.set(fields.pairing_code.into_inner());
                        } else {host.set(raw.to_owned());} }).on_submit(move |_:&str|submit()))
                    .child(text(t("Pairing code")))
                    .child(masked_text_field(code,"PAIRCODE".into(),t("Pairing code"),move ||busy.get(),submit))
                    .child(text(t("Paste a full pairing URL here to fill both fields automatically.")))
                    .child(region(move ||error.get(),|error|error.map(text)))
                    .child(button(ButtonContent::Label(if is_route {t("Add route")} else {t("Add environment")}),ButtonVariant::Outline,ButtonSize::Sm,move ||busy.get(),submit))))
        })
    }).into_element()
}

/// Port of LinkedEnvironmentsSettings.tsx. Each mount belongs to a specific
/// host session. A reconnect or loss of read access disposes its query and rows.
fn linked_environments(services: Services) -> AnyElement {
    let rt = rt();
    let hosts = computed(move || {
        rt.store.track_environments();
        rt.environments()
            .into_iter()
            .filter_map(|e| {
                let session = services.get().peer_session(&e.id)?;
                let writer = session.grants_scope(AuthEnvironmentScope::AccessWrite) == Some(true);
                Some((e.id, e.label, e.state.generation, writer))
            })
            .collect::<Vec<_>>()
    });
    each(
        move || hosts.get(),
        |e| (e.0.clone(), e.2.0, e.3),
        move |row, index| {
            let (id, label, _, _) = row.get_untracked();
            let id = stored(id);
            let title = if hosts.with(|h| h.len()) > 1 {
                tr(
                    "Linked environments · {name}",
                    &[("name", Value::Text(label))],
                )
            } else {
                t("Linked environments")
            };
            let refresh = signal(0u32);
            let content = region(
                move || refresh.get(),
                move |_| peer_list(id, services, refresh),
            );
            settings_section(
                if index.get_untracked() == 0 {
                    "linked-environments"
                } else {
                    ""
                },
                title.into(),
                content,
            )
        },
    )
}

fn peer_list(id: Stored<EnvironmentId>, services: Services, refresh: Signal<u32>) -> AnyElement {
    use t3_client::rpc::RpcEndpoint;
    use t3_contracts::rpc::{Empty, WsPeerLinksListRpc};
    let result = signal(None);
    let writer = signal(false);
    let busy = signal(false);
    let message = signal(None::<String>);
    let tasks = TaskOwner::here();
    if let Some(session) = services.get().peer_session(&id.get()) {
        writer.set(session.grants_scope(AuthEnvironmentScope::AccessWrite) == Some(true));
        tasks.spawn(async move {
            result.set(Some(
                session
                    .call::<WsPeerLinksListRpc>(Empty {})
                    .await
                    .map(|r| r.links)
                    .map_err(|_| ()),
            ));
        });
    }
    let opened = signal(false);
    view().child(writer.get_untracked().then(||action(t("Link"),move ||opened.set(true))))
        .child(region(move ||opened.get(),move |visible|visible.then(||peer_form(id,services,opened,refresh,result))))
        .child(region(move ||result.get(),move |state|{
        let links=match state {
            None=>return text(t("Loading linked environments…")).into_element(),
            Some(Err(()))=>return text(t("Could not load linked environments")).into_element(),
            Some(Ok(links)) if links.is_empty()=>return view().child(text(t("No linked environments"))).child(text(t("Link another machine running T3 Code, such as a VPS, so agents here can launch and drive threads there."))).into_element(),
            Some(Ok(links))=>links,
        };
        let mut list=view();
        for link in links {
            use t3_client::services::views::{peer_link_health,PeerLinkHealth};
            let status=match peer_link_health(&link,nsh_core::time::unix_ms() as i64) {
                PeerLinkHealth::Expired=>t("Expired. Link it again with a new pairing code."),
                PeerLinkHealth::Unreachable{..}|PeerLinkHealth::Failing{..}=>t("Not answering right now."),
                PeerLinkHealth::Reachable{expires_in_days:None}=>t("Reachable"),
                PeerLinkHealth::Reachable{expires_in_days:Some(days)}=>tr("Expires in {count, plural, one {# day} other {# days}}. Link it again to renew.", &[("count",Value::Int(days))]),
            };
            let access=Text::client_key(t3_client::thread_views::link_request::access_label(&link.access)).resolve();
            let urls=link.urls.iter().filter_map(|u|t3_client::connection::agent_consent::page_origin(&u.0)).collect::<Vec<_>>().join(", ");
            let target=stored(link.environment_id);
            let confirmation=stored(tr("Forget {name}? Agents here stop working there. Revoke \"T3 Code · …\" in {name}'s Sharing & access to end the session there too.", &[("name",Value::Text(link.label.clone()))]));
            list=list.child(view().app_class(tw("px-3 py-2.5 sm:px-4"))
                .child(text(link.label).app_class(tw("text-sm font-medium text-foreground")))
                .child(text(format!("{access} · {status}")).app_class(tw("text-xs text-muted-foreground")))
                .child(text(urls).app_class(tw("truncate text-xs text-muted-foreground/80")))
                .child(writer.get_untracked().then(||button(ButtonContent::Label(t("Forget")),ButtonVariant::DestructiveOutline,ButtonSize::Xs,move ||busy.get(),move ||{
                    if busy.get_untracked(){return;}
                    let Some(answer)=request_confirm_dialog(confirmation.get(),Some(ConfirmDialogOptions{variant:ConfirmDialogVariant::Destructive,..Default::default()})) else{return};
                    busy.set(true);
                    tasks.spawn(async move{
                        if answer.await && let Some(session)=services.get().peer_session(&id.get()) {
                            use t3_contracts::{peer_link::PeerLinkRemoveInput,rpc::WsPeerLinksUnlinkRpc};
                            let input=PeerLinkRemoveInput{environment_id:target.get()};
                            if session.available::<WsPeerLinksUnlinkRpc>(&input) {
                                match session.call::<WsPeerLinksUnlinkRpc>(input).await {
                                    Ok(_)=>refresh.update(|n|*n+=1),
                                    Err(_)=>message.set(Some(t("Could not forget the link"))),
                                }
                            }
                        }
                        busy.set(false);
                    });
                }))));
        }
        list.into_element()
    })).child(region(move ||message.get(),|m|m.map(text))).into_element()
}

fn relay_rows(services: Services, tick: Signal<u64>) -> AnyElement {
    use t3_client::relay::RelayEnvironmentAvailability as Availability;
    let rt = rt();
    let commands = Commands {
        tasks: TaskOwner::here(),
        busy: signal(false),
        error: signal(None),
    };
    region(
        move || {
            tick.get();
            let saved = rt.environments();
            services
                .get()
                .relay_state()
                .into_iter()
                .flat_map(|s| s.environments)
                .filter(|(id, _)| !saved.iter().any(|e| e.id == *id))
                .map(|(id, entry)| {
                    let unsupported = entry
                        .status
                        .as_ref()
                        .and_then(|s| s.descriptor.as_ref()?.as_ref())
                        .and_then(t3_client::connection::orchestration_protocol_compatibility_error)
                        .is_some();
                    (
                        id,
                        entry.environment.label.0,
                        entry.availability,
                        unsupported,
                    )
                })
                .collect::<Vec<_>>()
        },
        move |rows| {
            let mut list = view();
            for (id, label, availability, unsupported) in rows {
                let status = if unsupported {
                    "Client not supported"
                } else {
                    match availability {
                        Availability::Online => "Relay online",
                        Availability::Offline => "Relay offline",
                        Availability::Checking => "Checking relay status…",
                        Availability::Error => "Relay status unavailable",
                    }
                };
                let name = stored(label.clone());
                let id = stored(id);
                list = list.child(
                    view()
                        .app_class(tw("flex flex-row items-center gap-2 px-3 py-2.5 sm:px-4"))
                        .child(
                            view().flex_1().child(text(label)).child(
                                text(format!(
                                    "{} · {} · {}",
                                    t("T3 Connect"),
                                    t("Not added"),
                                    Text::client_key(status).resolve()
                                ))
                                .app_class(tw("text-xs text-muted-foreground")),
                            ),
                        )
                        .child(button(
                            ButtonContent::Label(t("Add")),
                            ButtonVariant::Default,
                            ButtonSize::Sm,
                            move || unsupported || commands.busy.get(),
                            move || {
                                commands.run("Could not connect to environment", move || {
                                    services.get().add_relay(&id.get(), &name.get())
                                })
                            },
                        )),
                );
            }
            list.child(region(
                move || commands.error.get(),
                |e| e.map(|e| text(Text::client_key(e).resolve())),
            ))
        },
    )
}

fn environment_menu(
    services: Services,
    id: Stored<EnvironmentId>,
    label: Stored<String>,
    commands: Commands,
    relay_managed: bool,
    routes_open: Signal<bool>,
    mcp: Option<String>,
) -> AnyElement {
    use crate::ui::dialog::{
        alert_dialog, alert_dialog_close, alert_dialog_description, alert_dialog_footer,
        alert_dialog_header, alert_dialog_popup, alert_dialog_title,
    };
    use crate::ui::menu::{ItemOptions, MenuOptions, Variant, menu_button, menu_item, menu_popup};
    use t3_client::relay::RelayEnvironmentRegistration;
    let relay_confirm = signal(false);
    let name = label.get();
    let options = ItemOptions {
        variant: Variant::Destructive,
        ..Default::default()
    };
    let remove = t("Remove from this device…");
    let routes = if routes_open.get_untracked() {
        t("Hide routes")
    } else {
        t("Routes")
    };
    let menu=menu_button(tr("More actions for {name}",&[("name",Value::Text(name))]),move ||commands.busy.get(),crate::ui::icon::icon_with_class(crate::ui::icon::Icon::Ellipsis,"size-4"),MenuOptions::default(),menu_popup(view()
        .child(menu_item(routes.clone(),ItemOptions::default(),text(routes),move || routes_open.update(|open| *open = !*open)))
        .child(mcp.map(|url|{
            let label=t("Copy MCP URL");
            menu_item(label.clone(),ItemOptions::default(),text(label),move ||set_clipboard(url.clone()))
        }))
        .child(menu_item(remove.clone(),options,text(remove),move ||{
            if relay_managed&&services.get().relay_state().is_some(){relay_confirm.set(true);return}
            let message=tr("Remove {name} from this device?\nThis forgets its pairing, credentials, and cached threads here. Switch it off instead to keep it saved.",&[("name",Value::Text(label.get()))]);
            let Some(answer)=request_confirm_dialog(message,Some(ConfirmDialogOptions{variant:ConfirmDialogVariant::Destructive,..Default::default()})) else{return};
            let id=id.get();
            commands.tasks.spawn(async move {if answer.await{commands.run("Could not remove backend",move ||services.get().remove_saved(&id));}});
        }))));
    view().child(menu).child(alert_dialog(relay_confirm,move |state|alert_dialog_popup(state,Default::default(),move ||{
        let registration=services.get().relay_state().map(|s|s.registration(&id.get()));
        let description=match registration {
            Some(RelayEnvironmentRegistration::Registered)=>"It stays on your T3 Connect account and keeps its host space. Deregister it in T3 Connect settings to free it.",
            Some(RelayEnvironmentRegistration::Unregistered)=>"It's no longer on your T3 Connect account.",
            _=>"This doesn't change your T3 Connect account. To deregister it there, use T3 Connect settings.",
        };
        view().child(alert_dialog_header(state,view().child(alert_dialog_title(state,move ||Text::content(tr("Remove {name} from this device?",&[("name",Value::Text(label.get()))]))))
            .child(alert_dialog_description(state,||"This forgets its pairing, credentials, and cached threads here.".into()))
            .child(text(Text::client_key(description).resolve()))))
            .child(alert_dialog_footer(state,Default::default(),view().flex_row().child(alert_dialog_close(state,ButtonContent::Label(t("Cancel")),ButtonVariant::Outline,ButtonSize::Default,||false))
            .child(button(ButtonContent::Label(t("Remove from this device")),ButtonVariant::Destructive,ButtonSize::Default,move ||commands.busy.get(),move ||{
                state.close();commands.run("Could not remove backend",move ||services.get().remove_saved(&id.get()));
            }))))
    }))).into_element()
}

fn peer_form(
    source: Stored<EnvironmentId>,
    services: Services,
    opened: Signal<bool>,
    refresh: Signal<u32>,
    links: Signal<Option<Result<Vec<PeerLinkSummary>, ()>>>,
) -> AnyElement {
    use crate::ui::{
        dialog::{
            dialog, dialog_description, dialog_header, dialog_panel, dialog_popup, dialog_title,
        },
        input::masked_text_field,
        select::{SelectItem, SelectProps, Value as SelectValue, select},
    };
    use t3_client::thread_views::link_request::{ANOTHER_ADDRESS, access_label};
    let rt = rt();
    let choice = signal(None::<String>);
    let address = signal(String::new());
    let code = signal(String::new());
    let access = signal(AuthMcpClientAccess::ApprovalRequired);
    let busy = signal(false);
    let error = signal(None::<String>);
    let failed = signal(std::collections::BTreeSet::new());
    let tasks = TaskOwner::here();
    let machines = computed(move || {
        rt.environments();
        rt.store.track_environments();
        services.get().machines(
            &source.get(),
            &links.get().and_then(Result::ok).unwrap_or_default(),
        )
    });
    let selected = computed(move || {
        choice.get().or_else(|| {
            machines
                .with(|m| m.is_empty())
                .then(|| ANOTHER_ADDRESS.to_owned())
        })
    });
    let machine = computed(move || {
        machines.with(|ms| {
            ms.iter()
                .find(|m| Some(&m.environment_id.0) == selected.get().as_ref())
                .cloned()
        })
    });
    let manual = computed(move || {
        selected.get().is_some() && machine.get().is_none_or(|m| m.urls.is_empty())
    });
    let mint = computed(move || {
        machine.get().is_some_and(|m| {
            m.connected
                && !manual.get()
                && !failed.with(|f| f.contains(&m.environment_id))
                && services
                    .get()
                    .grants(&m.environment_id, AuthEnvironmentScope::AccessWrite)
                    == Some(true)
        })
    });
    let ready = computed(move || {
        selected.get().is_some()
            && (!manual.get() || !address.get().trim().is_empty())
            && (mint.get() || !code.get().trim().is_empty())
    });
    let submit = move || {
        if busy.get_untracked() || !ready.get_untracked() {
            return;
        }
        let dest = if manual.get_untracked() {
            PeerDestination::Address(address.get_untracked().trim().to_owned())
        } else if let Some(m) = machine.get_untracked() {
            PeerDestination::Machine(m.environment_id)
        } else {
            return;
        };
        let target = machine.get_untracked().map(|m| m.environment_id);
        let supplied =
            (!mint.get_untracked()).then(|| Sensitive(code.get_untracked().trim().to_owned()));
        busy.set(true);
        error.set(None);
        let work = services
            .get()
            .link(source.get(), dest, supplied, access.get_untracked());
        tasks.spawn(async move {
            let outcome = work.await;
            busy.set(false);
            match outcome {
                Ok(()) => {
                    opened.set(false);
                    refresh.update(|v| *v += 1);
                }
                Err(failure) => {
                    let message = match failure {
                        PeerLinkFailure::Mint(reason) => {
                            if let Some(target) = target {
                                failed.update(|set| {
                                    set.insert(target);
                                });
                            }
                            reason.to_string()
                        }
                        PeerLinkFailure::Unavailable => t("Could not link the environment"),
                    };
                    error.set(Some(message));
                }
            }
        });
    };
    dialog(signal(true),move |state|{
        state.on_open_change(move |next|{if !next&&!busy.get_untracked(){opened.set(false);}});
        dialog_popup(state,Default::default(),move ||{
            let modes=[AuthMcpClientAccess::ReadOnly,AuthMcpClientAccess::ApprovalRequired,AuthMcpClientAccess::AutoAcceptEdits,AuthMcpClientAccess::Auto,AuthMcpClientAccess::FullAccess];
            let mut access_picker=SelectProps::new("Access");
            access_picker.items=modes.iter().map(|a|SelectItem::new(access_label(a),Text::client_key(access_label(a)).resolve())).collect::<Vec<_>>().into();
            access_picker.value=Prop::bind(move ||SelectValue::Controlled(Some(access_label(&access.get()).into())));
            access_picker.disabled=Prop::bind(move ||busy.get());
            let panel=view().child(region(move ||!machines.with(|ms|ms.is_empty()),move |show|show.then(||{            let mut picker=SelectProps::new("Machine");
            picker.items=Prop::bind(move ||{let mut items=machines.get().into_iter().map(|m|SelectItem::new(m.environment_id.0,m.label)).collect::<Vec<_>>();items.push(SelectItem::new(ANOTHER_ADDRESS,t("Another address")));items});
            picker.value=Prop::bind(move ||SelectValue::Controlled(selected.get().map(Into::into)));
            picker.disabled=Prop::bind(move ||busy.get());
select(picker,move |value|choice.set(value.map(|v|v.to_string())))})))
                .child(region(move ||machine.get().is_some_and(|m|m.linked),|yes|yes.then(||text(t("Already linked. Linking again renews it with the access you pick.")))))
                .child(region(move ||manual.get(),move |show|show.then(||view().child(text(t("Address"))).child(nsh_base::input().label(t("Address")).placeholder(t("https://box.example.ts.net")).value(move ||address.get()).disabled(move ||busy.get()).on_change(move |raw:&str|{address.set(raw.to_owned());if choice.get_untracked().is_none(){choice.set(Some(ANOTHER_ADDRESS.into()));}})))))
                .child(region(move ||manual.get()&&!address.get().trim().is_empty(), move |show|show.then(||view()
                    .child(text(t("Check this address")))
                    .child(text(t("This app cannot confirm it is one of your machines. The pairing code you paste is sent there.")))
                    .child(text(move ||SharedString::from(t3_client::connection::agent_consent::page_origin(&address.get()).unwrap_or_default()))))))
                .child(region(move ||selected.get().is_some()&&!mint.get(),move |show|show.then(||view().child(text(t("Pairing code"))).child(masked_text_field(code,t("Paste the code"),t("Pairing code"),move ||busy.get(),submit)))))
                .child(text(t("Access"))).child(select(access_picker,move |next|{if let Some(mode)=modes.iter().find(|m|next.as_deref()==Some(access_label(m))){access.set(*mode);}}))
                .child(text(move ||Text::client_key(match access.get() {
                    AuthMcpClientAccess::ReadOnly=>"Read projects and threads. Cannot start, message or change anything.",
                    AuthMcpClientAccess::ApprovalRequired=>"Ask before commands and file changes.",
                    AuthMcpClientAccess::AutoAcceptEdits=>"Auto-approve edits, ask before other actions.",
                    AuthMcpClientAccess::Auto=>"Supported providers approve routine actions; others still ask.",
                    AuthMcpClientAccess::FullAccess=>"Allow commands and edits without prompts.",
                }).resolve()))
                .child(text(t("An agent never gets more access there than its own mode here. Below full access, it can change only threads this link started, not that machine's own threads, projects or settings.")))
                .child(region(move ||error.get(),|m|m.map(text)))
                .child(button(ButtonContent::Label(t("Link")),ButtonVariant::Default,ButtonSize::Sm,move ||busy.get()||!ready.get(),submit));
            view().child(dialog_header(state,view().child(dialog_title(state,||"Link an environment".into())).child(dialog_description(state,||"Agents here can then launch, message and wait on threads there. It appears under Authorized clients in that environment's Sharing & access, where it can be revoked.".into())))).child(dialog_panel(state,panel))
        })
    }).into_element()
}
