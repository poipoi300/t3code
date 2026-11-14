//! CMP-BANNERS: stack ordering, overflow recovery and details interaction.
use crate::support::{DESKTOP, PHONE};
use nsh_core::prelude::*;
use nsh_engine::{EngineConfig, Root};
use nsh_testing::{Headless, LayoutNode};
use std::rc::Rc;
use t3_ui::i18n::Text as UiText;
use t3_ui::runtime::Shared;
use t3_ui::screens::composer::banners::{
    self, ComposerBannerGroup, ComposerBannerStackItem, NoticeContent, NoticeTextStyle,
    StackOptions,
};
use t3_ui::ui::{alert::composer_banner as banner, icon::Icon};

fn item(id: &str, title: &str, group: ComposerBannerGroup) -> Shared<ComposerBannerStackItem> {
    Shared(Rc::new(ComposerBannerStackItem {
        id: id.into(),
        variant: banner::Variant::Warning,
        group,
        icon: Icon::Info,
        custom_icon: None,
        title: NoticeContent::from(UiText::content(title.to_owned())),
        description: None,
        children: None,
        actions: None,
        dismiss_label: None,
        on_dismiss: None,
    }))
}

#[component]
fn BannerFixture() -> impl IntoElement {
    t3_ui::theme::install(&Default::default());
    let machine = signal(true);
    let notices = signal(true);
    let narrow = signal(true);
    let short = signal(false);

    let standalone = signal(0.0);
    view()
        .child(
            nsh_base::button()
                .accessibility_label(Some("Remove all notices".into()))
                .on_press(move || notices.set(false))
                .child(text(SharedString::from("Remove all notices"))),
        )
        .w_full()
        .h_full()
        .child(
            nsh_base::button()
                .accessibility_label(Some("Remove machine notice".into()))
                .on_press(move || machine.set(false))
                .child(text(SharedString::from("Remove machine notice"))),
        )
        .child(
            nsh_base::button()
                .accessibility_label(Some("Widen notice".into()))
                .on_press(move || narrow.set(false))
                .child(text(SharedString::from("Widen notice"))),
        )
        .child(
            nsh_base::button()
                .accessibility_label(Some("Replace content".into()))
                .on_press(move || short.set(true))
                .child(text(SharedString::from("Replace content"))),
        )
        .child(view().w_full().child(banners::composer_banner_stack(
            move || {
                if !notices.get() {
                    return Vec::new();
                }
                let mut items = vec![item(
                    "attention",
                    "Branch changed",
                    ComposerBannerGroup::Attention,
                )];
                if machine.get() {
                    items.push(item(
                        "environment",
                        "Offline",
                        ComposerBannerGroup::Environment,
                    ));
                }
                items
            },
            StackOptions {
                in_column: true,
                ..Default::default()
            },
        )))
        .child(banner::attachment(
            banner::AttachmentProps {
                in_column: true,
                last: true,
            },
            banner::root(Default::default(), |_| {
                text(SharedString::from("Approval drawer"))
            }),
        ))
        .child(
            view()
                .w(move || px(if narrow.get() { 64.0 } else { 600.0 }))
                .child(dynamic(move || {
                    let short = short.get();
                    untracked(move || {
                        banners::notice_text(
                            NoticeContent::from(UiText::content(if short {
                                "Ready".to_owned()
                            } else {
                                "Waiting on a subagent with a long name".to_owned()
                            })),
                            NoticeTextStyle::Title,
                            standalone,
                            "Show details".into(),
                        )
                    })
                })),
        )
}

fn start(window: (f32, f32)) -> Headless {
    let mut sets = t3_ui::registries();
    let mut fixtures = Registry::new();
    fixtures.register::<BannerFixture>();
    sets.push(fixtures);
    let mut config = EngineConfig {
        ui_sets: sets,
        clock: nsh_engine::Clock::manual(std::time::UNIX_EPOCH),
        viewport: size(px(window.0), px(window.1)),
        ..EngineConfig::new(Root::of::<BannerFixture>())
    };
    config.environment.os = if window == PHONE {
        nsh_core::platform::Os::Android
    } else {
        nsh_core::platform::Os::Linux
    };
    config.environment.idiom = if window == PHONE {
        nsh_core::platform::Idiom::Phone
    } else {
        nsh_core::platform::Idiom::Desktop
    };
    let mut app = Headless::start(config).expect("start");
    app.wait_for("Branch changed").expect("notices mounted");
    app
}
fn node(app: &Headless, id: &str) -> LayoutNode {
    app.screen()
        .layout_nodes()
        .into_iter()
        .find(|n| n.test_id.as_deref() == Some(id))
        .expect("notice layout")
}

#[test]
fn machine_notices_sort_first_and_removing_them_keeps_the_next_attachment() {
    for window in [PHONE, DESKTOP] {
        let mut app = start(window);
        assert!(
            node(&app, "environment").bounds.origin.y < node(&app, "attention").bounds.origin.y
        );
        app.press("role:button[Remove machine notice]")
            .expect("remove");
        assert!(!app.screen().shows("Offline"));
        assert!(app.screen().shows("Branch changed"));
        assert!(app.screen().shows("Approval drawer"));
        app.press("role:button[Remove all notices]")
            .expect("clear stack");
        assert!(!app.screen().shows("Branch changed"));
        assert!(app.screen().shows("Approval drawer"));
    }
}

#[test]
fn details_open_on_press_and_hover_and_leave_when_text_fits_or_changes() {
    for window in [PHONE, DESKTOP] {
        let mut app = start(window);
        app.wait_for("role:button[Show details]")
            .expect("overflow detected");
        app.press("role:button[Show details]")
            .expect("details press");
        app.wait_for("label:Notice details")
            .expect("full notice popup");
        app.press_key("escape").expect("dismiss popup");
        app.press("role:button[Widen notice]")
            .expect("grow text container");
        assert_eq!(app.screen().count("role:button[Show details]"), Ok(0));
    }
    let mut app = start(DESKTOP);
    app.hover("role:button[Show details]")
        .expect("hover trigger");
    app.wait_for("label:Notice details").expect("hover details");
    app.press_key("escape").expect("close");
    app.press("role:button[Replace content]").expect("replace");
    app.wait_for("Ready").expect("new content");
    assert_eq!(app.screen().count("role:button[Show details]"), Ok(0));
}

#[component]
fn ScriptNoticeFixture() -> impl IntoElement {
    t3_ui::theme::install(&Default::default());
    let no_banner = signal(0.0);
    let script = (0..400)
        .map(|line| format!("echo \"line {line}\""))
        .collect::<Vec<_>>()
        .join("\n");
    view()
        .w_full()
        .h_full()
        .justify_end()
        .items_end()
        .p(px(64.0))
        .child(view().w(px(640.0)).child(banners::notice_text(
            NoticeContent::from(UiText::content(script)),
            NoticeTextStyle::Title,
            no_banner,
            "Show script details".into(),
        )))
}

#[test]
fn multiline_details_stay_above_the_trigger_and_cap_the_long_content() {
    let mut sets = t3_ui::registries();
    let mut fixtures = Registry::new();
    fixtures.register::<ScriptNoticeFixture>();
    sets.push(fixtures);
    let mut config = EngineConfig {
        ui_sets: sets,
        viewport: size(px(DESKTOP.0), px(DESKTOP.1)),
        clock: nsh_engine::Clock::manual(std::time::UNIX_EPOCH),
        ..EngineConfig::new(Root::of::<ScriptNoticeFixture>())
    };
    config.environment.os = nsh_core::platform::Os::Linux;
    config.environment.idiom = nsh_core::platform::Idiom::Desktop;
    let mut app = Headless::start(config).expect("start script notice");
    app.wait_for("role:button[Show script details]")
        .expect("multiline source is truncated");
    app.hover("role:button[Show script details]")
        .expect("hover details");
    app.wait_for("label:Notice details")
        .expect("full script popup");
    let trigger = app
        .screen()
        .id("role:button[Show script details]")
        .expect("trigger");
    let popup = app.screen().id("role:dialog").expect("popup surface");
    let nodes = app.screen().layout_nodes();
    let trigger = nodes
        .iter()
        .find(|n| n.id == trigger)
        .expect("trigger bounds")
        .bounds;
    let popup = nodes
        .iter()
        .find(|n| n.id == popup)
        .expect("popup bounds")
        .bounds;
    let gap = trigger.origin.y - (popup.origin.y + popup.size.height);
    assert!(gap >= px(0.0) && gap < px(8.0), "popup gap: {gap:?}");
    assert!(node(&app, "notice-details-content").bounds.size.height <= px(320.0));
}
