//! The root: reads the launch address, builds the runtime and UI state once, then shows the
//! standalone or workspace layout.

use std::cell::RefCell;
use std::rc::Rc;

use nsh_core::prelude::*;
use t3_client::ClientStore;
use t3_client::composition::ClientRuntime;
use t3_client::platform::Platform;

use crate::runtime::Rt;
use crate::runtime::host::{self, Started};
use crate::screens::{router, shell};
use crate::state::clock;
use crate::state::nav::{Navigator, Route, initial_route};
use crate::state::panels::Panels;
use crate::theme::{self, ThemeChoice, palette};

#[component]
pub fn App() -> impl IntoElement {
    // Booting waits for the launch address (the page URL, a deep link) and the backend it chooses
    // (the real client, or the demo when asked). This also keeps token installation and task
    // spawns out of the root's first synchronous render, which aborts NSH's web runtime ("a prop
    // read or written after its owner was disposed").
    let address = signal(None::<Option<String>>);
    let slot: Rc<RefCell<Option<(ClientStore, Started)>>> = Rc::new(RefCell::new(None));
    let filling = slot.clone();
    spawn(async move {
        let launched = app_url().await;
        let store = ClientStore::default();
        let started = host::start(store.clone(), host::launch_choice(launched.as_deref())).await;
        *filling.borrow_mut() = Some((store, started));
        address.set(Some(launched));
    });
    // Untracked: root builds the runtime once; a signal it happens to read must not rebuild it.
    // `address` is set once, so this runs once with the slot filled.
    view().w_full().h_full().child(dynamic(move || {
        address.get().and_then(|launched| {
            let (store, started) = slot.borrow_mut().take()?;
            Some(untracked(|| root(launched.as_deref(), store, started)))
        })
    }))
}

fn root(address: Option<&str>, store: ClientStore, started: Started) -> AnyElement {
    let paired = started.paired;
    let has_environments = !started.backend.environments().is_empty();
    booted_tree(initial_route(address, paired || has_environments), store, started)
}

/// The app once its backend has started, opened at `route`: what `App` shows after the launch
/// address is read. Tests boot the host's client (`runtime::client::start`) and mount this.
pub fn booted_tree(route: Route, store: ClientStore, started: Started) -> AnyElement {
    let runtime = Rt::new(store, started.backend, started.choice);
    let client = runtime.backend().client();
    app_tree(runtime, route, move || match client {
        Some(client) => provide_client_services(&client),
        None => provide_without_client(),
    })
}

/// Everything below the boot: contexts, client services (`provide_services`), the routes and
/// the root hosts. Tests mount it with a fake-platform client, as `root` does with the host's.
pub fn app_tree(runtime: Rt, route: Route, provide_services: impl FnOnce()) -> AnyElement {
    theme::install(&ThemeChoice::default());
    provide_context(theme::text_view_parts());
    if let Some(highlighter) = crate::host::code_highlighter() {
        provide_context(highlighter);
    }
    crate::i18n::set_language("system", &[locale().to_string()]);
    provide_context(runtime);
    provide_services();
    let nav = Navigator::new(route);
    provide_context(nav);
    let panels = Panels::new();
    provide_context(panels);
    clock::start();
    // SH-TITLEBAR: the hidden native bar (NSH draws the traffic lights or caption buttons); the
    // headers reserve their space through screens::titlebar.
    crate::screens::titlebar::install_window_titlebar(crate::i18n::Text::content("T3 Code"));
    // The client exposes no designated primary yet (request in client-seam-requests.md): unscoped.
    let keys = crate::keys::install(panels, signal(None));
    let command_palette = crate::screens::palette::palette_controller();
    keys.register(crate::keys::bindings::Scope {
        commands: command_palette.command_handlers(),
        when: Rc::new(move |_| command_palette.is_mounted()),
        ..Default::default()
    });

    let top = computed(move || nav.top());
    let standalone = computed(move || top.get().is_standalone());
    let content = view()
        .flex_1()
        .w_full()
        .bg(move || palette().background)
        .text_color(move || palette().foreground)
        .pt(move || safe_area().top)
        .app_class(theme::tw("relative"))
        // Absolute drag background behind the routes; desktop windows only.
        .child(crate::screens::titlebar::desktop_titlebar())
        // Untracked: only standalone-or-shell (then the route) switches the tree; a signal read
        // while building it (theme tokens, layout) must not rebuild the whole shell.
        .child(dynamic(move || {
            let standalone = standalone.get();
            untracked(|| {
                if standalone {
                    dynamic(move || {
                        let route = top.get();
                        untracked(|| router::screen(&route))
                    })
                    .into_element()
                } else {
                    crate::screens::palette::command_palette(shell::app_shell()).into_element()
                }
            })
        }))
        // ConfirmDialogHost at the root, as the web mounts it: every confirm request needs one.
        .child(crate::ui::confirm::confirm_dialog_host())
        // CONN-1: pairing links handed to the running app (confirmed by origin, through the
        // confirm host above) and the storage notice, once; nothing on the demo backend.
        .child(
            view()
                .test_id("connection-notices")
                .accessible(false)
                .app_class(theme::tw("absolute size-0"))
                .child(crate::screens::connect::connection_notices()),
        )
        // ThreadNotificationCoordinator, started once from the app root (hidden, effects only).
        .child(crate::screens::notifications::thread_notifications())
        .into_element();
    crate::ui::toast::toast_provider(
        crate::ui::toast::anchored_toast_provider(crate::keys::bindings::keyboard_events(
            keys, content,
        )),
        crate::ui::toast::Position::TopRight,
    )
    .into_element()
}

/// The client services every screen reads from context, bound once above the routes
/// (`runtime/services`). Plans, the thread header, Markdown links and rewind reach the client
/// themselves. Tests pass a fake-platform runtime here, as `root` passes the host's.
pub fn provide_client_services<P: Platform + Clone + 'static>(client: &ClientRuntime<P>) {
    use crate::runtime::services::{
        diff, files, git_panel, navigation, shell, terminal, thread_ops,
    };
    use crate::screens::panel::{self, PanelRegistry};
    thread_ops::bind_confirmation(client);
    provide_context(git_panel::git_actions(client.git_panel()));
    provide_context(git_panel::git_actions_registration(client.git_panel()));
    provide_context(git_panel::branch_registration(client.git_panel()));
    provide_context(git_panel::thread_details_runtime(client.git_panel()));
    provide_context(thread_ops::approval_services(client));
    provide_context(navigation::new_thread_runtime(client));
    provide_context(navigation::add_project_runtime(client.add_project()));
    provide_context(files::files(client));
    provide_context(files::file_viewer(client));
    // Every address opened outside the app goes through the client's policy (`links.rs`).
    provide_context(crate::runtime::services::links::link_opener(client));
    provide_context(shell::sidebar_width_store(client));
    provide_context(shell::sidebar_shelves_store(client));
    provide_context(crate::screens::notifications::notification_settings(client));
    let host = panel::workspace_panel_host();
    provide_context(host);
    // RightPanelSurfaceContent.tsx's surfaces. Browser, Terminal tabs, pull requests, devices and
    // side chats register here when their lanes land (briefs/ui-mounts-notes.md).
    provide_context(PanelRegistry::new(vec![
        panel::file_viewer::files_panel_registration(host),
        panel::diff::diff_panel_registration(diff::diff_panel_runtime(client), host),
    ]));
    // TERM's drawer and header toggle: the workspace builds `terminal_registration` per thread.
    provide_context(terminal::TerminalServices::new(client.terminals()));
}

/// The demo and an unavailable host: the workspace host with no surface to offer, as the web
/// offers none it cannot load.
pub fn provide_without_client() {
    let host = crate::screens::panel::workspace_panel_host();
    provide_context(host);
    provide_context(crate::screens::panel::PanelRegistry::new(vec![]));
}
