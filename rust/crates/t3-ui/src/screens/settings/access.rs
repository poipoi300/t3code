//! Port of ConnectionsSettings.tsx AccessSettings and SessionPermissions.tsx.
use super::{
    connections::{Services, action, saved},
    shell::{region, settings_page_container, settings_section},
};
use crate::{
    i18n::t,
    runtime::rt,
    state::url::UrlState,
    theme::tw,
    ui::select::{SelectItem, SelectProps, Size, Value, select},
};
use nsh_core::prelude::*;
use t3_client::{
    composition::EnvironmentEntry,
    connection::{ConnectionMode, Phase},
    services::settings::scope::validate_settings_route_search,
    types::EnvironmentId,
};
use t3_contracts::auth::AuthEnvironmentScope as Scope;

const OPTIONS: &[(Scope, &str)] = &[
    (Scope::OrchestrationRead, "View environment"),
    (Scope::OrchestrationOperate, "Operate tasks"),
    (Scope::SettingsWrite, "Change environment settings"),
    (Scope::ProvidersManage, "Manage providers"),
    (Scope::EnvironmentMaintain, "Maintain environment"),
    (Scope::PreviewOperate, "Control previews"),
    (Scope::DiagnosticsRead, "View diagnostics and usage"),
    (Scope::TerminalRead, "View terminals"),
    (Scope::TerminalOperate, "Use terminals"),
    (Scope::SourceControlWrite, "Change source control"),
    (Scope::FilesystemRead, "Read files"),
    (Scope::FilesystemWrite, "Write files"),
    (Scope::AccessRead, "View access"),
    (Scope::AccessWrite, "Manage access"),
    (Scope::RelayRead, "View relay"),
    (Scope::RelayWrite, "Manage relay"),
];

pub(super) fn session_permissions(
    id: EnvironmentId,
    connected: bool,
    route: bool,
    services: Option<Services>,
) -> AnyElement {
    use t3_client::services::settings::access::PermissionState;
    let rt = rt();
    let id = stored(id);
    region(
        move || {
            rt.environments()
                .into_iter()
                .find(|e| connected && e.id == id.get() && e.state.phase == Phase::Connected)
                .map(|e| e.state.generation)
        },
        move |generation| {
            let Some(manager) = generation.and_then(|_| services?.get().access_manager(&id.get()))
            else {
                return text(t(
                    "Connect to this environment to view this session’s permissions.",
                ))
                .app_class(tw("py-3 text-xs text-muted-foreground"))
                .into_element();
            };
            let status = signal(PermissionState::Checking);
            watch(async move {
                let scopes = OPTIONS.iter().map(|(scope, _)| *scope).collect::<Vec<_>>();
                manager
                    .permissions(&scopes, move |value| status.set(value))
                    .await;
            });
            region(
                move || status.get(),
                move |state| {
                    let grants=match state{
                PermissionState::Verified(scopes)=>scopes,
                other=>return text(t(match other{
                    PermissionState::Failed=>"Could not verify your permissions. Retrying automatically.",
                    PermissionState::Rejected=>"This environment did not accept your session. Reconnect to try again.",
                    _=>"Checking permissions…",
                })).app_class(tw("py-3 text-xs text-muted-foreground")).into_element(),
            };
                    let mut list = view()
                        .role(AccessibilityRole::List)
                        .app_class(tw("grid gap-x-6 gap-y-2 sm:grid-cols-2"));
                    for ((_, title), granted) in OPTIONS.iter().zip(grants) {
                        list = list.child(
                            view()
                                .flex_row()
                                .justify_between()
                                .gap(px(12.0))
                                .child(text(crate::i18n::Text::client_key(*title).resolve()))
                                .child(
                                    text(if granted {
                                        t("Allowed")
                                    } else {
                                        t("Not granted")
                                    })
                                    .app_class(tw("shrink-0 text-muted-foreground")),
                                ),
                        );
                    }
                    view().app_class(tw("space-y-3 py-3 text-xs"))
            .child(text(if route {t("These permissions apply to your active route. Other routes may have different permissions.")} else {t("These permissions apply to this client’s current connection.")}).app_class(tw("text-muted-foreground"))).child(list).into_element()
                },
            )
        },
    )
}

fn primary(e: &EnvironmentEntry) -> bool {
    e.routes
        .first()
        .is_some_and(|r| r.mode == ConnectionMode::Local)
}

pub fn screen() -> AnyElement {
    let rt = rt();
    let urls = use_context::<UrlState>();
    let requested = computed(move || {
        urls.and_then(|url| validate_settings_route_search(&url.location.get().search).machine)
            .map(EnvironmentId)
    });
    region(
        move || requested.get(),
        move |initial| {
            let selected = signal(initial);
            let candidates = computed(move || {
                rt.environments()
                    .into_iter()
                    .filter(|e| {
                        primary(e)
                            || selected.with(|id| id.as_ref() == Some(&e.id))
                            || (e.state.desired && saved(e))
                    })
                    .collect::<Vec<_>>()
            });
            let current = computed(move || {
                candidates.with(|envs| {
                    envs.iter()
                        .find(|e| selected.with(|id| id.as_ref() == Some(&e.id)))
                        .or_else(|| envs.iter().find(|e| primary(e)))
                        .or_else(|| envs.iter().find(|e| e.state.phase == Phase::Connected))
                        .or_else(|| envs.first())
                        .map(|e| e.id.clone())
                })
            });
            let services = rt.backend().connection_settings().map(stored);
            let picker = when(
                move || candidates.with(Vec::len) > 1,
                move || {
                    let mut picker = SelectProps::new("Environment");
                    picker.size = Size::Sm;
                    picker.items = Prop::bind(move || {
                        candidates.with(|envs| {
                            envs.iter()
                                .map(|e| SelectItem::new(e.id.0.clone(), e.label.clone()))
                                .collect()
                        })
                    });
                    picker.value =
                        Prop::bind(move || Value::Controlled(current.get().map(|id| id.0.into())));
                    view()
                        .flex_row()
                        .child(text(t("Showing access for")))
                        .child(select(picker, move |next| {
                            if let Some(id) = next {
                                selected.set(Some(EnvironmentId(id.to_string())));
                            }
                        }))
                },
            );
            settings_page_container(view().child(picker).child(region(
                move || current.get(),
                move |id| {
                    let Some(id) = id else {
                        return settings_section(
                            "connections-environment",
                            "This machine".into(),
                            view()
                                .child(text(t("No environment selected")))
                                .child(text(t(
                                    "Connect an environment to view its settings and access.",
                                ))),
                        )
                        .into_element();
                    };
                    let id = stored(id);
                    let opened = signal(false);
                    let entry = computed(move || {
                        candidates.with(|es| es.iter().find(|e| e.id == id.get()).cloned())
                    });
                    view()
                        .child(region(
                            move || {
                                entry
                                    .get()
                                    .map(|e| (e.label, e.state.phase == Phase::Connected))
                            },
                            move |env| {
                                env.map(|(label, online)| {
                                    settings_section(
                                        "connections-environment",
                                        label.into(),
                                        view()
                                            .app_class(tw("px-3 sm:px-4"))
                                            .child(action(t("Permissions"), move || {
                                                opened.update(|v| *v = !*v)
                                            }))
                                            .child(region(
                                                move || opened.get(),
                                                move |show| {
                                                    show.then(|| {
                                                        session_permissions(
                                                            id.get(),
                                                            online,
                                                            false,
                                                            services,
                                                        )
                                                    })
                                                },
                                            )),
                                    )
                                })
                            },
                        ))
                        .child(access_core(id, services))
                        .into_element()
                },
            )))
            .into_element()
        },
    )
}

use crate::{
    i18n::{Text, Value as MessageValue, tr},
    runtime::tasks::{TaskOwner, watch},
    ui::button::{ButtonContent, ButtonSize, ButtonVariant, button},
};
use std::rc::Rc;
use t3_client::services::settings::access::{
    AccessFailure, Action, ClientRow, Manager, Outcome, Records, RevokeTitle,
};

fn access_core(id: Stored<EnvironmentId>, services: Option<Services>) -> AnyElement {
    let rt = rt();
    region(
        move || {
            rt.store.track_environments();
            let e = rt
                .environments()
                .into_iter()
                .find(|e| e.id == id.get() && e.state.phase == Phase::Connected)?;
            let s = services?.get();
            let read = s.grants(&e.id, Scope::AccessRead) == Some(true);
            let write = s.grants(&e.id, Scope::AccessWrite) == Some(true);
            (read || write).then_some((e.state.generation, read, write))
        },
        move |state| {
            state.and_then(|(_, read, write)| {
                Some(management(
                    services?.get().access_manager(&id.get())?,
                    read,
                    write,
                ))
            })
        },
    )
}

#[derive(Clone, Copy)]
struct Controls {
    manager: Stored<Rc<Manager>>,
    tasks: TaskOwner,
    busy: Signal<Vec<Action>>,
    pending: Signal<Option<(Action, String)>>,
    error: Signal<Option<String>>,
}
impl Controls {
    fn working(self, action: &Action) -> bool {
        self.busy.with(|active| {
            active.iter().any(|other| match (action, other) {
                (Action::Approve(a) | Action::Deny(a), Action::Approve(b) | Action::Deny(b)) => {
                    a == b
                }
                _ => action == other,
            })
        })
    }
    fn run(self, action: Action, choice: Option<bool>) {
        if self.working(&action) || (self.pending.with(Option::is_some) && choice.is_none()) {
            return;
        }
        self.busy.update(|active| active.push(action.clone()));
        self.error.set(None);
        let manager = self.manager.get();
        self.tasks.spawn(async move {
            let result = manager.apply(&action, choice).await;
            self.busy
                .update(|active| active.retain(|item| item != &action));
            match result {
                Ok(Outcome::Done) => {
                    if choice.is_some() {
                        self.pending.set(None);
                    }
                }
                Ok(Outcome::Confirm(title)) => {
                    if self.pending.with(Option::is_none) {
                        let title = match title {
                            RevokeTitle::OtherClients => t("Revoke all other clients?"),
                            RevokeTitle::Device(name) => {
                                tr("Revoke {name}?", &[("name", MessageValue::Text(name))])
                            }
                        };
                        self.pending.set(Some((action, title)));
                    }
                }
                Err(error) => {
                    if error != AccessFailure::Save && choice.is_some() {
                        self.pending.set(None);
                    }
                    if error == AccessFailure::Save && choice == Some(true) {
                        self.error
                            .set(Some(t("Could not require approval for new devices.")));
                        return;
                    }
                    self.error.set(Some(t(match action {
                        Action::Approve(_) => "Could not approve the device",
                        Action::Deny(_) => "Could not deny the device",
                        Action::Device(_) => "Could not revoke the device",
                        Action::Others => "Could not revoke other clients",
                        Action::Client(_) => "Could not revoke client access",
                    })))
                }
            }
        });
    }
    fn button(self, action: Action, label: &'static str, allowed: bool) -> AnyElement {
        let variant = if matches!(action, Action::Approve(_)) {
            ButtonVariant::Default
        } else {
            ButtonVariant::DestructiveOutline
        };
        let action = stored(action);
        region(
            move || self.working(&action.get()),
            move |busy| {
                let label = match (busy, action.get()) {
                    (true, Action::Approve(_)) => "Saving…",
                    (true, Action::Client(_) | Action::Device(_) | Action::Others) => "Revoking…",
                    _ => label,
                };
                button(
                    ButtonContent::Label(t(label)),
                    variant,
                    ButtonSize::Xs,
                    move || !allowed || busy,
                    move || self.run(action.get(), None),
                )
            },
        )
    }
}

fn management(manager: Rc<Manager>, read: bool, write: bool) -> AnyElement {
    let records = signal(None::<Result<Records, ()>>);
    let controls = Controls {
        manager: stored(manager.clone()),
        tasks: TaskOwner::here(),
        busy: signal(Vec::new()),
        pending: signal(None),
        error: signal(None),
    };
    if read {
        watch(async move {
            manager.follow(move |data| records.set(Some(data))).await;
        });
    }
    let expanded = signal(false);
    let list = region(
        move || (expanded.get(), records.get()),
        move |(open, state)| {
            if !open {
                return view().into_element();
            }
            if !read {
                return text(t(
                    "This connection can create access links but cannot view authorized clients.",
                ))
                .into_element();
            }
            let data = match state {
                None => return text(t("Loading…")).into_element(),
                Some(Err(())) => {
                    return text(t("Could not load authorized clients")).into_element();
                }
                Some(Ok(data)) => data,
            };
            let mut rows = view();
            if data.clients.is_empty() {
                rows = rows.child(text(t("No client sessions.")));
            }
            for (index, client) in data.clients.into_iter().enumerate() {
                rows = rows.child(client_row(index, client, controls, write));
            }
            rows.into_element()
        },
    );
    let header = view()
        .app_class(tw(
            "flex flex-row flex-wrap items-center justify-between gap-2",
        ))
        .child(action(t("Authorized clients"), move || {
            expanded.update(|v| *v = !*v)
        }))
        .child(region(
            move || {
                records.with(|state| {
                    state
                        .as_ref()
                        .and_then(|rows| rows.as_ref().ok())
                        .map(|rows| rows.clients.len())
                })
            },
            |count| {
                count.map(|count| {
                    text(tr(
                        "{count, plural, one {# client} other {# clients}}",
                        &[("count", MessageValue::Int(count as i64))],
                    ))
                    .app_class(tw("text-xs text-muted-foreground"))
                })
            },
        ))
        .child(write.then(|| {
            button(
                ButtonContent::Label(t("Revoke others")),
                ButtonVariant::DestructiveOutline,
                ButtonSize::Xs,
                move || {
                    controls.working(&Action::Others)
                        || records.with(
                            |v| matches!(v,Some(Ok(r)) if !r.clients.iter().any(|c|!c.current)),
                        )
                },
                move || controls.run(Action::Others, None),
            )
        }));
    view()
        .child(
            view()
                .test_id("authorized-clients")
                .child(header)
                .child(list),
        )
        .child(region(
            move || records.get(),
            move |records| {
                records
                    .and_then(Result::ok)
                    .and_then(|r| devices(r, controls, write))
            },
        ))
        .child(region(move || controls.error.get(), |e| e.map(text)))
        .child(region(
            move || controls.pending.get(),
            move |request| request.map(|(target, title)| revoke_dialog(controls, target, title)),
        ))
        .into_element()
}

fn access_date(value: &str) -> String {
    let Some(ms) = t3_client::time::epoch_ms(value) else {
        return String::new();
    };
    let settings = locale_settings();
    let zone = match settings.time_zone.as_str() {
        "UTC" | "Etc/UTC" | "GMT" => t3_i18n::TimeZone::Utc,
        name => t3_i18n::TimeZone::Iana(name.into()),
    };
    crate::i18n::formats().date(t3_i18n::DateFormat::DateMediumTimeShort, ms, &zone, None)
}

fn client_row(index: usize, client: ClientRow, controls: Controls, write: bool) -> AnyElement {
    let show = signal(false);
    let count = client.scopes.len();
    let summary = tr(
        "{count, plural, one {# scope} other {# scopes}}",
        &[("count", MessageValue::Int(count as i64))],
    );
    let label = tr(
        "Client scopes: show {count, plural, one {# scope} other {# scopes}}",
        &[("count", MessageValue::Int(count as i64))],
    );
    let scopes = stored(client.scopes);
    let status = match (client.live, client.last_connected.as_deref()) {
        (true, Some(date)) => {
            let elapsed = t3_client::time::epoch_ms(date).map(|start| {
                t3_client::view_logic::session::timestamps::format_elapsed_duration_label(
                    start,
                    nsh_core::time::unix_ms() as i64,
                )
            });
            elapsed.map_or_else(
                || t("Connected"),
                |duration| {
                    tr(
                        "Connected for {duration}",
                        &[("duration", MessageValue::Text(duration))],
                    )
                },
            )
        }
        (false, Some(date)) => tr(
            "Last connected at {date}",
            &[("date", MessageValue::Text(access_date(date)))],
        ),
        (true, None) => t("Connected"),
        (false, None) => t("Not connected yet."),
    };
    let details = view()
        .flex_1()
        .child(text(client.name).app_class(tw("text-sm font-medium text-foreground")))
        .child(client.current.then(|| text(t("This device"))))
        .child(text(status).app_class(tw("text-xs text-muted-foreground")))
        .child((!client.details.is_empty()).then(|| text(client.details)))
        .child(crate::ui::popover::preview_card(
            crate::ui::popover::PreviewCardOptions {
                open: Some(show),
                open_delay: 250.0,
                close_delay: 100.0,
                ..Default::default()
            },
            nsh_base::button()
                .accessibility_label(label)
                .app_class(tw("underline underline-offset-2 text-xs"))
                .on_press(move || {
                    if is_desktop() {
                        show.update(|open| *open = !*open);
                    }
                })
                .child(text(summary)),
            move || {
                let mut panel =
                    view().child(text(t("Granted scopes")).app_class(tw("mb-1 font-medium")));
                for scope in scopes.get() {
                    panel = panel.child(text(scope).app_class(tw("font-mono text-xs")));
                }
                panel.into_element()
            },
        ));
    view()
        .test_id(&format!("access-client-{index}"))
        .app_class(tw("flex flex-row items-center gap-2 px-3 py-2.5 sm:px-4"))
        .child(details)
        .child(
            (!client.current).then(|| controls.button(Action::Client(client.id), "Revoke", write)),
        )
        .into_element()
}

fn devices(data: Records, controls: Controls, write: bool) -> Option<AnyElement> {
    if data.pending.is_empty() && data.trusted.is_empty() {
        return None;
    }
    let mut list = view();
    for (index, r) in data.pending.into_iter().enumerate() {
        let title = r.code.as_ref().map_or_else(
            || t("Waiting for approval"),
            |code| {
                tr(
                    "Waiting for approval · {code}",
                    &[("code", MessageValue::Text(code.clone()))],
                )
            },
        );
        list = list.child(
            view()
                .test_id(&format!("access-request-{index}"))
                .app_class(tw(
                    "flex flex-wrap items-center gap-2 px-3 py-2.5 sm:flex-row sm:px-4",
                ))
                .child(view().flex_1().child(text(title)).child(text(tr(
                    "{device}. Approve only if the device in front of you shows this code.",
                    &[("device", MessageValue::Text(r.details))],
                ))))
                .child(controls.button(Action::Deny(r.id.clone()), "Deny", write))
                .child(controls.button(
                    Action::Approve(r.id),
                    "Approve",
                    write && r.code.is_some(),
                )),
        );
    }
    for (index, d) in data.trusted.into_iter().enumerate() {
        list = list.child(
            view()
                .test_id(&format!("access-trusted-{index}"))
                .app_class(tw("flex flex-row items-center gap-2 px-3 py-2.5 sm:px-4"))
                .child(
                    view()
                        .flex_1()
                        .child(text(d.name))
                        .child(d.code.map(text))
                        .child(text(tr(
                            "Trusted since {date}",
                            &[("date", MessageValue::Text(access_date(&d.since)))],
                        ))),
                )
                .child(controls.button(Action::Device(d.key), "Revoke", write)),
        );
    }
    Some(settings_section("connect-devices", "T3 Connect devices".into(), list).into_element())
}

fn revoke_dialog(controls: Controls, target: Action, title: String) -> AnyElement {
    use crate::ui::{
        dialog::{
            alert_dialog, alert_dialog_description, alert_dialog_footer, alert_dialog_header,
            alert_dialog_popup, alert_dialog_title,
        },
        switch::{CheckboxVariant, ControlState, checkbox},
    };
    let require = signal(true);
    let title = stored(title);
    let target = stored(target);
    let busy = move || controls.working(&target.get());
    alert_dialog(signal(true),move|state|{
        state.on_open_change(move |open|{if !open && !busy(){controls.pending.set(None);}});
        alert_dialog_popup(state,Default::default(),move||{
            let setting=checkbox(
                "Require approval for new devices",
                ControlState{checked:require.into(),disabled:Prop::bind(busy),..Default::default()},
                CheckboxVariant::Default,move |checked|require.set(checked),
            );
            let description=alert_dialog_description(state,||{
                "Approval for new devices is off, so a revoked device signed in to your T3 Connect account can connect again on its own.".into()
            });
            let header=view()
                .child(alert_dialog_title(state,move||Text::content(title.get())))
                .child(description)
                .child(view().flex_row().gap(px(12.0)).child(setting).child(
                    view().flex_1().child(text(t("Require approval for new devices"))).child(
                        text(t("The device has to be approved here before it can connect again."))
                            .app_class(tw("text-xs text-muted-foreground"))
                    )
                ))
                .child(region(move||controls.error.get(),|error|error.map(text)));
            let cancel=button(ButtonContent::Label(t("Cancel")),ButtonVariant::Outline,ButtonSize::Default,busy,
                move||controls.pending.set(None));
            let revoke=button(ButtonContent::Label(t("Revoke")),ButtonVariant::Destructive,ButtonSize::Default,busy,
                move||controls.run(target.get(),Some(require.get_untracked())));
            view().child(alert_dialog_header(state,header)).child(alert_dialog_footer(state,Default::default(),
                view().flex_row().child(cancel).child(revoke)))
        })
    }).into_element()
}
