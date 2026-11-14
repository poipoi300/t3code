//! Interaction contracts for the primitive before TL-ROWS-WORK-2 and SB-SHELVES mount it.

use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use crate::support::{DESKTOP, PHONE};
use nsh_core::{MountItem, prelude::*};
use nsh_engine::{Clock, EngineConfig, Root};
use nsh_testing::Headless;
use t3_ui::i18n::t;
use t3_ui::theme;
use t3_ui::ui::collapsible::*;

#[component]
fn StatefulChild() -> impl IntoElement {
    let count = signal(0);
    view()
        .child(nsh_base::input().test_id("retained-input"))
        .child(
            nsh_base::button()
                .accessibility_label(t("Increment"))
                .on_press(move || count.update(|count| *count += 1))
                .child(text(move || format!("count:{}", count.get()))),
        )
}

#[component]
fn Harness() -> impl IntoElement {
    theme::install(&Default::default());
    let open = signal(false);
    let requested = signal(false);
    let disabled = signal(false);
    let changes = signal(0);
    let handle = CollapsibleHandle::controlled(
        move || open.get(),
        move || disabled.get(),
        move |next| {
            requested.set(next);
            changes.update(|n| *n += 1);
        },
    );
    view()
        .child(collapsible(
            view()
                .child(collapsible_trigger(
                    handle,
                    "Worked for".into(),
                    text(t("Worked for")),
                    "",
                ))
                .child(collapsible_panel(handle, true, stateful_child())),
        ))
        .child(
            nsh_base::button()
                .accessibility_label(t("Accept"))
                .on_press(move || open.set(requested.get()))
                .child(text(t("Accept"))),
        )
        .child(
            nsh_base::button()
                .accessibility_label(t("Disable"))
                .on_press(move || disabled.set(true))
                .child(text(t("Disable"))),
        )
        .child(text(move || {
            format!("changes:{};requested:{}", changes.get(), requested.get())
        }))
}

fn start<C: Component<Props = ()>>(window: (f32, f32), reduced: bool) -> Headless {
    let mut config = EngineConfig {
        ui_sets: t3_ui::registries(),
        viewport: size(px(window.0), px(window.1)),
        clock: Clock::manual(SystemTime::UNIX_EPOCH),
        ..EngineConfig::new(Root::of::<C>())
    };
    config.environment.reduce_motion = reduced;
    config.environment.os = nsh_core::platform::Os::MacOs;
    let mut app = Headless::start(config).expect("start fixture");
    app.first_render().expect("render fixture");
    app
}

fn expanded(app: &Headless) -> bool {
    app.screen()
        .accessibility_state("role:button[Worked for]")
        .expect("trigger accessibility")
        .expanded
        .expect("expanded state")
}

fn toggle(app: &mut Headless) {
    app.press("role:button[Worked for]")
        .expect("request toggle");
    app.press("role:button[Accept]")
        .expect("accept controlled value");
}

#[test]
fn controlled_requests_wait_for_the_owner_and_disabled_triggers_are_inert() {
    for window in [PHONE, DESKTOP] {
        let mut app = start::<Harness>(window, true);
        app.press("role:button[Worked for]").expect("request open");
        assert!(!expanded(&app));
        assert!(!app.screen().shows("count:0"));
        app.wait_for("text:changes:1;requested:true")
            .expect("one request");
        app.press("role:button[Accept]").expect("accept");
        assert!(expanded(&app));
        app.wait_for("text:count:0").expect("panel appears");
        app.press("role:button[Disable]").expect("disable");
        assert!(app.press("role:button[Worked for]").is_err());
        assert!(expanded(&app));
        assert!(app.screen().shows("changes:1;requested:true"));
    }
}

#[test]
fn child_local_state_and_native_input_survive_a_completed_close_and_reopen() {
    for reduced in [false, true] {
        let mut app = start::<Harness>(DESKTOP, reduced);
        toggle(&mut app);
        app.type_text("id:retained-input", "unsent draft")
            .expect("edit native input");
        app.press("role:button[Increment]")
            .expect("change child local state");
        toggle(&mut app);
        app.run_for(Duration::from_millis(250))
            .expect("finish close on manual clock");
        assert!(!app.screen().shows("count:1"));
        assert!(
            app.press("role:button[Increment]").is_err(),
            "closed content is inert"
        );
        toggle(&mut app);
        app.wait_for("text:count:1")
            .expect("same component instance");
        assert_eq!(
            app.screen().input_text("id:retained-input").expect("input"),
            Some("unsent draft")
        );
    }
}

#[test]
fn reopening_cancels_the_pending_detach_and_motion_respects_the_environment() {
    for reduced in [false, true] {
        let mut app = start::<Harness>(PHONE, reduced);
        let motions = Arc::new(Mutex::new(Vec::new()));
        let seen = motions.clone();
        app.observe(move |_, batch| {
            for item in &batch.items {
                if let MountItem::LayoutAnimation(animation) = item {
                    seen.lock().expect("motions").push(*animation);
                }
            }
        });
        toggle(&mut app);
        toggle(&mut app);
        toggle(&mut app);
        app.run_for(Duration::from_millis(250))
            .expect("pass obsolete closing deadline");
        assert!(expanded(&app));
        assert!(app.screen().shows("count:0"));
        let motions = motions.lock().expect("motions");
        if reduced {
            assert!(motions.is_empty());
        } else {
            assert_eq!(motions.len(), 3);
            assert!(motions.iter().all(|animation| {
                animation.create.is_none()
                    && animation.delete.is_none()
                    && animation
                        .update
                        .is_some_and(|update| update.duration_ms == 200.0)
            }));
        }
    }
}

#[component]
fn Section() -> impl IntoElement {
    theme::install(&Default::default());
    let handle = CollapsibleHandle::new(false);
    collapsible(
        view()
            .child(collapsible_section_header(
                handle,
                "Projects".into(),
                text(t("Projects")),
                Tone::Emphasized,
                SectionHeaderVariant::Panel,
                Some(section_header_status(text(SharedString::from("3"))).into_element()),
            ))
            .child(collapsible_content(
                handle,
                false,
                text(t("Project details")),
            )),
    )
}

#[test]
fn section_header_activates_with_the_keyboard_and_reports_both_states() {
    for window in [PHONE, DESKTOP] {
        let mut app = start::<Section>(window, false);
        assert_eq!(
            app.screen()
                .accessibility_state("role:button[Projects]")
                .expect("header")
                .expanded,
            Some(false)
        );
        app.press_key("tab").expect("focus header");
        app.press_key("enter").expect("expand");
        app.wait_for("Project details").expect("content");
        assert_eq!(
            app.screen()
                .accessibility_state("role:button[Projects]")
                .expect("header")
                .expanded,
            Some(true)
        );
        app.press_key("space").expect("collapse");
        assert!(!app.screen().shows("Project details"));
    }
}

// ---- Ownership: the panel as timeline rows mount it (a row built untracked inside a region,
// a controlled handle, `dynamic` content, no animation). `strict()` fails the engine on any
// panic it catches. ----

#[component]
fn RowLikePanel() -> impl IntoElement {
    t3_ui::engine_panics::strict();
    theme::install(&Default::default());
    let expanded = signal(false);
    let mounted = signal(true);
    view()
        .child(
            nsh_base::button()
                .accessibility_label(t("Unmount"))
                .on_press(move || mounted.set(false))
                .child(text(t("Unmount"))),
        )
        .child(dynamic(move || {
            mounted.get().then(|| {
                untracked(|| {
                    let handle = CollapsibleHandle::controlled(
                        move || expanded.get(),
                        || false,
                        move |open| expanded.set(open),
                    );
                    let panel = dynamic(move || {
                        expanded
                            .get()
                            .then(|| text("Inside the panel").into_element())
                    });
                    collapsible(
                        view()
                            .child(collapsible_trigger(handle, "Group".into(), text("Group"), ""))
                            .child(collapsible_panel(handle, false, panel)),
                    )
                    .into_element()
                })
            })
        }))
}

#[test]
fn a_row_like_panel_opens_closes_and_unmounts_without_an_engine_panic() {
    let mut app = Headless::start(EngineConfig {
        ui_sets: {
            let mut sets = t3_ui::registries();
            let mut fixtures = Registry::new();
            fixtures.register::<RowLikePanel>();
            sets.push(fixtures);
            sets
        },
        viewport: size(px(DESKTOP.0), px(DESKTOP.1)),
        ..EngineConfig::new(Root::of::<RowLikePanel>())
    })
    .expect("fixture");
    app.wait_for("role:button[Group]").expect("mounted");
    app.press("role:button[Group]").expect("open");
    app.wait_for("Inside the panel").expect("opened");
    app.press("role:button[Group]").expect("close");
    app.settle();
    app.press("role:button[Unmount]").expect("unmount");
    app.settle();
    assert!(!app.screen().shows("Group"), "unmounted");
}
