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
fn TableRecycleFixture() -> impl IntoElement {
    use t3_ui::screens::thread::{
        blocks::flatten::BlockId,
        markdown::links::{MarkdownTablesState, markdown_table_leaf},
    };
    t3_ui::theme::install(&Default::default());
    let tables = MarkdownTablesState::default();
    let mounted = signal(true);
    view()
        .w_full()
        .h_full()
        .child(
            nsh_base::button()
                .accessibility_label(t3_ui::i18n::t("Recycle table"))
                .on_press(move || mounted.set(!mounted.get_untracked()))
                .child(text(t3_ui::i18n::t("Recycle table"))),
        )
        .child(dynamic(move || {
            mounted.get().then(|| {
                untracked(|| {
                    let id = BlockId {
                        message_id: "table-message".into(),
                        index: 2,
                    };
                    markdown_table_leaf(
                        computed(|| {
                            Rc::from("| Name | Result |\n| --- | --- |\n| test | 12 passed |")
                        }),
                        id,
                        tables,
                    )
                })
            })
        }))
}

#[test]
fn table_copied_feedback_survives_recycling_and_reopens_the_current_menu() {
    for window in [DESKTOP, PHONE] {
        let mut app = Headless::start(EngineConfig {
            ui_sets: t3_ui::registries(),
            viewport: size(px(window.0), px(window.1)),
            clock: nsh_core::time::Clock::manual(std::time::SystemTime::UNIX_EPOCH),
            ..EngineConfig::new(Root::of::<TableRecycleFixture>())
        })
        .expect("table fixture");
        app.wait_for("12 passed").expect("table mounted");
        app.press("role:button[Copy table]").expect("table actions");
        app.press("role:menuitem[Copy as Markdown]")
            .expect("copy table");
        app.press("role:button[Recycle table]")
            .expect("unmount cell");
        assert!(
            !app.screen()
                .matches("role:button[Copied]")
                .expect("unmounted")
        );
        app.press("role:button[Recycle table]")
            .expect("remount cell");
        app.press("role:button[Copied]")
            .expect("retained copied feedback");
        app.press("role:menuitem[Copy as CSV]")
            .expect("reopened copy menu");
        assert_eq!(
            app.screen().clipboard(),
            Some("Name,Result\ntest,12 passed")
        );
    }
}

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
    let source = computed(|| Rc::<str>::from("[See source](src/main.rs:12:3)"));
    let panel_source =
        computed(|| Rc::<str>::from("[main.rs](src/main.rs:12:3) [exact.rs](/repo/exact.rs:7)"));
    let windows =
        resolve_markdown_file_link_meta("C:/repo/main.rs:9:2", Some("C:/repo"), Some("C:/repo"))
            .expect("client path");
    view()
        .w_full()
        .h_full()
        .child(markdown_link_block(
            source,
            MarkdownLinkContext {
                cwd: Some(Rc::from("/repo")),
                ..Default::default()
            },
        ))
        .child(markdown_link_block(
            panel_source,
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
        app.wait_for_gone(
            "role:menuitem[Copy relative path]",
            std::time::Duration::from_secs(5),
        )
        .expect("copy dismisses the menu");
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
        app.press("role:link[main.rs · L12:C3]")
            .expect("relative citation opens the mounted panel");
        app.wait_for("text:panel:src/main.rs;line:Some(12)")
            .expect("relative path and reveal line");
        app.press("role:link[exact.rs · L7]")
            .expect("known workspace path opens the client panel");
        app.wait_for("text:panel:exact.rs;line:Some(7)")
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
        app.wait_for("text:destination:thread-shared-ui;environment_matches:true")
            .expect("navigator destination uses decoded id");
        assert!(
            app.screen().opened_urls().is_empty(),
            "thread links stay inside the shared navigator"
        );
    }
}

#[component]
fn AssistantFileFixture() -> impl IntoElement {
    use t3_client::thread_views::messages_timeline::{
        MessagesTimelineRowContent, MessagesTimelineRowsInput, derive_messages_timeline_rows,
    };
    use t3_ui::runtime::Shared;
    use t3_ui::screens::thread::rows::assistant::{
        AssistantRowContext, AssistantRowsState, assistant_row, assistant_timeline_row,
    };
    t3_ui::theme::install(&Default::default());
    let store = ClientStore::default();
    let backend = Rc::new(DemoBackend::new(store.clone()));
    provide_context(Rt::new(store, backend.clone(), Choice::Demo));
    let nav = Navigator::new(Route::Home);
    provide_context(nav);
    let host = t3_ui::screens::panel::PanelHost::new(Default::default(), |_| {});
    provide_context(host);
    let ready = signal(None);
    spawn(async move {
        let environment = backend
            .pair(PairingInput {
                pairing_url: Some(DEMO_PAIRING_URL.into()),
                ..Default::default()
            })
            .await
            .expect("pair demo");
        let key = t3_client::types::ThreadKey {
            environment,
            thread: t3_client::types::ThreadId("thread-shared-ui".into()),
        };
        nav.open(Route::Thread(key.clone()));
        ready.set(Some(key));
    });
    let panel = computed(move || ready.get().map(|key| host.panel(key)));
    view().w_full().h_full()
        .child(dynamic(move || ready.get().map(|_| untracked(|| {
            let entry = Rc::new(serde_json::from_value(serde_json::json!({
                "id":"row-answer", "kind":"message", "createdAt":"2026-11-13T12:34:00Z",
                "message":{"id":"answer","role":"assistant","text":"[Open source](src/main.rs:12:3)","streaming":false,"runId":"run","createdAt":"2026-11-13T12:34:00Z","updatedAt":"2026-11-13T12:34:00Z"},
                "projectedItem":{"position":0,"visibility":"inherited","sourceThreadId":"thread-shared-ui","sourceItemId":"source-item",
                    "item":{"id":"source-item","threadId":"thread-shared-ui","type":"assistant_message","messageId":"answer","text":"[Open source](src/main.rs:12:3)",
                    "streaming":false,"runId":"run","nodeId":null,"providerThreadId":null,"providerTurnId":null,"nativeItemRef":null,
                    "parentItemId":null,"ordinal":0,"status":"completed","title":null,"startedAt":"2026-11-13T12:34:00Z","completedAt":null,"updatedAt":"2026-11-13T12:34:00Z"}}
            })).expect("canonical assistant entry"));
            let rows = derive_messages_timeline_rows(&MessagesTimelineRowsInput { timeline_entries: vec![entry], ..Default::default() });
            let row = rows.into_iter().find(|row| matches!(&row.content, MessagesTimelineRowContent::Message { message, .. } if message.role == "assistant")).map(Shared);
            assistant_timeline_row(assistant_row(computed(move || row.clone())), AssistantRowContext::new(AssistantRowsState::default()))
        }))))
        .child(text(move || SharedString::from(panel.get().and_then(|panel| panel.with(|panel| panel.active().and_then(|surface| match &surface.content {
            t3_ui::screens::panel::SurfaceContent::File { relative_path, reveal_line, .. } => Some(format!("assistant-panel:{relative_path};line:{reveal_line:?}")),
            _ => None,
        }))).unwrap_or_default())))
}

#[test]
fn assistant_reply_file_links_use_shared_prose_and_the_real_panel_action() {
    for window in [DESKTOP, PHONE] {
        let mut app = Headless::start(EngineConfig {
            ui_sets: t3_ui::registries(),
            viewport: size(px(window.0), px(window.1)),
            ..EngineConfig::new(Root::of::<AssistantFileFixture>())
        })
        .expect("assistant fixture");
        app.wait_for("role:link[main.rs · L12:C3]")
            .expect("file link from the assistant's shared prose path");
        app.press("role:link[main.rs · L12:C3]")
            .expect("open file from reply");
        app.wait_for("text:assistant-panel:src/main.rs;line:Some(12)")
            .expect("PanelHost action retained path and reveal line");
        assert!(app.screen().opened_urls().is_empty());
    }
}
