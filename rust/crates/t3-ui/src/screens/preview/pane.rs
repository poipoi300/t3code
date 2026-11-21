//! Port of PreviewPanel.tsx, PreviewChromeRow.tsx and PreviewEmptyState.tsx.

use std::collections::BTreeSet;

use futures::StreamExt;
use nsh_core::prelude::*;
use t3_client::{
    composition::ClientRuntime,
    platform::Platform,
    preview::{
        browser::{BrowserAccess, BrowserCommandError, BrowserPreview},
        socket::BrowserInput,
    },
    view_logic::shell::preview_state::{PreviewStore, ThreadPreview},
};
use t3_contracts::{
    base_schemas::TrimmedNonEmptyString,
    preview::{
        DiscoveredLocalServer, PreviewNavStatus, PreviewRuntime, PreviewSessionSnapshot,
        PreviewTabId,
    },
};

use crate::{
    i18n::{Text, Value, t, tr},
    runtime::tasks::{TaskOwner, watch},
    screens::panel::{
        Action, PanelHost, PanelRegistration, SurfaceContent, SurfaceContext, SurfaceKind,
    },
    theme::tw,
    ui::button::{ButtonContent, ButtonSize, ButtonVariant, button},
    ui::icon::{Icon, icon_with_class},
};

use super::stream::{StreamControls, server_browser_surface};

pub fn registration<P: Platform + Clone + 'static>(
    client: &ClientRuntime<P>,
    host: PanelHost,
) -> PanelRegistration {
    let client = client.clone();
    let sessions = signal_with(PreviewStore::<()>::default(), SignalEquals::Never);
    PanelRegistration::new(
        SurfaceKind::Browser,
        Prop::new(None::<Text>),
        move |key| host.apply(&key, Action::OpenBrowser(None)),
        move |context| {
            let service = client.browser_preview(context.thread.clone());
            preview_panel(context, service, sessions).into_element()
        },
    )
}

fn preview_panel<P: Platform + Clone + 'static>(
    context: SurfaceContext,
    service: BrowserPreview<P>,
    sessions: Signal<PreviewStore<()>>,
) -> impl IntoElement {
    let key = stored(context.thread);
    let surface = context.surface;
    let visible = context.visible;
    let stream = StreamControls::new();
    let host = context.host;
    let panel = host.panel(key.get());
    let service = stored(service);
    let tasks = TaskOwner::here();
    let tab = computed(move || {
        surface.with(|surface| match surface.as_ref().map(|s| &s.content) {
            Some(SurfaceContent::Preview { resource_id }) => {
                resource_id.clone().map(TrimmedNonEmptyString)
            }
            _ => None,
        })
    });
    let busy = signal(false);
    let synchronized = signal(false);
    let error = signal(None::<String>);
    let access = signal(service.get().access());
    let configured = signal(service.get().configured_urls());
    let servers = signal(Vec::<DiscoveredLocalServer>::new());
    let address = signal(String::new());
    let snapshot = computed(move || {
        let tab = tab.get()?;
        sessions.with(|store| store.threads.get(&key.get())?.sessions.get(&tab.0).cloned())
    });
    let page_address = computed(move || {
        snapshot.with(|snapshot| {
            snapshot
                .as_ref()
                .map_or_else(String::new, |snapshot| match &snapshot.nav_status {
                    PreviewNavStatus::Idle {} => String::new(),
                    PreviewNavStatus::Loading { url, .. }
                    | PreviewNavStatus::Success { url, .. }
                    | PreviewNavStatus::LoadFailed { url, .. } => url.0.clone(),
                })
        })
    });
    effect(move || address.set(page_address.get()));

    let mut changes = service.get().changes();
    watch(async move {
        while changes.next().await.is_some() {
            let service = service.get();
            access.set(service.access());
            configured.set(service.configured_urls());
        }
    });

    effect(move || {
        synchronized.set(false);
        if !matches!(access.get(), BrowserAccess::Connected { .. }) {
            return;
        }
        let service = service.get();
        let thread = key.get();
        watch(async move {
            for _ in 0..2 {
                // The endpoint starts the subscription immediately and queues events while the list loads.
                let mut events = service.events();
                let Ok(list) = service.list().await else {
                    return;
                };
                sessions.update(|store| {
                    store.update(thread.clone(), |state| state.reconcile(list));
                });
                synchronized.set(true);
                loop {
                    match events.next().await {
                        Some(Ok(event)) => sessions.update(|store| {
                            store.update(thread.clone(), |state| state.apply_event(event));
                        }),
                        Some(Err(t3_client::ClientError::InvalidPayload(_))) => {
                            // The RPC stream reports consumer overflow as invalid payload.
                            synchronized.set(false);
                            break;
                        }
                        _ => return,
                    }
                }
            }
        });
    });

    effect(move || {
        if !synchronized.get() || busy.get() {
            return;
        }
        let thread = key.get();
        let (tabs, hidden) = sessions.with(|store| {
            store
                .threads
                .get(&thread)
                .map_or_else(Default::default, |state| {
                    let hidden = state
                        .sessions
                        .values()
                        .filter(|snapshot| {
                            snapshot.runtime == Some(Some(PreviewRuntime::Server))
                                && snapshot.reveal == Some(Some(false))
                        })
                        .map(|snapshot| snapshot.tab_id.0.clone())
                        .collect::<BTreeSet<_>>();
                    (state.session_order.clone(), hidden)
                })
        });
        let present = panel.with_untracked(|panel| {
            panel
                .surfaces
                .iter()
                .filter_map(|surface| match &surface.content {
                    SurfaceContent::Preview {
                        resource_id: Some(id),
                    } => Some(id.clone()),
                    _ => None,
                })
                .collect::<BTreeSet<_>>()
        });
        let wanted = tabs
            .iter()
            .filter(|id| !hidden.contains(*id) || present.contains(*id))
            .cloned()
            .collect::<BTreeSet<_>>();
        if wanted != present {
            host.apply(&thread, Action::ReconcileBrowsers { tabs, hidden });
        }
    });

    effect(move || {
        let urls = configured.get();
        if tab.get().is_some() || !matches!(access.get(), BrowserAccess::Connected { .. }) {
            servers.set(Vec::new());
            return;
        }
        let mut discovery = service.get().discovery(urls);
        watch(async move {
            while let Some(Ok(list)) = discovery.next().await {
                servers.set(list);
            }
        });
    });

    let open = move |url: String| {
        if busy.get_untracked() || tab.get_untracked().is_some() {
            return;
        }
        let service = service.get();
        let thread = key.get();
        busy.set(true);
        error.set(None);
        tasks.spawn(async move {
            let result = service.open(url).await;
            busy.set(false);
            match result {
                Ok(snapshot) => {
                    let tab = snapshot.tab_id.0.clone();
                    sessions.update(|store| {
                        store.update(thread.clone(), |state| state.apply_snapshot(Some(snapshot)));
                    });
                    host.apply(&thread, Action::OpenBrowser(Some(tab)));
                }
                Err(problem) => error.set(Some(command_error(problem))),
            }
        });
    };
    let submit = move || {
        let draft = address.get_untracked();
        if draft.trim().is_empty() {
            return;
        }
        if let Some(url) = service.get().address(&draft) {
            if tab.get_untracked().is_some() {
                stream.send(BrowserInput::Navigate { url });
            } else {
                open(url);
            }
        } else {
            error.set(Some(t("Enter an http or https URL.")));
        }
    };
    let close = move || {
        let (Some(tab), Some(surface)) = (tab.get_untracked(), surface.get_untracked()) else {
            return;
        };
        if busy.get_untracked() {
            return;
        }
        let service = service.get();
        let thread = key.get();
        busy.set(true);
        error.set(None);
        tasks.spawn(async move {
            let result = service.close(tab.clone()).await;
            busy.set(false);
            match result {
                Ok(()) | Err(BrowserCommandError::TabClosed) => {
                    sessions.update(|store| {
                        store.update(thread.clone(), |state| state.begin_close(&tab.0));
                    });
                    host.apply(&thread, Action::CloseSurface(surface.id));
                }
                Err(problem) => error.set(Some(command_error(problem))),
            }
        });
    };

    view()
        .app_class(tw("flex min-h-0 flex-1 flex-col"))
        .test_id("preview-panel")
        .child(dynamic(move || match access.get() {
            BrowserAccess::Disconnected => {
                panel_message(t("This environment is not connected.")).into_element()
            }
            BrowserAccess::Restricted => panel_message(t(
                "Pair this client again with preview access to control browser previews.",
            ))
            .into_element(),
            BrowserAccess::Connected { .. } => view()
                .app_class(tw("flex min-h-0 flex-1 flex-col"))
                .child(preview_chrome_row(
                    address, busy, snapshot, tab, stream, submit, close,
                ))
                .child(when(
                    move || error.get().is_some(),
                    move || {
                        text(move || error.get().unwrap_or_default())
                            .app_class(tw("px-2 py-1 text-sm text-destructive"))
                    },
                ))
                .child(dynamic(move || match tab.get() {
                    None => {
                        let recents = sessions.with(|store| {
                            store
                                .threads
                                .get(&key.get())
                                .map_or_else(Vec::new, |state| recent_urls(state))
                        });
                        preview_empty_state(servers.get(), recents, open)
                    }
                    Some(tab_id) => {
                        let tab_id = stored(tab_id);
                        when(visible, move || {
                            let runtime = computed(move || {
                                snapshot.with(|s| {
                                    s.as_ref().and_then(|snapshot| snapshot.runtime.flatten())
                                })
                            });
                            dynamic(move || match runtime.get() {
                                Some(PreviewRuntime::Server) => {
                                    server_browser_surface(service.get(), tab_id.get(), stream)
                                        .into_element()
                                }
                                Some(PreviewRuntime::Desktop) => {
                                    panel_message(t("Browser unavailable")).into_element()
                                }
                                None => panel_message(t("Connecting to browser…")).into_element(),
                            })
                        })
                        .into_element()
                    }
                }))
                .into_element(),
        }))
}

fn command_error(error: BrowserCommandError) -> String {
    match error {
        BrowserCommandError::InvalidUrl => t("Enter an http or https URL."),
        BrowserCommandError::TabClosed => t("This browser tab is closed."),
        BrowserCommandError::ControlRequired => t("Take control of this browser tab first."),
        BrowserCommandError::Unavailable => t("Browser unavailable"),
        BrowserCommandError::Client(_) => t("Unable to open browser"),
    }
}

fn preview_chrome_row(
    address: Signal<String>,
    busy: Signal<bool>,
    snapshot: Memo<Option<PreviewSessionSnapshot>>,
    tab: Memo<Option<PreviewTabId>>,
    stream: StreamControls,
    submit: impl Fn() + Copy + 'static,
    close: impl Fn() + Copy + 'static,
) -> impl IntoElement {
    view()
        .app_class(tw("flex h-10 min-h-10 shrink-0 items-center gap-1 border-b border-border/60 bg-background px-2"))
        .child(
            view().app_class(tw("flex items-center gap-0.5")).label(t("Navigation"))
                .child(button(ButtonContent::Icon { icon: Icon::ArrowLeft, label: t("Back") },
                    ButtonVariant::Ghost, ButtonSize::IconXs,
                    move || !stream.controls_page() || !snapshot.with(|snapshot| snapshot.as_ref().is_some_and(|s| s.can_go_back)),
                    move || stream.send(BrowserInput::History { delta: -1 })))
                .child(button(ButtonContent::Icon { icon: Icon::ArrowRight, label: t("Forward") },
                    ButtonVariant::Ghost, ButtonSize::IconXs,
                    move || !stream.controls_page() || !snapshot.with(|snapshot| snapshot.as_ref().is_some_and(|s| s.can_go_forward)),
                    move || stream.send(BrowserInput::History { delta: 1 })))
                .child(dynamic(move || {
                    let loading = snapshot.with(|snapshot| snapshot.as_ref()
                        .is_some_and(|s| matches!(s.nav_status, PreviewNavStatus::Loading { .. })));
                    button(
                        ButtonContent::Icon {
                            icon: if loading { Icon::X } else { Icon::RotateCw },
                            label: if loading { t("Stop") } else { t("Refresh") },
                        },
                        ButtonVariant::Ghost,
                        ButtonSize::IconXs,
                        move || !stream.controls_page(),
                        move || stream.send(if loading { BrowserInput::StopLoading } else { BrowserInput::Reload })
                    )
                })),
        )
        .child(
            nsh_base::input()
                .label(t("Search or enter URL"))
                .placeholder(t("Search or enter URL"))
                .value(move || address.get())
                .disabled(move || busy.get() || tab.get().is_some() && !stream.controls_page())
                .on_change(move |value: &str| address.set(value.to_owned()))
                .submit_behavior(SubmitBehavior::Submit)
                .on_submit(move |_: &str| submit())
                .app_class(tw("h-7 min-w-0 flex-1 rounded-md px-2 text-sm")),
        )
        .child(when(move || tab.get().is_some(), move || {
            button(
                ButtonContent::Icon { icon: Icon::X, label: t("Close browser tab") },
                ButtonVariant::Ghost,
                ButtonSize::IconXs,
                move || busy.get(),
                close,
            )
        }))
}

fn recent_urls(state: &ThreadPreview<()>) -> Vec<String> {
    state.recent_urls.iter().take(8).cloned().collect()
}

fn preview_empty_state(
    servers: Vec<DiscoveredLocalServer>,
    recents: Vec<String>,
    open: impl Fn(String) + Copy + 'static,
) -> AnyElement {
    if servers.is_empty() && recents.is_empty() {
        return view()
            .app_class(tw("flex min-h-0 flex-1 flex-col items-center justify-center gap-3 p-8 text-center"))
            .child(icon_with_class(Icon::Globe, "size-4.5 text-muted-foreground"))
            .child(text(t("No preview yet")).app_class(tw("text-sm font-medium")))
            .child(text(t("Type a URL above, or run a dev script. Local servers this thread starts will show up here automatically."))
                .app_class(tw("text-sm text-muted-foreground")))
            .into_element();
    }
    scroll()
        .flex_1()
        .min_h(px(0.0))
        .child(
            view()
                .app_class(tw("mx-auto flex w-full max-w-xl flex-col gap-6 px-5 py-8"))
                .child((!recents.is_empty()).then(|| {
                    view()
                        .app_class(tw("flex flex-col gap-3"))
                        .child(
                            text(t("Recently used"))
                                .app_class(tw("text-sm font-medium text-muted-foreground")),
                        )
                        .child(
                            recents
                                .into_iter()
                                .map(|url| preview_recent_url_card(url, open)),
                        )
                }))
                .child((!servers.is_empty()).then(|| {
                    view()
                        .app_class(tw("flex flex-col gap-3"))
                        .child(
                            text(t("Local servers"))
                                .app_class(tw("text-sm font-medium text-muted-foreground")),
                        )
                        .child(
                            servers
                                .into_iter()
                                .map(|server| preview_local_server_card(server, open)),
                        )
                        .child(
                            text(t(
                                "Select a live local server to open it in this browser tab.",
                            ))
                            .app_class(tw("px-1 text-xs text-muted-foreground")),
                        )
                })),
        )
        .into_element()
}

fn preview_local_server_card(
    server: DiscoveredLocalServer,
    open: impl Fn(String) + 'static,
) -> impl IntoElement {
    let url = server.url.0;
    let label = tr("Open {url}", &[("url", Value::Text(url.clone()))]);
    let name = server
        .process_name
        .map_or_else(|| t("Listening"), |name| name.0);
    nsh_base::button()
        .accessibility_label(label)
        .on_press(move || open(url.clone()))
        .app_class(tw(
            "flex min-w-0 items-center gap-2 rounded-md p-2 text-left hover:bg-accent",
        ))
        .child(icon_with_class(
            Icon::Globe,
            "size-4 shrink-0 text-muted-foreground",
        ))
        .child(
            view()
                .app_class(tw("flex min-w-0 flex-col"))
                .child(text(name).app_class(tw("truncate text-sm font-medium")))
                .child(
                    text(format!("{}:{}", server.host.0, server.port))
                        .app_class(tw("truncate text-xs text-muted-foreground")),
                ),
        )
}

fn preview_recent_url_card(url: String, open: impl Fn(String) + 'static) -> impl IntoElement {
    let target = url.clone();
    nsh_base::button()
        .accessibility_label(tr("Open {url}", &[("url", Value::Text(url.clone()))]))
        .on_press(move || open(target.clone()))
        .app_class(tw(
            "flex min-w-0 items-center gap-2 rounded-md p-2 text-left hover:bg-accent",
        ))
        .child(icon_with_class(
            Icon::Globe,
            "size-4 shrink-0 text-muted-foreground",
        ))
        .child(text(url).app_class(tw("truncate text-sm")))
}

fn panel_message(message: String) -> impl IntoElement {
    view()
        .app_class(tw(
            "flex min-h-0 flex-1 flex-col items-center justify-center gap-3 p-8 text-center",
        ))
        .child(text(message).app_class(tw("max-w-sm text-sm text-muted-foreground")))
}
