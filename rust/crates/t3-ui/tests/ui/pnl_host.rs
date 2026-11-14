//! PNL-HOST integration fixtures; the frozen app shell is mounted by the integrator.

use crate::support::{DESKTOP, PHONE};
use nsh_core::prelude::*;
use nsh_engine::{EngineConfig, Root};
use nsh_testing::Headless;
use t3_client::types::ThreadKey;
use t3_ui::i18n::{Text as I18nText, t};
use t3_ui::screens::panel::{
    Action, PanelHost, PanelRegistration, PanelRegistry, PanelStore, SurfaceKind, chat_right_panel,
    panel_target, right_panel_tabs,
};

fn key(environment: &str) -> ThreadKey {
    ThreadKey {
        environment: t3_client::types::EnvironmentId(environment.into()),
        thread: t3_client::types::ThreadId("same-thread".into()),
    }
}

fn control(label: &str, action: impl Fn() + 'static) -> impl IntoElement {
    nsh_base::button()
        .accessibility_label(label.to_owned())
        .on_press(action)
        .child(text(SharedString::from(label.to_owned())))
}

#[derive(Clone, PartialEq, Props)]
struct ContentProps {
    name: Prop<SharedString>,
}

#[component]
fn Content(props: ContentProps) -> impl IntoElement {
    let expanded = signal(false);
    view()
        .flex_1()
        .min_h(px(0.0))
        .child(text(move || format!("content:{}", props.name.get())))
        .child(control("Expand", move || expanded.set(true)))
        .child(when(expanded, || text(t("Expanded file details"))))
        .child(
            scroll()
                .flex_1()
                .min_h(px(0.0))
                .test_id("files-scroll")
                .child(view().child((0..80).map(|index| {
                    view()
                        .h(px(40.0))
                        .test_id(&format!("file-row-{index}"))
                        .child(text(index.to_string()))
                }))),
        )
}

#[component]
fn TargetContent() -> impl IntoElement {
    let expanded = signal(false);
    view()
        .child(control("Expand target", move || expanded.set(true)))
        .child(when(expanded, || text(t("Target stays expanded"))))
}

#[component]
fn Host() -> impl IntoElement {
    t3_ui::theme::install(&Default::default());
    let closed = signal(Vec::<String>::new());
    let saved = signal(String::new());
    let host = PanelHost::new(PanelStore::default(), move |change| {
        closed.update(|ids| ids.extend(change.closed.into_iter().map(|surface| surface.id)));
        saved.set(change.state.encode().expect("client panel codec"));
    });
    let thread = stored(key("first"));
    let other = stored(key("second"));
    let state = host.panel(thread.get());
    let other_state = host.panel(other.get());
    let unavailable = signal(None::<I18nText>);
    let opens = signal(0);
    let mounts = signal(0);
    let registrations =
        [(SurfaceKind::Files, "files"), (SurfaceKind::Diff, "diff")].map(|(kind, singleton)| {
            PanelRegistration::new(
                kind,
                Prop::bind(move || unavailable.get()),
                move |key| {
                    opens.update(|n| *n += 1);
                    host.apply(&key, Action::Open(singleton.into()));
                },
                move |context| {
                    mounts.update(|n| *n += 1);
                    let name = context
                        .surface
                        .get_untracked()
                        .expect("registered surface")
                        .id;
                    let thread = stored(context.thread);
                    view()
                        .flex_1()
                        .min_h(px(0.0))
                        .child(control("Hide workspace panel", move || {
                            context.host.apply(&thread.get(), Action::Close)
                        }))
                        .child(content().name(SharedString::from(name)))
                        .into_element()
                },
            )
        });
    let registry = PanelRegistry::new(registrations.into());
    let dock_open = signal(false);
    let width = signal(540.0);
    let workspace = view()
        .flex_1()
        .min_h(px(0.0))
        .test_id("thread-workspace")
        .child(control("Toggle workspace panel", move || {
            host.apply(&thread.get(), Action::ToggleVisibility)
        }))
        .child(control("Toggle details", move || {
            host.toggle_thread_panel(&thread.get(), true)
        }))
        .child(control("Toggle terminal", move || {
            dock_open.update(|open| *open = !*open)
        }))
        .child(control("Files", move || {
            registry.open(SurfaceKind::Files, &thread.get())
        }))
        .child(control("Diff", move || {
            registry.open(SurfaceKind::Diff, &thread.get())
        }))
        .child(control("Open other environment", move || {
            host.apply(&other.get(), Action::Open("diff".into()))
        }))
        .child(control("Make unavailable", move || {
            unavailable.set(Some(I18nText::from("The environment is offline.")))
        }))
        .child(control("Try disabled surface", move || {
            registry.open(SurfaceKind::Files, &thread.get())
        }))
        .child(control("Reorder files", move || {
            host.apply(
                &thread.get(),
                Action::Move {
                    surface: "files".into(),
                    to: 1,
                },
            )
        }))
        .child(control("Open new surface menu", move || {
            host.request_new_surface(&thread.get())
        }))
        .child(control("Open many files", move || {
            for index in 0..12 {
                host.apply(
                    &thread.get(),
                    Action::OpenFile {
                        path: format!("long-file-{index:02}.rs"),
                        line: None,
                    },
                );
            }
        }))
        .child(text(move || format!("closed:{}", closed.get().join(","))))
        .child(text(move || {
            format!(
                "state:{};other:{};opens:{};mounts:{};width:{}",
                state.with(|p| p
                    .surfaces
                    .iter()
                    .map(|s| s.id.as_str())
                    .collect::<Vec<_>>()
                    .join(",")),
                other_state.with(|p| p.is_open),
                opens.get(),
                mounts.get(),
                width.get()
            )
        }))
        .child(text(move || {
            if saved.get().is_empty() {
                t("No saved changes")
            } else {
                t("Client record emitted")
            }
        }))
        .child(
            panel_target()
                .open(move || host.thread_panel_open(&thread.get(), true))
                .child(text(t("Thread details target"))),
        )
        .child(
            panel_target()
                .open(dock_open)
                .child(text(t("Terminal target"))),
        )
        .child(view().flex_1());
    let controls = control("Close workspace panel", move || {
        host.apply(&thread.get(), Action::Close)
    });
    view().w_full().h_full().child(
        chat_right_panel(
            host,
            thread.get(),
            Prop::bind(|| window().width.0),
            width.into(),
            0,
            move |next| width.set(next),
        )
        .fill("workspace", workspace)
        .child(right_panel_tabs(host, thread.get(), registry, controls)),
    )
}

fn start(viewport: (f32, f32)) -> Headless {
    let mut config = EngineConfig {
        ui_sets: t3_ui::registries(),
        viewport: size(px(viewport.0), px(viewport.1)),
        ..EngineConfig::new(Root::of::<Host>())
    };
    config.environment.os = nsh_core::platform::Os::MacOs;
    if viewport == PHONE {
        config.environment.idiom = nsh_core::platform::Idiom::Phone;
    }
    let mut app = Headless::start(config).expect("workspace host starts");
    app.wait_for("Toggle workspace panel")
        .expect("thread workspace");
    app
}

#[test]
fn the_three_toggle_targets_are_independent_and_reversible() {
    let mut app = start(DESKTOP);
    assert!(app.screen().shows("Thread details target"));
    assert!(!app.screen().shows("Terminal target"));
    app.press("role:button[Toggle terminal]")
        .expect("open dock");
    app.wait_for("Terminal target").expect("dock target");
    app.press("role:button[Toggle details]")
        .expect("hide details");
    assert!(!app.screen().shows("Thread details target"));
    app.press("role:button[Toggle workspace panel]")
        .expect("open empty panel");
    app.wait_for("Open a surface").expect("launcher");
    assert!(app.screen().shows("Terminal target"));
    app.press("role:button[Close workspace panel]")
        .expect("hide workspace panel");
    app.press("role:button[Toggle details]")
        .expect("restore details");
    app.press("role:button[Toggle terminal]")
        .expect("hide dock");
    app.wait_for("Thread details target")
        .expect("details return");
    assert!(!app.screen().shows("Terminal target"));
}

#[test]
fn tab_selection_close_neighbor_and_reorder_use_the_client_reducer() {
    let mut app = start(DESKTOP);
    app.press("role:button[Files]").expect("open files");
    app.wait_for("text:content:files").expect("files mounted");
    app.press("role:button[Diff]").expect("open diff");
    app.wait_for("text:content:diff").expect("diff active");
    assert!(!app.screen().shows("content:files"));
    app.press("role:tab[Files]").expect("select files");
    app.wait_for("text:content:files").expect("files active");
    app.press("role:button[Reorder files]").expect("move tab");
    app.wait_for("text:state:diff,files;other:false;opens:2;mounts:2;width:540")
        .expect("source order changes without remounting");
    app.hover("role:tab[Files]").expect("show close");
    app.press("role:button[Close Files]")
        .expect("close selected surface");
    app.wait_for("text:content:diff")
        .expect("neighbor activates");
    app.wait_for("text:closed:files")
        .expect("closed tab passed to history");
    app.hover("role:tab[Diff]").expect("show close");
    app.press("role:button[Close Diff]")
        .expect("close last surface");
    app.wait_for("text:closed:files,diff")
        .expect("last close reaches history");
    assert!(!app.screen().shows("content:diff"));
    app.press("role:button[Toggle workspace panel]")
        .expect("show empty host");
    app.wait_for("Open a surface").expect("empty launcher");
    assert!(app.screen().shows("Client record emitted"));
}

#[test]
fn bulk_close_remembers_the_active_tab_last_for_the_client_reopen_history() {
    let mut app = start(DESKTOP);
    app.press("role:button[Files]").expect("open files");
    app.press("role:button[Diff]").expect("open diff");
    app.wait_for("text:content:diff").expect("active diff");
    app.right_click("role:tab[Files]").expect("tab menu");
    app.wait_for("role:menuitem[Close all]")
        .expect("bulk action");
    app.press("role:menuitem[Close all]").expect("close tabs");
    app.wait_for("text:closed:files,diff")
        .expect("active tab remembered last");
    assert!(!app.screen().shows("content:diff"));
}

#[test]
fn pointer_drag_reorders_tabs_without_recreating_their_content() {
    let mut app = start(DESKTOP);
    app.press("role:button[Files]").expect("files");
    app.press("role:button[Diff]").expect("diff");
    app.wait_for("text:content:diff").expect("two tabs");
    app.drag(
        "id:panel-tab-files",
        nsh_core::geometry::point(px(90.0), px(0.0)),
    )
    .expect("drag to second tab");
    app.wait_for("text:state:diff,files;other:false;opens:2;mounts:2;width:540")
        .expect("client order changed; views retained");
    assert!(
        app.screen().shows("content:diff"),
        "dragging does not select a different tab"
    );
    app.press("role:tab[Files]")
        .expect("moved tab remains selectable");
    app.wait_for("text:content:files")
        .expect("same files content");
}

#[test]
fn overflowing_tabs_reveal_the_active_file_and_scroll_controls_move_the_native_viewport() {
    let mut app = start(DESKTOP);
    app.press("role:button[Open many files]")
        .expect("open overflowing tabs");
    app.wait_for("text:content:file:long-file-11.rs")
        .expect("last file active");
    app.wait_for("role:button[Scroll tabs left]")
        .expect("overflow controls");
    assert!(
        !app.screen()
            .accessibility_state("role:button[Scroll tabs left]")
            .expect("left control")
            .disabled
    );
    let before = app
        .screen()
        .layout_nodes()
        .into_iter()
        .find(|node| node.test_id.as_deref() == Some("panel-tab-file:long-file-00.rs"))
        .expect("first tab")
        .bounds
        .origin
        .x;
    app.press("role:button[Scroll tabs left]")
        .expect("scroll back");
    let after = app
        .screen()
        .layout_nodes()
        .into_iter()
        .find(|node| node.test_id.as_deref() == Some("panel-tab-file:long-file-00.rs"))
        .expect("first tab")
        .bounds
        .origin
        .x;
    assert!(
        after > before,
        "the content moves right as the horizontal offset decreases"
    );
}

#[test]
fn launcher_shortcuts_and_the_new_surface_command_reach_the_registered_actions() {
    let mut app = start(DESKTOP);
    app.press("role:button[Toggle workspace panel]")
        .expect("empty launcher");
    app.wait_for("Open a surface").expect("launcher");
    app.press_key("ctrl+f")
        .expect("modified letter belongs to other shortcuts");
    assert!(app.screen().shows("Open a surface"));
    app.press_key("f")
        .expect("open files through the launcher shortcut");
    app.wait_for("text:content:files")
        .expect("registered action");
    app.press("role:button[Open new surface menu]")
        .expect("rightPanel.new command");
    app.wait_for("role:menuitem[Diff]").expect("add menu");
    app.press("role:menuitem[Diff]").expect("create diff");
    app.wait_for("text:content:diff").expect("diff selected");
}

fn scroller(app: &Headless) -> (nsh_core::NodeId, Points) {
    let nodes = app.screen().layout_nodes();
    let scroll = nodes
        .iter()
        .find(|node| node.test_id.as_deref() == Some("files-scroll"))
        .expect("scroll viewport");
    let first = nodes
        .iter()
        .find(|node| node.test_id.as_deref() == Some("file-row-0"))
        .expect("first row");
    assert!(scroll.bounds.size.height > px(0.0));
    (scroll.id, scroll.bounds.origin.y - first.bounds.origin.y)
}

#[test]
fn tabs_keep_child_state_and_native_scroll_across_tab_changes_close_and_all_layouts() {
    let mut app = start(DESKTOP);
    app.press("role:button[Files]").expect("open files");
    app.wait_for("text:content:files").expect("files");
    app.press("role:button[Expand]")
        .expect("expand child-local state");
    app.scroll("id:files-scroll", px(400.0))
        .expect("scroll files");
    let original = scroller(&app);
    assert_eq!(original.1, px(400.0));
    app.press("role:button[Diff]").expect("another tab");
    app.wait_for("text:content:diff").expect("diff");
    app.press("role:tab[Files]")
        .expect("return to retained files");
    app.wait_for("Expanded file details")
        .expect("child still expanded");
    assert_eq!(scroller(&app), original);
    for viewport in [(800.0, 800.0), PHONE, DESKTOP] {
        app.set_viewport(size(px(viewport.0), px(viewport.1)))
            .expect("change presentation");
        app.wait_for("Expanded file details")
            .expect("retained content");
        assert_eq!(scroller(&app), original);
        app.press("role:button[Hide workspace panel]")
            .expect("hide");
        app.wait_for("Toggle workspace panel")
            .expect("workspace restored");
        assert!(!app.screen().shows("Expanded file details"));
        app.press("role:button[Toggle workspace panel]")
            .expect("reopen");
        app.wait_for("Expanded file details")
            .expect("same local state");
        assert_eq!(scroller(&app), original);
    }
}

#[test]
fn compact_back_and_medium_escape_close_the_host_without_closing_its_tab() {
    for viewport in [PHONE, (800.0, 800.0)] {
        let mut app = start(viewport);
        app.press("role:button[Files]").expect("open files");
        app.wait_for("text:content:files").expect("panel");
        if viewport == PHONE {
            app.back().expect("native back");
        } else {
            app.press_key("Escape").expect("sheet escape");
        }
        app.wait_for("Toggle workspace panel").expect("dismissed");
        app.press("role:button[Toggle workspace panel]")
            .expect("reopen");
        app.wait_for("text:content:files").expect("same tab");
        app.press("role:button[Hide workspace panel]")
            .expect("hide");
        app.wait_for("text:state:files;other:false;opens:1;mounts:1;width:540")
            .expect("no tab-close/remount");
    }
}

#[test]
fn unavailable_actions_fail_closed_and_environment_keys_do_not_collide() {
    let mut app = start(DESKTOP);
    app.press("role:button[Open other environment]")
        .expect("different environment, same thread id");
    app.wait_for("text:state:;other:true;opens:0;mounts:0;width:540")
        .expect("scoped to other environment");
    app.press("role:button[Make unavailable]")
        .expect("capability revoked");
    app.press("role:button[Try disabled surface]")
        .expect("programmatic invocation");
    app.press("role:button[Toggle workspace panel]")
        .expect("show launcher");
    app.wait_for("The environment is offline.")
        .expect("visible disabled reason");
    app.press("role:button[Close workspace panel]")
        .expect("hide launcher");
    app.wait_for("text:state:;other:true;opens:0;mounts:0;width:540")
        .expect("no service call or panel creation");
}

#[test]
fn native_resize_changes_only_the_pane_and_commits_its_clamped_width() {
    let mut app = start(DESKTOP);
    app.press("role:button[Files]").expect("open files");
    app.wait_for("text:content:files").expect("pane");
    app.drag(
        "id:workspace-panel-resize",
        nsh_core::geometry::point(px(-100.0), px(0.0)),
    )
    .expect("grow pane from its left edge");
    app.wait_for("text:state:files;other:false;opens:1;mounts:1;width:640")
        .expect("width committed once, content stays mounted");
    let pane = app
        .screen()
        .layout_nodes()
        .into_iter()
        .find(|node| node.test_id.as_deref() == Some("workspace-panel-host"))
        .expect("pane host");
    assert_eq!(pane.bounds.size.width, px(640.0));
    app.press("role:button[Hide workspace panel]")
        .expect("hide");
    app.press("role:button[Toggle workspace panel]")
        .expect("reopen");
    app.wait_for("text:content:files").expect("same pane");
    let pane = app
        .screen()
        .layout_nodes()
        .into_iter()
        .find(|node| node.test_id.as_deref() == Some("workspace-panel-host"))
        .expect("pane host");
    assert_eq!(pane.bounds.size.width, px(640.0));
}

#[component]
fn RetainedTarget() -> impl IntoElement {
    let open = signal(true);
    view()
        .child(control("Toggle", move || {
            open.update(|value| *value = !*value)
        }))
        .child(panel_target().open(open).child(target_content()))
}

#[test]
fn details_and_terminal_mounting_seams_retain_child_owned_state() {
    let mut app =
        Headless::start(EngineConfig::new(Root::of::<RetainedTarget>())).expect("target host");
    app.wait_for("Expand target").expect("target mounted");
    app.press("role:button[Expand target]").expect("expand");
    app.press("role:button[Toggle]").expect("close");
    assert!(!app.screen().shows("Target stays expanded"));
    app.press("role:button[Toggle]").expect("reopen");
    app.wait_for("Target stays expanded")
        .expect("same child component");
}

#[component]
fn ProjectedWorkspace() -> impl IntoElement {
    use std::rc::Rc;
    use t3_client::connection::PairingInput;
    use t3_ui::runtime::{Rt, backend::Backend, demo, host::Choice};
    use t3_ui::state::nav::{Navigator, Route};

    t3_ui::theme::install(&Default::default());
    provide_context(t3_ui::theme::text_view_parts());
    provide_context(t3_ui::state::panels::Panels::new());
    let store = t3_client::ClientStore::default();
    let backend: Rc<dyn Backend> = Rc::new(demo::DemoBackend::new(store.clone()));
    provide_context(Rt::new(store, backend.clone(), Choice::Demo));
    t3_ui::app::provide_without_client();
    let key = demo::demo_thread(&t3_client::types::EnvironmentId(
        "demo-demo.t3.codes".into(),
    ));
    provide_context(Navigator::new(Route::Thread(key.clone())));
    let ready = signal(false);
    spawn(async move {
        backend
            .pair(PairingInput {
                pairing_url: Some(demo::DEMO_PAIRING_URL.into()),
                ..Default::default()
            })
            .await
            .expect("real demo projection");
        ready.set(true);
    });
    view().w_full().h_full().child(when(ready, move || {
        t3_ui::screens::panel::thread_workspace(key.clone())
    }))
}

#[test]
fn production_workspace_mounts_real_details_and_header_toggle_targets() {
    for window in [DESKTOP, PHONE] {
        let mut app = Headless::start(EngineConfig {
            ui_sets: t3_ui::registries(),
            viewport: size(px(window.0), px(window.1)),
            ..EngineConfig::new(Root::of::<ProjectedWorkspace>())
        })
        .expect("production thread workspace");
        app.wait_for("I added clamp next to sum and the tests pass.")
            .expect("real timeline");
        if window == DESKTOP {
            app.wait_for("demo.t3.codes")
                .expect("default inline details from real environment");
            app.press("role:button[Toggle thread details panel]")
                .expect("close details");
            assert!(!app.screen().shows("demo.t3.codes"));
        }
        app.press("role:button[Toggle thread details panel]")
            .expect("open details");
        app.wait_for("demo.t3.codes")
            .expect("actual details target");
        if window == PHONE {
            app.press_key("Escape").expect("dismiss popup");
        }
        app.press("role:button[Toggle right panel]")
            .expect("open workspace panel");
        app.wait_for("Open a surface")
            .expect("actual workspace panel target");
        app.press("role:button[Toggle right panel]")
            .expect("close workspace panel");
        app.wait_for("role:button[Toggle right panel]")
            .expect("return to thread");
    }
}
