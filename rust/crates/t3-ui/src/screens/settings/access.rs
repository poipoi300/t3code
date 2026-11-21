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
    let rt = rt();
    region(
        move || {
            rt.store.track_environments();
            connected
                .then(|| {
                    OPTIONS
                        .iter()
                        .map(|(scope, _)| services?.get().grants(&id, *scope))
                        .collect::<Option<Vec<_>>>()
                })
                .flatten()
        },
        move |grants| {
            let Some(grants) = grants else {
                return text(if connected {
                    t("Checking permissions…")
                } else {
                    t("Connect to this environment to view this session’s permissions.")
                })
                .app_class(tw("py-3 text-xs text-muted-foreground"))
                .into_element();
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
                    region(
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
                    )
                },
            )))
            .into_element()
        },
    )
}
