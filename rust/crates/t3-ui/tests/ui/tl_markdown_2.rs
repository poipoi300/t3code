//! TL-MARKDOWN-2: real link rendering/actions from client paths and demo shell projections.

use nsh_core::prelude::*;
use nsh_engine::{EngineConfig, Root};
use nsh_testing::Headless;
use std::rc::Rc;
use t3_client::view_logic::session::links::resolve_markdown_file_link_meta;
use t3_client::view_logic::session::markdown_link_presentation::parse_thread_link_href;
use t3_client::{ClientStore, connection::PairingInput};
use t3_ui::runtime::{
    Rt,
    backend::Backend,
    demo::{DEMO_PAIRING_URL, DemoBackend},
    host::Choice,
};
use t3_ui::screens::thread::markdown::links::{
    MarkdownLinkContext, markdown_file_link, markdown_link_block, markdown_thread_link,
};
use t3_ui::state::nav::{Navigator, Route};

use crate::support::{DESKTOP, PHONE};

#[component]
fn FileFixture() -> impl IntoElement {
    t3_ui::theme::install(&Default::default());
    let host = t3_ui::screens::panel::PanelHost::new(Default::default(), |_| {});
    provide_context(host);
    let key = t3_client::types::ThreadKey {
        environment: t3_client::types::EnvironmentId("host".into()),
        thread: t3_client::types::ThreadId("reply".into()),
    };
    let panel = host.panel(key.clone());
    let source =
        computed(|| Rc::<str>::from("[See source](src/main.rs:12:3) [exact.rs](/repo/exact.rs:7)"));
    let windows =
        resolve_markdown_file_link_meta("C:/repo/main.rs:9:2", Some("C:/repo"), Some("C:/repo"))
            .expect("client path");
    view()
        .w_full()
        .h_full()
        .child(markdown_link_block(
            source,
            MarkdownLinkContext {
                thread: Some(key),
                cwd: Some(Rc::from("/repo")),
                ..Default::default()
            },
        ))
        .child(markdown_file_link(windows))
        .child(text(move || {
            SharedString::from(panel.with(|panel| {
                panel
                    .active()
                    .and_then(|surface| match &surface.content {
                        t3_ui::screens::panel::SurfaceContent::File {
                            relative_path,
                            reveal_line,
                            ..
                        } => Some(format!("panel:{relative_path};line:{reveal_line:?}")),
                        _ => None,
                    })
                    .unwrap_or_default()
            }))
        }))
}

#[test]
fn file_options_copy_current_host_paths_with_positions_and_close_at_both_sizes() {
    for window in [DESKTOP, PHONE] {
        let mut app = Headless::start(EngineConfig {
            ui_sets: t3_ui::registries(),
            viewport: size(px(window.0), px(window.1)),
            ..EngineConfig::new(Root::of::<FileFixture>())
        })
        .expect("file fixture");
        app.wait_for("See source")
            .expect("authored descriptive label retained");
        app.press("role:button[File options for main.rs · L12:C3]")
            .expect("fallback opens its own menu");
        app.press("role:menuitem[Copy relative path]")
            .expect("relative path");
        assert_eq!(app.screen().clipboard(), Some("src/main.rs:12:3"));
        assert!(
            !app.screen()
                .matches("role:menuitem[Copy relative path]")
                .expect("menu query")
        );
        app.press("role:button[File options for main.rs · L12:C3]")
            .expect("reopen");
        app.press("role:menuitem[Copy full path]")
            .expect("full path");
        assert_eq!(app.screen().clipboard(), Some("/repo/src/main.rs:12:3"));
        app.press("role:button[File options for main.rs · L9:C2]")
            .expect("Windows host link");
        app.press("role:menuitem[Copy full path]")
            .expect("Windows path");
        assert_eq!(app.screen().clipboard(), Some("C:\\repo\\main.rs:9:2"));
        app.press("role:link[exact.rs · L7]")
            .expect("known workspace path opens the client panel");
        app.wait_for("panel:exact.rs;line:Some(7)")
            .expect("path and reveal line in client-owned panel state");
        assert!(
            app.screen().opened_urls().is_empty(),
            "a file path never opens this client's filesystem"
        );
    }
}

#[component]
fn ExternalFixture() -> impl IntoElement {
    t3_ui::theme::install(&Default::default());
    t3_ui::i18n::set_language("fr", &[]);
    let source = computed(|| {
        Rc::<str>::from(
            "[Settings](https://example.com) and [another](https://example.com/path) and [local](https://box.ts.net)",
        )
    });
    view()
        .w_full()
        .h_full()
        .child(markdown_link_block(source, Default::default()))
}

#[test]
fn external_links_keep_literal_labels_open_the_url_and_share_failed_favicon_state() {
    for window in [DESKTOP, PHONE] {
        let mut app = Headless::start(EngineConfig {
            ui_sets: t3_ui::registries(),
            viewport: size(px(window.0), px(window.1)),
            ..EngineConfig::new(Root::of::<ExternalFixture>())
        })
        .expect("external fixture");
        app.wait_for("role:link[Settings]")
            .expect("authored catalogue-like text stays literal in French");
        assert_eq!(
            app.screen()
                .count("id:markdown-link-favicon")
                .expect("public favicon nodes"),
            2,
            "public links have favicons; the tailnet link does not send its host"
        );
        app.press("role:link[Settings]")
            .expect("activate external link");
        assert_eq!(app.screen().opened_urls(), &["https://example.com"]);
        app.fail_images("https://www.google.com/s2/favicons?domain=example.com&sz=32")
            .expect("favicon fails");
        app.wait_for("role:link[another]")
            .expect("fallback preserves link content");
        assert!(
            !app.screen()
                .matches("id:markdown-link-favicon")
                .expect("all mounted links share the failed-host set")
        );
        app.press("role:link[another]")
            .expect("fallback remains a working link");
        assert_eq!(
            app.screen().opened_urls().last().map(String::as_str),
            Some("https://example.com/path")
        );
    }
}

#[component]
fn ThreadFixture() -> impl IntoElement {
    t3_ui::theme::install(&Default::default());
    let store = ClientStore::default();
    let backend = Rc::new(DemoBackend::new(store.clone()));
    let runtime = Rt::new(store, backend.clone(), Choice::Demo);
    provide_context(runtime);
    let nav = Navigator::new(Route::Home);
    provide_context(nav);
    let ready = signal(None);
    spawn(async move {
        let paired = backend
            .pair(PairingInput {
                pairing_url: Some(DEMO_PAIRING_URL.into()),
                ..Default::default()
            })
            .await
            .expect("demo pairing");
        ready.set(Some(paired));
    });
    view()
        .w_full()
        .h_full()
        .child(text(move || {
            SharedString::from(match nav.top() {
                Route::Thread(key) => format!(
                    "destination:{};environment_matches:{}",
                    key.thread.0,
                    ready.get().as_ref() == Some(&key.environment)
                ),
                route => route.path(),
            })
        }))
        .child(dynamic(move || {
            ready.get().map(|environment| {
                untracked(|| {
                    view()
                        .child(markdown_thread_link(
                            environment.clone(),
                            parse_thread_link_href("t3-thread://v1/thread-shared%2Dui")
                                .expect("legacy"),
                            None,
                        ))
                        .child(markdown_thread_link(
                            environment,
                            parse_thread_link_href("t3-thread://v2/peer/unknown")
                                .expect("explicit"),
                            None,
                        ))
                })
            })
        }))
}

#[test]
fn thread_chips_read_the_real_shell_navigate_to_the_decoded_id_and_refuse_unreachable_targets() {
    for window in [DESKTOP, PHONE] {
        let mut app = Headless::start(EngineConfig {
            ui_sets: t3_ui::registries(),
            viewport: size(px(window.0), px(window.1)),
            ..EngineConfig::new(Root::of::<ThreadFixture>())
        })
        .expect("thread fixture");
        app.wait_for("Build the shared Rust UI slice")
            .expect("title from the demo's canonical shell projection");
        app.wait_for("Thread on another machine")
            .expect("unreachable link settles immediately");
        assert!(
            !app.screen()
                .matches("role:link[Thread, Thread on another machine]")
                .expect("no navigation target for missing peer")
        );
        app.press("role:link[Thread, Build the shared Rust UI slice]")
            .expect("open cached decoded thread");
        app.wait_for("destination:thread-shared-ui;environment_matches:true")
            .expect("navigator destination uses decoded id");
        assert!(
            app.screen().opened_urls().is_empty(),
            "thread links stay inside the shared navigator"
        );
    }
}
